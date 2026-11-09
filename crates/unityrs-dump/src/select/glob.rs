// Ported from CPython 3.11 (PSF-2.0): Lib/fnmatch.py, translate; Lib/re/_parser.py, _parse's character set. See THIRD_PARTY_LICENSES.
//! `--name`'s globs: Python 3.11's `fnmatch.fnmatchcase`, which
//! `fnmatch.translate` compiles to a regular expression, ported to a
//! wildcard matcher over code points (`tests/fixtures/dump/select.json`
//! "fnmatch" records Python's answers, invalid UTF-8 included).

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

#[cfg(test)]
mod tests {
    use super::*;

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
