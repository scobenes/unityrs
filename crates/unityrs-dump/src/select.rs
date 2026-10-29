//! Which inputs a dump reads, and what it names them: a port of
//! `tools/parity/unitypy_dump.py`'s `collect_inputs` and its `--name`
//! globs (`fnmatch.fnmatchcase`), so both producers dump the same inputs
//! under the same names in the same order (`docs/CANONICAL_DUMP.md` section
//! 4.2 "Names", section 5). `tests/fixtures/dump/select.json` records the
//! oracle's selections over a synthetic tree and index, and Python's
//! `fnmatchcase` over names and patterns, invalid UTF-8 included.
//!
//! Names are bytes throughout: a path's name is its bytes as given (Python's
//! `os.fsencode` of the surrogate-escaped `str`), never resolved, so a
//! symlink is named after itself, and inputs sort by those bytes.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The index entry kinds `--kind` defaults to (`unitypy_dump.py` `INDEX_KINDS`).
pub const INDEX_KINDS: [&str; 2] = ["bundle", "unitypy-sample"];

/// What to dump: the selection flags of `unitypy_dump.py`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The inputs as given: files or folders, or with `index` one corpus
    /// `index.json`. `unitypy_dump.py` takes one; several are collected one
    /// by one and must not repeat a name.
    pub inputs: Vec<PathBuf>,
    /// Each input is a corpus index (`--index`).
    pub index: bool,
    /// Index entry kinds to dump (`--kind`).
    pub kinds: Vec<String>,
    /// Index builds to dump, as Python's `str()` of the entry's `build`
    /// (`--build`); `None` keeps every build.
    pub builds: Option<Vec<String>>,
    /// Globs on the dump name, any of which keeps an input (`--name`, as
    /// bytes, matched as `fnmatch.fnmatchcase` matches the surrogate-escaped
    /// `str`s).
    pub names: Vec<Vec<u8>>,
    /// Keep this many inputs spread evenly over the sorted list (`--sample`;
    /// 0 keeps all, a negative count none, as Python's truthiness and
    /// `range` have it).
    pub sample: Option<i64>,
    /// Keep the first this many (`--limit`; 0 keeps all, a negative count
    /// drops that many from the end, as Python's `items[:limit]`).
    pub limit: Option<i64>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            index: false,
            kinds: INDEX_KINDS.iter().map(|k| (*k).to_owned()).collect(),
            builds: None,
            names: Vec::new(),
            sample: None,
            limit: None,
        }
    }
}

const _: () = crate::send_sync::<Selection>();

/// One input to dump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    /// The path to read.
    pub path: PathBuf,
    /// Its dump name: the path below the folder, the file's name, or the
    /// index entry's `path`, as bytes.
    pub name: Vec<u8>,
    /// The sha256 its index entry pins, as written there.
    pub sha256: Option<String>,
}

const _: () = crate::send_sync::<Input>();

/// `collect_inputs`: the inputs in canonical order, filtered, sampled and
/// limited (`unitypy_dump.py` `collect_inputs`).
pub fn collect(sel: &Selection) -> Result<Vec<Input>, String> {
    let mut items = Vec::new();
    for given in &sel.inputs {
        let before = items.len();
        collect_one(sel, given, &mut items)?;
        if sel.inputs.len() > 1 {
            // Several inputs: a name two of them give would be two records
            // under one key, which no single run of the oracle writes.
            let (old, new) = items.split_at(before);
            if let Some(dup) = new.iter().find(|n| old.iter().any(|o| o.name == n.name)) {
                return Err(format!(
                    "{} is named by two inputs",
                    String::from_utf8_lossy(&dup.name)
                ));
            }
        }
    }
    if !sel.names.is_empty() {
        let pats: Vec<Glob> = sel
            .names
            .iter()
            .map(|p| Glob::new(p))
            .collect::<Result<_, _>>()?;
        items.retain(|i| pats.iter().any(|p| p.matches(&i.name)));
    }
    // Names order by their bytes, as Rust's OsStr does on Unix (a stable sort,
    // as Python's `list.sort`).
    items.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(s) = sel.sample {
        let n = items.len();
        // `if args.sample and len(items) > args.sample`
        if s != 0 && i64::try_from(n).map_or(true, |n| n > s) {
            let keep = usize::try_from(s).unwrap_or(0);
            let mut out = Vec::with_capacity(keep);
            for k in 0..keep {
                // `items[(k * n) // args.sample]`; k < keep < n, so in range.
                let at = u128::try_from(k).unwrap_or(0) * u128::try_from(n).unwrap_or(0)
                    / u128::try_from(keep).unwrap_or(1);
                if let Some(item) = usize::try_from(at).ok().and_then(|at| items.get(at)) {
                    out.push(item.clone());
                }
            }
            items = out;
        }
    }
    if let Some(l) = sel.limit {
        // `if args.limit: items = items[: args.limit]`
        if l != 0 {
            let n = i64::try_from(items.len()).unwrap_or(i64::MAX);
            let end = if l < 0 { (n + l).max(0) } else { l.min(n) };
            items.truncate(usize::try_from(end).unwrap_or(0));
        }
    }
    Ok(items)
}

