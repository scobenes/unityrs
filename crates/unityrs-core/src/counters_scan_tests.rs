//! Every refusal the core makes is made where CR8's counters count it
//! (`crate::counters`; `docs/plans/batch.md` section 6, CR8): a source scan
//! of `src/`, outside tests and the counters themselves, for each name that
//! would make one uncounted, every hit allowed by its file and its line's
//! text. It reads the tokens of live code alone
//! (`unityrs_testkit::waits::live_code`: comments, test items and literals
//! blanked), across lines, so a name in a comment or a string is no hit
//! and a call split over lines is one.
//!
//! A `LimitError` outside `error.rs` is no form here: its fields are
//! private to `error.rs` (round 3), so the compiler refuses one built
//! anywhere else, under any name or alias, and no query hands one out
//! (round 4: `Meter::work_used_up` returns a `WorkPast`, not an error). In
//! `error.rs` the scan reads each construction and each alias of it, and
//! pins its derive list, so no derived constructor (`Default`,
//! `Deserialize`) is added unseen.
//!
//! What it cannot see: a name a macro builds out of pieces (`paste`-style),
//! and a call reached with no name at all (a function value passed in from
//! a test). A method cannot be imported under another name (`use … as`),
//! so a rename reaches these methods only through such a value.

use std::path::{Path, PathBuf};

use unityrs_testkit::waits::live_code;

/// A form of code that makes a refusal the counters would miss.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Form {
    /// In `error.rs`, a `LimitError` or `Self` followed by `{` that is no
    /// header (`-> Self {`, `impl … for LimitError {`): a construction,
    /// where only `LimitError::refused`, which counts, may be.
    Built,
    /// In `error.rs`, another name for `LimitError` (or `Self`): a `type`
    /// alias of it, or an import of it `as` another name, which a
    /// construction could be spelled with (round 4).
    Alias,
    /// A budget's `try_reserve`, named after `.` or `::`, outside
    /// `Meter::reserve`, which counts its refusal.
    Budget,
    /// A `try_reserve_exact` outside the two marked sites, which mark the
    /// thread and count the allocator's refusal.
    Allocation,
}

/// The lines allowed, by file, form and text, and why.
const ALLOWED: &[(&str, Form, &str, &str)] = &[
    (
        "error.rs",
        Form::Built,
        "Self { kind, value, max }",
        "`LimitError::refused`, which counts",
    ),
    (
        "limits/meter.rs",
        Form::Budget,
        ".try_reserve(bytes, account.what)",
        "`Meter::reserve`, which counts",
    ),
    (
        "limits/vec.rs",
        Form::Allocation,
        "vec.try_reserve_exact(additional)",
        "the fallible helper, marked and counted",
    ),
    (
        "limits/vec.rs",
        Form::Allocation,
        "moved.try_reserve_exact(vec.len())",
        "`shrink_charged`'s move, marked and counted",
    ),
];

/// `LimitError`'s derives: none of them builds one (round 4: pinned, so a
/// derived `Default` or `Deserialize`, which would, is not added unseen).
const DERIVES: &[&str] = &[
    "Clone",
    "Copy",
    "Debug",
    "PartialEq",
    "Eq",
    "thiserror::Error",
];

/// A token of live code: an identifier (a raw one, `r#name`, as `name`),
/// `::`, `->`, or one other character; and its line's index.
struct Token<'a> {
    text: &'a str,
    line: usize,
}

fn ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

fn tokens(code: &str) -> Vec<Token<'_>> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 0);
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if ident(c) {
            // `r#name` is `name`.
            let from = if b[i..].starts_with(b"r#") && b.get(i + 2).is_some_and(|c| ident(*c)) {
                i + 2
            } else {
                i
            };
            let mut end = from;
            while end < b.len() && ident(b[end]) {
                end += 1;
            }
            out.push(Token {
                text: &code[from..end],
                line,
            });
            i = end;
        } else {
            let two = b[i..].starts_with(b"::") || b[i..].starts_with(b"->");
            let end = if two { i + 2 } else { i + char_len(c) };
            out.push(Token {
                text: &code[i..end],
                line,
            });
            i = end;
        }
    }
    out
}

