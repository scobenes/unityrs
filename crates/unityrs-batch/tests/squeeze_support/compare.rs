//! What a run one below a need may be (X13), against its baseline: the
//! limit's own failure, the baseline's records with the refusal counted,
//! for `D` alone with an eviction counted, or, for `M`, exactly the records
//! a reference model derives from the baseline and the run's `M` skips.
//!
//! The model reads the baseline's records with the engine's test tap on
//! them ([`super::tap`]): each row's export-work increments, each skip
//! row's phase and an export skip's place in export order. Each `M` skip
//! names an output or object the baseline has (one that names none fails),
//! and the model applies `docs/BATCH_SPEC.md`'s rules to the baseline:
//! - a skip at export removes the outputs it names and their members (4.4,
//!   4.7); a skip at plan removes its object whole, the skips its planning
//!   would have found among them (an atlas whose read is refused plans none
//!   of its sprites);
//! - the surviving candidates' units form again (4.5's **Units**);
//! - an `M` skip row's every field is derived: its object's file and path
//!   id, its class from its output, the input, its sprite index, no detail
//!   and an `M` reason at the run's `M`;
//! - record order (4.4's **Record order**): the plan's skips in plan order,
//!   then the export's in export order, each export skip at the place the
//!   baseline's rows give it;
//! - the summary's skips are the skip rows, its status follows them (7.3's
//!   `inputs` row), and its export work is exactly the surviving rows'
//!   increments, each `M` skip's (a prefix of what it removed: the work
//!   counted before its refusal, `docs/plans/batch.md` 2.6 step 10) and
//!   the records file's, from its length.
//!
//! The run's records must equal the model's exactly and in order.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value as J, json};

use super::{Outcome, m_reason, run_failure};

type Object = (String, String);

fn parse(x: &Outcome) -> Vec<J> {
    x.records
        .iter()
        .map(|r| serde_json::from_str(r).unwrap())
        .collect()
}

fn kind(r: &J) -> &str {
    r["type"].as_str().unwrap_or_default()
}

fn object(r: &J) -> Object {
    (r["serialized_file"].to_string(), r["path_id"].to_string())
}

fn is_m_skip(r: &J) -> bool {
    kind(r) == "skip" && r["reason"].as_str().is_some_and(m_reason)
}

fn is(r: &J, k: &str) -> bool {
    kind(r) == "candidate" && r["kind"] == k
}

fn work(r: &J) -> u64 {
    r["work"]
        .as_array()
        .map_or(0, |w| w.iter().filter_map(|e| e[1].as_u64()).sum())
}

/// A records file's export work at `len` bytes: each full 64 KiB flush at
/// its size, the last at `max(4 KiB, its size rounded up to 4 KiB)`
/// (`docs/plans/batch.md` 2.6 step 10).
fn records_work(len: u64) -> u64 {
    let tail = len % (64 << 10);
    len - tail
        + if tail == 0 {
            0
        } else {
            tail.div_ceil(4096) * 4096
        }
}

/// A run's export work as its rows account for it.
fn accounted(rows: &[J], o: &Outcome) -> u64 {
    let tail: J = serde_json::from_str(&o.tail).unwrap_or_default();
    let tail = tail
        .as_array()
        .map_or(0, |t| t.iter().filter_map(|e| e[1].as_u64()).sum());
    rows.iter().map(work).sum::<u64>() + tail + records_work(o.records_len)
}

/// Each atlas crop's `mSprites` index, by row: an atlas's sprites are
/// planned and recorded in `mSprites` order (4.4's **The plan**), each a
/// candidate or a skip row naming its index, so the crops take, in record
/// order, the indices no skip row of their atlas names.
fn crop_indices(base: &[J]) -> Vec<Option<u64>> {
    let taken: BTreeSet<(Object, u64)> = base
        .iter()
        .filter(|r| kind(r) == "skip" && r["output"] == "atlas_sprite")
        .filter_map(|r| Some((object(r), r["sprite"].as_u64()?)))
        .collect();
    let mut next: BTreeMap<Object, u64> = BTreeMap::new();
    base.iter()
        .map(|r| {
            if !is(r, "atlas_sprite") {
                return None;
            }
            let n = next.entry(object(r)).or_insert(0);
            while taken.contains(&(object(r), *n)) {
                *n += 1;
            }
            *n += 1;
            Some(*n - 1)
        })
        .collect()
}

