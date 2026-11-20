//! Freeze v8's admitted root configurations against their evidence (plan
//! section 2.4, B4a): `ADMITTED` is the variants of `e4.passing`, each
//! with the settings E4's fixture builds it with. The fixture's
//! `_clip_variants` (`tools/parity/anim_fixture_body.py`) is read as
//! values: its `_flags` and `_basis`, its `neutral` row, its loops and its
//! rows are parsed and evaluated here, so a changed axis order, level or
//! motion flag fails, not only a changed line.
use super::body_model::{FREEZE, token};
use serde_json::Value;
use std::collections::BTreeMap;
use unityrs_anim::humanoid::{
    body_model::U64,
    root::{ADMITTED, U32},
};

const FIXTURE: &str = include_str!("../../../../tools/parity/anim_fixture_body.py");

/// One `CLIP_VARIANTS` row.
#[derive(Clone, Debug, Default, PartialEq)]
#[allow(clippy::struct_excessive_bools, reason = "the fixture's row")]
struct Row {
    bake: [bool; 3],
    keep: [bool; 3],
    height_from_feet: bool,
    level: f32,
    orientation: f32,
    start: bool,
    motion: bool,
    knee: bool,
    goals: bool,
}

/// The source of the fixture's function `name`, its `def` line excluded.
fn function(name: &str) -> &'static str {
    let at = FIXTURE.find(&format!("\ndef {name}(")).unwrap();
    let body = &FIXTURE[at + 1..];
    let body = &body[body.find('\n').unwrap() + 1..];
    &body[..body.find("\n\n").unwrap()]
}

/// `_basis`, from its masks: `return (bool(bits & a), bool(bits & b), ...)`.
fn basis(bits: u32) -> [bool; 3] {
    let line = function("_basis").trim();
    let masks: Vec<u32> = line
        .split("bool(bits & ")
        .skip(1)
        .map(|part| part[..part.find(')').unwrap()].parse().unwrap())
        .collect();
    assert_eq!(line.matches("bool(").count(), 3, "{line}");
    [0, 1, 2].map(|k| bits & masks[k] != 0)
}

/// `_flags`, from its f-string: each `{bits >> n & 1}` after its label.
fn flags(bits: u32) -> String {
    let line = function("_flags").trim();
    let text = line
        .strip_prefix("return f\"")
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap();
    let mut out = String::new();
    for (k, part) in text.split('{').enumerate() {
        if k == 0 {
            out.push_str(part);
            continue;
        }
        let (expr, label) = part.split_once('}').unwrap();
        let shift = if let Some(rest) = expr.strip_prefix("bits >> ") {
            rest.strip_suffix(" & 1").unwrap().parse().unwrap()
        } else {
            assert_eq!(expr, "bits & 1");
            0
        };
        out.push_str(&(bits >> shift & 1).to_string());
        out.push_str(label);
    }
    out
}

/// `key=value, ...` at the top level of a `dict(...)`'s arguments.
fn arguments(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let (mut depth, mut from) = (0, 0);
    for (at, c) in text.char_indices().chain([(text.len(), ',')]) {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                let item = text[from..at].trim();
                if !item.is_empty() {
                    let (key, value) = item.split_once('=').unwrap();
                    out.push((key.trim(), value.trim()));
                }
                from = at + 1;
            }
            _ => {}
        }
    }
    out
}

/// `row | dict(args)`, with `bits` for `_basis(bits)`.
fn apply(mut row: Row, args: &str, bits: Option<u32>) -> Row {
    for (key, value) in arguments(args) {
        let triple = match value {
            "(False,) * 3" => Some([false; 3]),
            "(True,) * 3" => Some([true; 3]),
            "_basis(bits)" => Some(basis(bits.unwrap())),
            _ => None,
        };
        let flag = match value {
            "True" => Some(true),
            "False" => Some(false),
            _ => None,
        };
        match key {
            "bake" => row.bake = triple.unwrap(),
            "keep" => row.keep = triple.unwrap(),
            "height_from_feet" => row.height_from_feet = flag.unwrap(),
            "start" => row.start = flag.unwrap(),
            "motion" => row.motion = flag.unwrap(),
            "knee" => row.knee = flag.unwrap(),
            "goals" => row.goals = flag.unwrap(),
            "level" => row.level = value.parse().unwrap(),
            "orientation" => row.orientation = value.parse().unwrap(),
            _ => panic!("an unknown variant field {key}"),
        }
    }
    row
}