/// The byte length of the UTF-8 character `lead` starts.
fn char_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// Whether the `type` item at `i` names `LimitError` (or `Self`) after
/// its `=`, before its `;`.
fn aliases(t: &[Token<'_>], i: usize) -> bool {
    let rest = t[i + 1..].iter().take_while(|x| x.text != ";");
    rest.skip_while(|x| x.text != "=")
        .any(|x| x.text == "LimitError" || x.text == "Self")
}

/// The form of the token at `i`, if it is one; `defining` for `error.rs`.
fn form(t: &[Token<'_>], i: usize, defining: bool) -> Option<Form> {
    let before = i.checked_sub(1).map_or("", |p| t[p].text);
    let after = t.get(i + 1).map_or("", |n| n.text);
    match t[i].text {
        "try_reserve" if before == "." || before == "::" => Some(Form::Budget),
        "try_reserve_exact" => Some(Form::Allocation),
        "LimitError" | "Self"
            if defining
                && after == "{"
                && !["->", "impl", "for", "struct", "enum"].contains(&before) =>
        {
            Some(Form::Built)
        }
        "LimitError" if defining && after == "as" => Some(Form::Alias),
        "type" if defining && aliases(t, i) => Some(Form::Alias),
        _ => None,
    }
}

/// Each hit in `text`: its form and its line's text, trimmed. `defining`
/// for `error.rs`, where a construction or an alias is a form too.
fn flagged(text: &str, defining: bool) -> Vec<(Form, String)> {
    let code = live_code(text);
    let lines: Vec<&str> = text.lines().collect();
    let t = tokens(&code);
    (0..t.len())
        .filter_map(|i| form(&t, i, defining).map(|f| (f, lines[t[i].line].trim().to_owned())))
        .collect()
}

/// The traits derived for `struct LimitError` in `text`, by every
/// `derive(…)` among the attributes before it (one inside a `cfg_attr`
/// too), each path joined (`thiserror::Error`).
fn limit_error_derives(text: &str) -> Vec<String> {
    let code = live_code(text);
    let t = tokens(&code);
    let Some(at) = (1..t.len()).find(|&i| t[i - 1].text == "struct" && t[i].text == "LimitError")
    else {
        return Vec::new();
    };
    // Back to the end of the item before it.
    let start = t[..at]
        .iter()
        .rposition(|x| x.text == ";" || x.text == "}")
        .map_or(0, |p| p + 1);
    let mut out = Vec::new();
    let mut i = start;
    while i < at {
        if t[i].text == "derive" && t.get(i + 1).is_some_and(|x| x.text == "(") {
            let mut path = String::new();
            let mut j = i + 2;
            while j < at && t[j].text != ")" {
                if t[j].text == "," {
                    out.push(std::mem::take(&mut path));
                } else {
                    path.push_str(t[j].text);
                }
                j += 1;
            }
            if !path.is_empty() {
                out.push(path);
            }
            i = j;
        }
        i += 1;
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every refusal the core makes, outside tests (files named `*tests.rs`
/// and `#[cfg(test)]` items) and the counters themselves (`counters.rs`,
/// `testing.rs`, whose tables are no allocation of the core's), is made at
/// a counted site: every hit of a form is an allowed line, and every
/// allowed line is still there (round 2: the scan matched call forms, so a
/// budget's `MemoryBudget::try_reserve(` passed it; round 3: it read line
/// by line, so a call split over lines passed it; round 4: an alias of
/// `LimitError` in `error.rs` passed it). And `LimitError`'s derives are
/// the pinned ones.
#[test]
fn every_refusal_is_made_where_it_is_counted() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut found = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        if name.ends_with("tests.rs") || name == "counters.rs" || name == "testing.rs" {
            continue;
        }
        let rel = path.strip_prefix(&src).unwrap().to_string_lossy();
        let rel = rel.replace('\\', "/");
        let text = std::fs::read_to_string(&path).unwrap();
        for (form, line) in flagged(&text, rel == "error.rs") {
            found.push((rel.clone(), form, line));
        }
        if rel == "error.rs" {
            assert_eq!(limit_error_derives(&text), DERIVES, "LimitError's derives");
        }
    }
    found.sort();
    let mut allowed: Vec<(String, Form, String)> = ALLOWED
        .iter()
        .map(|(file, form, line, _)| ((*file).to_owned(), *form, (*line).to_owned()))
        .collect();
    allowed.sort();
    assert_eq!(found, allowed);
}

/// Each form is caught in each of its spellings, across lines and as a
/// raw identifier, and none in a comment, a literal (a raw C string's
/// quotes and an escaped quote included), a definition or a test item,
/// with the code after a test item still read.
#[test]
fn the_scan_catches_each_form() {
    let fixture = r##"
fn e(b: &dyn MemoryBudget) { let _ = b.try_reserve(1, what); }
fn f(b: &dyn MemoryBudget) { let _ = MemoryBudget::try_reserve(b, 1, what); }
fn g(v: &mut Vec<u8>) { let _ = v.try_reserve_exact(1); }
fn h(v: &mut Vec<u8>) { let _ = cr#"a"b"#; v.try_reserve_exact(1); }
fn i(v: &mut Vec<u8>) { let _ = ('\'', b'"', c"}"); v.try_reserve_exact(1); }
fn j(v: &mut Vec<u8>) {
    v
        .
        try_reserve_exact(1)
}
fn k(b: &dyn MemoryBudget) { MemoryBudget::
    try_reserve(b, 1, what); }
fn l(b: &dyn MemoryBudget) { b.r#try_reserve(1, what); v.r#try_reserve_exact(1); }
// b.try_reserve(1) in a comment, /* and v.try_reserve_exact(1) */
fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> { x }
fn
    try_reserve(&self) {}
fn m(v: &mut Vec<u8>) { v.try_reserve(1); }
const S: &str = ".try_reserve( try_reserve_exact(";
#[cfg(test)]
mod tests {
    fn t(v: &mut Vec<u8>) { v.try_reserve_exact(1); }
}
fn after(v: &mut Vec<u8>) { v.try_reserve_exact(1); }
"##;
    let lines: Vec<&str> = fixture.lines().collect();
    let at = |hits: Vec<(Form, String)>| -> Vec<(Form, usize)> {
        hits.into_iter()
            .map(|(f, l)| (f, lines.iter().position(|x| x.trim() == l).unwrap()))
            .collect()
    };
    assert_eq!(
        at(flagged(fixture, false)),
        [
            (Form::Budget, 1),
            (Form::Budget, 2),
            (Form::Allocation, 3),
            (Form::Allocation, 4),
            (Form::Allocation, 5),
            (Form::Allocation, 9),
            (Form::Budget, 12),
            (Form::Budget, 13),
            (Form::Allocation, 13),
            // A `Vec`'s `try_reserve` has the budget's name: the scan reads
            // names, not types.
            (Form::Budget, 18),
            (Form::Allocation, 24),
        ]
    );
}

/// In the defining module, a construction of `LimitError` (by its name or
/// `Self`, on one line or split) and an alias of it (a `type`, an import
/// `as` another name) are hits, and a header, a return type or another
/// module's text is not (round 4: `type L = LimitError; L { .. }` passed).
#[test]
fn the_scan_reads_the_defining_module_s_constructions_and_aliases() {
    let fixture = "
impl LimitError {
    fn refused(kind: LimitKind, value: u64, max: u64) -> Self {
        Self { kind, value, max }
    }
}
impl Default
    for LimitError {
    fn default() -> Self { LimitError
        { kind, value: 0, max: 0 } }
}
impl std::fmt::Display for LimitError {}
pub fn bypass() -> LimitError {
    type L = LimitError;
    L { kind: LimitKind::DecodeWork, value: 1, max: 0 }
}
use self::LimitError as Other;
type Mine<T> =
    Option<Self>;
type Unrelated = u64;
";
    let hits = flagged(fixture, true);
    assert_eq!(
        hits,
        [
            (Form::Built, "Self { kind, value, max }".to_owned()),
            (Form::Built, "fn default() -> Self { LimitError".to_owned()),
            (Form::Alias, "type L = LimitError;".to_owned()),
            (Form::Alias, "use self::LimitError as Other;".to_owned()),
            (Form::Alias, "type Mine<T> =".to_owned()),
        ]
    );
    assert_eq!(flagged(fixture, false), []);
}

/// The derive list is read from every `derive` before the struct, so one
/// added, or one inside a `cfg_attr`, changes it.
#[test]
fn the_scan_reads_limit_errors_derives() {
    let pinned = "
pub struct Before;
/// A bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(\"limit\")]
pub struct LimitError { kind: LimitKind }
";
    assert_eq!(limit_error_derives(pinned), DERIVES);
    let added = pinned.replace("Eq, thiserror", "Eq, Default, thiserror");
    assert_ne!(limit_error_derives(&added), DERIVES);
    let gated = pinned.replace(
        "#[error",
        "#[cfg_attr(feature = \"serde\", derive(serde::Deserialize))]\n#[error",
    );
    assert_eq!(
        limit_error_derives(&gated).last().map(String::as_str),
        Some("serde::Deserialize")
    );
}