/// The baseline rows an `M` skip removes. At export: one output of its
/// object (`output`; a crop by its `sprite` index), or for `atlas` its JSON
/// and every crop. At plan: its object whole, candidates and skips, since
/// none of its outputs is planned. None found is a failure.
fn resolve(skip: &J, base: &[J], crops: &[Option<u64>]) -> Result<Vec<usize>, String> {
    let output = skip["output"].as_str().unwrap_or_default();
    let sprite = skip["sprite"].as_u64();
    let plan = skip["phase"] == "plan";
    let rows: Vec<usize> = (0..base.len())
        .filter(|&i| object(&base[i]) == object(skip))
        .filter(|&i| match (plan, output, sprite) {
            (true, "texture" | "sprite" | "atlas", None) => {
                matches!(kind(&base[i]), "candidate" | "skip")
            }
            (false, "atlas", None) => is(&base[i], "atlas_json") || is(&base[i], "atlas_sprite"),
            (false, "atlas_sprite", Some(n)) => is(&base[i], "atlas_sprite") && crops[i] == Some(n),
            (false, k, None) => is(&base[i], k),
            _ => false,
        })
        .collect();
    let found = rows.len() == 1 || ((plan || output == "atlas") && !rows.is_empty());
    if found {
        Ok(rows)
    } else {
        Err(format!("an M skip names no baseline output: {skip}"))
    }
}

/// A surviving candidate's unit (4.5's **Units**): its sheet texture's key
/// while that texture is made; otherwise its atlas's JSON key while that
/// JSON is made; otherwise none, a plain candidate.
fn unit(base: &[J], i: usize, alive: &dyn Fn(usize) -> bool) -> J {
    let u = &base[i]["unit"];
    let owner = base
        .iter()
        .position(|r| kind(r) == "candidate" && r["key"] == *u);
    let Some(owner) = owner.filter(|_| !u.is_null()) else {
        return J::Null;
    };
    if alive(owner) {
        return u.clone();
    }
    if !is(&base[owner], "texture") {
        return J::Null;
    }
    let json = (0..base.len())
        .find(|&j| is(&base[j], "atlas_json") && object(&base[j]) == object(&base[i]));
    json.filter(|&j| alive(j))
        .map_or(J::Null, |j| base[j]["key"].clone())
}

/// A skip row's place in plan order (4.4's **The plan**): its kind's rank
/// (`textures`, `sprites`, `atlases`), its object's (its file in order of
/// first appearance, then its path id) and, in an atlas, its JSON before
/// its sprites in `mSprites` order.
fn plan_key(r: &J, files: &[String]) -> (u8, usize, i64, i64) {
    let rank = match r["output"].as_str() {
        Some("texture") => 0,
        Some("sprite") => 1,
        _ => 2,
    };
    let file = r["serialized_file"].to_string();
    let at = files.iter().position(|f| *f == file).unwrap_or(usize::MAX);
    let sprite = r["sprite"].as_i64().unwrap_or(-1);
    (rank, at, r["path_id"].as_i64().unwrap_or_default(), sprite)
}