fn collect_one(sel: &Selection, given: &Path, items: &mut Vec<Input>) -> Result<(), String> {
    if sel.index {
        let text = std::fs::read(given).map_err(|e| format!("{}: {e}", given.display()))?;
        let index: serde_json::Value =
            serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", given.display()))?;
        // Entries are relative to the index file itself, symlinks resolved.
        let resolved =
            std::fs::canonicalize(given).map_err(|e| format!("{}: {e}", given.display()))?;
        let root = resolved.parent().unwrap_or(Path::new("/"));
        let files = index
            .get("files")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("{}: the index has no files list", given.display()))?;
        for e in files {
            let kind = e.get("kind");
            if !kind
                .and_then(serde_json::Value::as_str)
                .is_some_and(|k| sel.kinds.iter().any(|x| x == k))
            {
                continue;
            }
            if let Some(builds) = &sel.builds {
                let build = py_str(e.get("build").unwrap_or(&serde_json::Value::Null));
                if !builds.contains(&build) {
                    continue;
                }
            }
            let rel = e
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("{}: an index entry has no path", given.display()))?;
            let sha256 = match e.get("sha256") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(other) => Some(other.to_string()),
            };
            items.push(Input {
                path: root.join(rel),
                name: rel.as_bytes().to_vec(),
                sha256,
            });
        }
    } else if std::fs::metadata(given).is_ok_and(|m| m.is_dir()) {
        walk(given, &[], items);
    } else if std::fs::metadata(given).is_ok_and(|m| m.is_file()) {
        // `os.path.basename`: everything after the last `/`.
        let bytes = given.as_os_str().as_bytes();
        let base = bytes
            .iter()
            .rposition(|b| *b == b'/')
            .map_or(bytes, |i| bytes.get(i + 1..).unwrap_or_default());
        items.push(Input {
            path: given.to_path_buf(),
            name: base.to_vec(),
            sha256: None,
        });
    } else {
        return Err(format!("no such input: {}", given.display()));
    }
    Ok(())
}

/// `os.walk(given)` with `followlinks=False`, keeping each entry for which
/// `os.path.isfile` holds: a symlink to a file is kept under its own name, a
/// symlink to a folder is neither walked nor kept, a dangling symlink and
/// anything else that is not a regular file are left out, and a folder that
/// cannot be read is skipped, as `os.walk` skips it.
fn walk(dir: &Path, rel: &[u8], items: &mut Vec<Input>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let path = dir.join(&name);
        let mut child = rel.to_vec();
        if !child.is_empty() {
            child.push(b'/');
        }
        child.extend_from_slice(name.as_bytes());
        let is_link = entry.file_type().is_ok_and(|t| t.is_symlink());
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => {
                if !is_link {
                    walk(&path, &child, items);
                }
            }
            Ok(m) if m.is_file() => items.push(Input {
                path,
                name: child,
                sha256: None,
            }),
            _ => {}
        }
    }
}