/// The text between `open` (at `from`) and its balancing `)`.
fn inside(text: &str, from: usize) -> &str {
    let mut depth = 0;
    for (at, c) in text[from..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[from + 1..from + at];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced")
}

/// `_clip_variants()`, evaluated from its statements.
fn clip_variants() -> BTreeMap<String, Row> {
    let body = function("_clip_variants");
    let at = body.find("neutral = dict(").unwrap() + "neutral = dict".len();
    let neutral = apply(Row::default(), inside(body, at), None);
    let mut rows = BTreeMap::new();
    let mut lines = body.lines().map(str::trim).peekable();
    while let Some(line) = lines.next() {
        if let Some(range) = line
            .strip_prefix("for bits in range(")
            .and_then(|rest| rest.strip_suffix("):"))
        {
            let bounds: Vec<u32> = range.split(", ").map(|v| v.parse().unwrap()).collect();
            let (low, high) = match bounds.as_slice() {
                [high] => (0, *high),
                [low, high] => (*low, *high),
                _ => panic!("{line}"),
            };
            let row = lines.next().unwrap();
            let prefix = row
                .strip_prefix("rows[\"")
                .and_then(|rest| rest.split_once("\" + _flags(bits)] = neutral | dict("))
                .unwrap();
            let args = prefix.1.strip_suffix(')').unwrap();
            for bits in low..high {
                let name = format!("{}{}", prefix.0, flags(bits));
                rows.insert(name, apply(neutral.clone(), args, Some(bits)));
            }
        } else if let Some(rest) = line.strip_prefix("rows[\"") {
            let (name, args) = rest.split_once("\"] = neutral | dict(").unwrap();
            let args = args.strip_suffix(')').unwrap();
            rows.insert(name.to_owned(), apply(neutral.clone(), args, None));
        } else if let Some(rest) = line.strip_prefix("rows |= {name + \"") {
            let (suffix, args) = rest.split_once("\": row | dict(").unwrap();
            let args = args
                .strip_suffix(") for name, row in list(rows.items())}")
                .unwrap();
            let derived: Vec<_> = rows
                .iter()
                .map(|(name, row)| (format!("{name}{suffix}"), apply(row.clone(), args, None)))
                .collect();
            rows.extend(derived);
        } else {
            assert!(
                ["rows = {}", "return rows"].contains(&line)
                    || line.starts_with("neutral = dict(")
                    || line.starts_with("start=False"),
                "an unread statement: {line}"
            );
        }
    }
    rows
}

/// `ADMITTED` is freeze v8's `e4.passing` (each configuration's variant)
/// with the fixture's settings for it; the unit roundoffs are its bounds'
/// and E4's thresholds its source's. Drift in any fails here.
#[test]
fn the_admitted_set_is_freeze_v8s() {
    let frozen: Value = serde_json::from_str(FREEZE).unwrap();
    let mut passing: Vec<&str> = frozen["e4"]["passing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().split('/').next().unwrap())
        .collect();
    passing.sort_unstable();
    passing.dedup();
    let mut admitted: Vec<&str> = ADMITTED.iter().map(|row| row.variant).collect();
    admitted.sort_unstable();
    assert_eq!(admitted, passing);
    let variants = clip_variants();
    assert_eq!(variants.len(), 39);
    for row in &ADMITTED {
        let want = &variants[row.variant];
        let got = Row {
            bake: row.bake,
            keep: row.keep,
            level: row.level,
            orientation: row.offset_degrees,
            motion: row.motion,
            ..Row::default()
        };
        assert_eq!(&got, want, "{}", row.variant);
        assert_eq!(row.level.to_bits(), want.level.to_bits());
        assert_eq!(row.offset_degrees.to_bits(), want.orientation.to_bits());
    }
    let bounds = FREEZE.find("\"bounds\": {").unwrap();
    assert_eq!(token("unit_roundoff_f32", bounds).to_bits(), U32.to_bits());
    assert_eq!(token("unit_roundoff_f64", bounds).to_bits(), U64.to_bits());
    let e4 = include_str!("../../../../tools/parity/anim_body_e4.py");
    for line in [
        "if not size > 4 * error:",
        "return normalize(vector), 2 * error / (size - error)",
        "gamma(4, U32) * (abs(a) + abs(b)) + abs(b - a) * gamma(4, U32) * (1 + abs(time - begin) * rate)",
        "return value, gamma(6, U32) * magnitude + slope * U32 * (abs(time) + abs(start))",
        "replay.require(slots[slot][0] == (0, 95, dof, 8, False)",
    ] {
        assert!(e4.contains(line), "{line}");
    }
    let model = include_str!("../../../../tools/parity/anim_body_model.py");
    assert!(model.contains("if not across > 2 * moved:"));
}

/// The parser reads the fixture as Python would: the bake mask `o1-y0-xz1`
/// bakes the orientation and XZ, and the `-start` twins start at origin.
#[test]
fn the_fixture_reads_as_python_evaluates_it() {
    let variants = clip_variants();
    assert_eq!(variants["bake-o1-y0-xz1"].bake, [true, false, true]);
    assert_eq!(variants["keep-o0-y1-xz0"].keep, [false, true, false]);
    assert!(variants["level-start"].start);
    assert_eq!(variants["level-start"].level.to_bits(), 0.125_f32.to_bits());
    assert!(variants["keep-o1-y1-xz1-motion"].motion);
    assert!(!variants.contains_key("bake-o0-y0-xz0"));
    assert!(variants["height-from-feet-knee-goals"].goals);
}