/// The run's `M` skip row as the model derives it, from the baseline row
/// it took (`first`) and the run's own `M`; the actual row supplies only
/// which `M` refusal it met and, at export, how much of what it removed it
/// counted first.
fn derived(skip: &J, base: &[J], taken: &[usize], ctx: (&J, u64), anchor: Option<usize>) -> J {
    let (input, m) = ctx;
    let first = &base[taken[0]];
    let output = skip["output"].as_str().unwrap_or_default();
    let class = match output {
        "texture" => "Texture2D",
        "sprite" => "Sprite",
        _ => "MonoBehaviour",
    };
    let reasons = [
        format!("limit(input_memory, {m})"),
        format!("limit(input_bytes, {m})"),
        format!("limit(decode_work, {})", m.saturating_mul(4)),
        "budget(input)".to_owned(),
    ];
    let reason = skip["reason"]
        .as_str()
        .filter(|r| reasons.iter().any(|x| x == r));
    // What the removed outputs counted, in order, members after each.
    let removed: Vec<J> = taken
        .iter()
        .flat_map(|&i| {
            let key = &base[i]["key"];
            let members = base[i + 1..]
                .iter()
                .take_while(|r| kind(r) == "member" && r["key"] == *key);
            std::iter::once(&base[i]).chain(members)
        })
        .flat_map(|r| r["work"].as_array().cloned().unwrap_or_default())
        .collect();
    let mine = skip["work"].as_array().cloned().unwrap_or_default();
    let counted = if anchor.is_some() && removed.starts_with(&mine) {
        mine
    } else {
        Vec::new()
    };
    let mut row = json!({
        "type": "skip", "input": input, "serialized_file": first["serialized_file"],
        "path_id": first["path_id"], "class": class, "output": output,
        "sprite": if output == "atlas_sprite" { skip["sprite"].clone() } else { J::Null },
        "reason": reason.map_or(J::Null, J::from), "detail": null, "cut": false,
        "work": counted, "phase": if anchor.is_some() { "export" } else { "plan" },
    });
    if let Some(a) = anchor {
        row["anchor"] = J::from(a);
    }
    row
}

/// The records the model expects below `M`, the summary aside.
fn expected(base: &[J], skips: &[&J], ctx: (&J, u64)) -> Result<Vec<J>, String> {
    let crops = crop_indices(base);
    let mut gone = BTreeSet::new();
    let mut taken = Vec::new();
    for s in skips {
        let rows = resolve(s, base, &crops)?;
        if rows.iter().any(|i| !gone.insert(*i)) {
            return Err(format!("two M skips name one output: {s}"));
        }
        taken.push(rows);
    }
    // A removed candidate's members leave with it (4.7).
    let keys: Vec<&J> = gone
        .iter()
        .filter(|&&i| kind(&base[i]) == "candidate")
        .map(|&i| &base[i]["key"])
        .collect();
    let removed = |i: usize| {
        gone.contains(&i) || (kind(&base[i]) == "member" && keys.contains(&&base[i]["key"]))
    };
    let before = |at: usize| at - (0..at).filter(|&i| removed(i)).count();
    let alive = |i: usize| !gone.contains(&i);
    let mut body = Vec::new();
    let (mut plan, mut export) = (Vec::new(), Vec::new());
    for (i, r) in base.iter().enumerate() {
        if removed(i) {
            continue;
        }
        match (kind(r), r["phase"].as_str()) {
            ("candidate", _) => {
                let mut c = r.clone();
                c["unit"] = unit(base, i, &alive);
                body.push(c);
            }
            ("skip", Some("plan")) => plan.push((0, r.clone())),
            ("skip", _) => {
                let mut s = r.clone();
                let a = before(usize::try_from(r["anchor"].as_u64().unwrap_or_default()).unwrap());
                s["anchor"] = J::from(a);
                export.push(((a, 0), s));
            }
            _ => body.push(r.clone()),
        }
    }
    for (s, rows) in skips.iter().zip(&taken) {
        if s["phase"] == "plan" {
            plan.push((1, derived(s, base, rows, ctx, None)));
        } else {
            let a = before(rows[0]);
            export.push(((a, 1), derived(s, base, rows, ctx, Some(a))));
        }
    }
    let files: Vec<String> = base.iter().fold(Vec::new(), |mut v, r| {
        let f = r["serialized_file"].to_string();
        if !v.contains(&f) {
            v.push(f);
        }
        v
    });
    let keyed: Vec<_> = plan
        .iter()
        .filter(|(m, _)| *m == 0)
        .map(|(_, r)| plan_key(r, &files))
        .collect();
    if !keyed.is_sorted() {
        return Err("the model's plan order does not order the baseline's plan skips".into());
    }
    plan.sort_by_key(|(_, r)| plan_key(r, &files));
    export.sort_by_key(|(a, _)| *a);
    body.extend(plan.into_iter().map(|(_, r)| r));
    body.extend(export.into_iter().map(|(_, r)| r));
    Ok(body)
}