/// Python's `str()` of a JSON value as `json.loads` gives it, for `--build`
/// (`str(e.get("build"))`): `None`, `True`/`False`, an integer's digits, a
/// float's `repr`, a string itself. Lists and objects (no index holds one)
/// get their JSON text.
#[must_use]
pub fn py_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "None".to_owned(),
        serde_json::Value::Bool(true) => "True".to_owned(),
        serde_json::Value::Bool(false) => "False".to_owned(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => {
            if n.is_f64() {
                n.as_f64().map_or_else(|| n.to_string(), py_float_repr)
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

/// `repr(float)`: the shortest digits that round-trip, positional when the
/// decimal exponent is in -4..16, else `d.ddde+XX` (`PyOS_double_to_string`
/// with mode `'r'`).
fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    // `{:e}` gives the shortest round-trip digits: "-1.2345e-7".
    let sci = format!("{x:e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (neg, mant) = mant.strip_prefix('-').map_or((false, mant), |m| (true, m));
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if (-4..16).contains(&exp) {
        let point = exp + 1; // digits before the decimal point
        if point <= 0 {
            out.push_str("0.");
            for _ in 0..-point {
                out.push('0');
            }
            out.push_str(&digits);
        } else {
            let p = usize::try_from(point).unwrap_or(0);
            if digits.len() <= p {
                out.push_str(&digits);
                for _ in digits.len()..p {
                    out.push('0');
                }
                out.push_str(".0");
            } else {
                out.push_str(digits.get(..p).unwrap_or_default());
                out.push('.');
                out.push_str(digits.get(p..).unwrap_or_default());
            }
        }
    } else {
        out.push_str(digits.get(..1).unwrap_or_default());
        if digits.len() > 1 {
            out.push('.');
            out.push_str(digits.get(1..).unwrap_or_default());
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let _ = write!(out, "{:02}", exp.unsigned_abs());
    }
    out
}

/// A name or pattern as Python sees it: the code points of its bytes
/// decoded as UTF-8 with `surrogateescape` (an invalid byte `b` is U+DC00+b).
#[must_use]
pub fn py_units(bytes: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.extend(chunk.valid().chars().map(u32::from));
        out.extend(chunk.invalid().iter().map(|b| 0xDC00 + u32::from(*b)));
    }
    out
}

/// `fnmatch.fnmatchcase(name, pattern)` on bytes (see [`py_units`]).
pub fn fnmatchcase(name: &[u8], pattern: &[u8]) -> Result<bool, String> {
    Ok(Glob::new(pattern)?.matches(name))
}

/// One element of a translated pattern: each but `Star` matches exactly one
/// code point.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    /// `*`: any run, newlines included (`(?s:...)`).
    Star,
    /// `?`, or a negated empty set: any one code point.
    Any,
    /// A literal code point.
    Lit(u32),
    /// A set: inclusive ranges, negated or not.
    Set {
        negate: bool,
        ranges: Vec<(u32, u32)>,
    },
    /// An empty set (`(?!)`): matches nothing.
    Never,
}

/// A `--name` pattern, translated as Python 3.11's `fnmatch.translate` does.
#[derive(Clone, Debug)]
pub struct Glob {
    toks: Vec<Tok>,
}

const _: () = crate::send_sync::<Glob>();

const BS: u32 = b'\\' as u32;
const DASH: u32 = b'-' as u32;
const BANG: u32 = b'!' as u32;
const CLOSE: u32 = b']' as u32;

impl Glob {
    /// Translates `pattern` (`fnmatch.translate`). A pattern Python's `re`
    /// would refuse (a set whose range runs backwards after `translate`
    /// removed the empty ones) is an error, as `fnmatchcase` raises.
    pub fn new(pattern: &[u8]) -> Result<Self, String> {
        let pat = py_units(pattern);
        let n = pat.len();
        let at = |k: usize| pat.get(k).copied();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < n {
            let c = at(i).unwrap_or(0);
            i += 1;
            if c == u32::from(b'*') {
                // Consecutive stars are one.
                if toks.last() != Some(&Tok::Star) {
                    toks.push(Tok::Star);
                }
            } else if c == u32::from(b'?') {
                toks.push(Tok::Any);
            } else if c == u32::from(b'[') {
                let mut j = i;
                if at(j) == Some(BANG) {
                    j += 1;
                }
                if at(j) == Some(CLOSE) {
                    j += 1;
                }
                while j < n && at(j) != Some(CLOSE) {
                    j += 1;
                }
                if j >= n {
                    toks.push(Tok::Lit(u32::from(b'[')));
                } else {
                    let stuff = class_stuff(&pat, i, j);
                    i = j + 1;
                    toks.push(class_tok(&stuff)?);
                }
            } else {
                toks.push(Tok::Lit(c));
            }
        }
        Ok(Self { toks })
    }

    /// Whether `name` matches the whole pattern (`re.match(...\Z)`).
    #[must_use]
    pub fn matches(&self, name: &[u8]) -> bool {
        let s = py_units(name);
        // The two-pointer wildcard match: every token but `*` takes exactly
        // one code point, so the leftmost match of each fixed piece is the
        // one Python's atomic `(?>.*?fixed)` takes, and it never needs to
        // backtrack past the last star.
        let (mut p, mut k) = (0usize, 0usize);
        let mut star: Option<(usize, usize)> = None;
        while k < s.len() {
            match self.toks.get(p) {
                Some(Tok::Star) => {
                    star = Some((p, k));
                    p += 1;
                }
                Some(t) if s.get(k).is_some_and(|c| one(t, *c)) => {
                    p += 1;
                    k += 1;
                }
                _ => match star {
                    Some((sp, sk)) => {
                        p = sp + 1;
                        k = sk + 1;
                        star = Some((sp, sk + 1));
                    }
                    None => return false,
                },
            }
        }
        self.toks
            .get(p..)
            .is_some_and(|rest| rest.iter().all(|t| *t == Tok::Star))
    }
}

fn one(t: &Tok, c: u32) -> bool {
    match t {
        Tok::Star | Tok::Any => true,
        Tok::Lit(l) => *l == c,
        Tok::Set { negate, ranges } => {
            ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *negate
        }
        Tok::Never => false,
    }
}

/// The body `translate` writes inside `[...]` for `pat[i..j]`, escapes
/// included, before the negation is read.
fn class_stuff(pat: &[u32], i: usize, j: usize) -> Vec<u32> {
    let slice = |a: usize, b: usize| pat.get(a..b).unwrap_or_default().to_vec();
    let raw = slice(i, j);
    let escape = |s: &[u32], dash: bool| -> Vec<u32> {
        let mut out = Vec::with_capacity(s.len());
        for &c in s {
            if c == BS || (dash && c == DASH) {
                out.push(BS);
            }
            out.push(c);
        }
        out
    };
    let mut stuff = if raw.contains(&DASH) {
        let mut chunks: Vec<Vec<u32>> = Vec::new();
        let mut i = i;
        let mut k = if pat.get(i) == Some(&BANG) {
            i + 2
        } else {
            i + 1
        };
        loop {
            // `pat.find('-', k, j)`
            let found = (k..j).find(|&x| pat.get(x) == Some(&DASH));
            let Some(f) = found else { break };
            chunks.push(slice(i, f));
            i = f + 1;
            k = f + 3;
        }
        let chunk = slice(i, j);
        if chunk.is_empty() {
            if let Some(last) = chunks.last_mut() {
                last.push(DASH);
            }
        } else {
            chunks.push(chunk);
        }
        // Remove empty ranges -- invalid in RE.
        let mut k = chunks.len().saturating_sub(1);
        while k > 0 {
            let prev_last = chunks.get(k - 1).and_then(|c| c.last().copied());
            let first = chunks.get(k).and_then(|c| c.first().copied());
            if let (Some(a), Some(b)) = (prev_last, first)
                && a > b
            {
                let tail: Vec<u32> = chunks
                    .get(k)
                    .map(|c| c.get(1..).unwrap_or_default().to_vec())
                    .unwrap_or_default();
                if let Some(prev) = chunks.get_mut(k - 1) {
                    prev.pop();
                    prev.extend(tail);
                }
                chunks.remove(k);
            }
            k -= 1;
        }
        let mut out = Vec::new();
        for (n, c) in chunks.iter().enumerate() {
            if n > 0 {
                out.push(DASH);
            }
            out.extend(escape(c, true));
        }
        out
    } else {
        escape(&raw, false)
    };
    // Escape set operations (&&, ~~ and ||).
    let mut out = Vec::with_capacity(stuff.len());
    for c in stuff.drain(..) {
        if c == u32::from(b'&') || c == u32::from(b'~') || c == u32::from(b'|') {
            out.push(BS);
        }
        out.push(c);
    }
    out
}

/// The token for a set body: never, any, or the set `re` parses from
/// `[body]` (`sre_parse`'s class loop: an escape is its literal, `x-y` a
/// range, a `]` first a literal).
fn class_tok(stuff: &[u32]) -> Result<Tok, String> {
    if stuff.is_empty() {
        return Ok(Tok::Never);
    }
    if stuff == [BANG] {
        return Ok(Tok::Any);
    }
    let (negate, body): (bool, Vec<u32>) = match stuff.first() {
        Some(&BANG) => (true, stuff.get(1..).unwrap_or_default().to_vec()),
        Some(&c) if c == u32::from(b'^') || c == u32::from(b'[') => {
            let mut b = vec![BS];
            b.extend_from_slice(stuff);
            (false, b)
        }
        _ => (false, stuff.to_vec()),
    };
    // sre_parse: `[` (`^`) items `]`; the final `]` is the one translate adds.
    let mut src = body;
    src.push(CLOSE);
    let mut pos = 0usize;
    // One item: a code point, and whether it was escaped.
    let get = |pos: &mut usize| -> Option<(u32, bool)> {
        let c = *src.get(*pos)?;
        *pos += 1;
        if c == BS {
            let e = *src.get(*pos)?;
            *pos += 1;
            Some((e, true))
        } else {
            Some((c, false))
        }
    };
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    loop {
        let Some((this, escaped)) = get(&mut pos) else {
            return Err("unterminated character set".to_owned());
        };
        if this == CLOSE && !escaped && !ranges.is_empty() {
            break;
        }
        if src.get(pos) == Some(&DASH) {
            pos += 1;
            let Some((that, that_escaped)) = get(&mut pos) else {
                return Err("unterminated character set".to_owned());
            };
            if that == CLOSE && !that_escaped {
                ranges.push((this, this));
                ranges.push((DASH, DASH));
                break;
            }
            if that < this {
                return Err("bad character range".to_owned());
            }
            ranges.push((this, that));
        } else {
            ranges.push((this, this));
        }
    }
    if pos != src.len() {
        // translate never puts an unescaped `]` past a set's first item.
        return Err("unsupported character set".to_owned());
    }
    Ok(Tok::Set { negate, ranges })
}

/// A path's bytes as an `OsStr`.
#[must_use]
pub fn os_str(bytes: &[u8]) -> &OsStr {
    OsStr::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_repr_as_python() {
        for (x, s) in [
            (4100.0, "4100.0"),
            (1e16, "1e+16"),
            (1_234_567_890_123_456.0, "1234567890123456.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-2.5, "-2.5"),
            (0.0, "0.0"),
            (1.5e300, "1.5e+300"),
        ] {
            assert_eq!(py_float_repr(x), s, "{x}");
        }
    }

    #[test]
    fn globs_cover_the_basics() {
        let m = |n: &str, p: &str| fnmatchcase(n.as_bytes(), p.as_bytes()).unwrap();
        assert!(m("a/b.bundle", "*.bundle"));
        assert!(m("abc", "a?c"));
        assert!(!m("abc", "a?"));
        assert!(m("a\nb", "a*"));
        assert!(m("b", "[abc]"));
        assert!(!m("b", "[!abc]"));
        assert!(m("[", "["));
        assert!(m("", "*"));
        assert!(!m("", "?"));
    }
}