/// The summary the model expects: the baseline's, its skips the expected
/// skip rows, its status `complete_with_skips` once there is one (7.3's
/// `inputs` row), its export work `work`.
fn summary(mine: &J, base: &J, rows: &[J], work: u64) -> Result<(), String> {
    let mut want = base.clone();
    let skips = rows.iter().filter(|r| kind(r) == "skip").count();
    want["skips"] = J::from(skips);
    if skips > 0 && base["status"] == "complete" {
        want["status"] = J::from("complete_with_skips");
    }
    want["export_work"] = J::from(work);
    if *mine == want {
        Ok(())
    } else {
        Err(format!("summary {mine}: the model's is {want}"))
    }
}

/// The run's summary, which must be its one summary and its last row.
fn terminal(rows: &[J]) -> Result<(&J, &[J]), String> {
    let (last, body) = rows.split_last().ok_or("no records")?;
    if kind(last) != "input_summary" || body.iter().any(|r| kind(r) == "input_summary") {
        return Err("not one summary, last".into());
    }
    Ok((last, body))
}

/// The `M` pass's run one below its need, against `base`: its name, or
/// what is wrong with it.
///
/// # Errors
/// An outcome X13 does not allow, described.
///
/// # Panics
/// On a record that is not JSON (a test helper).
pub fn below_m(o: &Outcome, base: &Outcome) -> Result<&'static str, String> {
    if run_failure(o) || o.allocator > 0 || !o.clear {
        return Err(format!("not a squeeze outcome: {o:?}"));
    }
    if o.same_result(base) {
        return if o.m_refusals > 0 {
            Ok("absorbed")
        } else {
            Err("baseline records and no refusal of M".into())
        };
    }
    if o.state == "Failed" && m_reason(&o.reason) {
        return Ok("failed");
    }
    if (&o.state, &o.reason) != (&base.state, &base.reason) || !o.names_m() {
        return Err(format!("neither a failure nor a skip of M: {o:?}"));
    }
    let (mine, theirs) = (parse(o), parse(base));
    let ((mine_summary, body), (base_summary, base_body)) = (terminal(&mine)?, terminal(&theirs)?);
    if base_summary["export_work"].as_u64() != Some(accounted(base_body, base)) || base.tail != "[]"
    {
        return Err("the tap does not account for the baseline's export work".into());
    }
    let skips: Vec<&J> = body.iter().filter(|r| is_m_skip(r)).collect();
    let want = expected(base_body, &skips, (&base_summary["id"], o.m))?;
    if body != want.as_slice() {
        let at = body.iter().zip(&want).position(|(a, b)| a != b);
        let at = at.unwrap_or(body.len().min(want.len()));
        return Err(format!(
            "row {at}: {:?}; the model's {:?}",
            body.get(at).map(J::to_string),
            want.get(at).map(J::to_string)
        ));
    }
    if o.tail != "[]" {
        return Err(format!("export work no row accounts for: {}", o.tail));
    }
    summary(mine_summary, base_summary, &want, accounted(&want, o))?;
    Ok("skipped")
}

/// The `D` pass's run one below its need, against `base`.
///
/// # Errors
/// An outcome X13 does not allow, described.
pub fn below_d(o: &Outcome, base: &Outcome) -> Result<&'static str, String> {
    if run_failure(o) || o.allocator > 0 || !o.clear {
        return Err(format!("not a squeeze outcome: {o:?}"));
    }
    if o.state == "Failed" && o.reason == "budget(dependency)" {
        return Ok("failed");
    }
    match (o.same_result(base), o.d_refusals > 0, o.evictions > 0) {
        (true, true, _) => Ok("absorbed"),
        (true, false, true) => Ok("evicted"),
        _ => Err(format!("{o:?}")),
    }
}
