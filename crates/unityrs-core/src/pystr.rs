// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py, UnityPy/classes/PPtr.py, UnityPy/helpers/ResourceReader.py. See THIRD_PARTY_LICENSES.
//! Python string semantics on raw bytes, as UnityPy applies them to names.
//!
//! UnityPy compares CAB and resource names after `ntpath.basename` and
//! `str.lower()` (`environment.py:357-363`, `classes/PPtr.py:66-77`,
//! `helpers/ResourceReader.py:11-18`). Python lower-cases a surrogate-escaped
//! string: valid UTF-8 runs with Unicode full case mapping (Python 3.11 has
//! Unicode 14.0), invalid bytes passed through. Rust's tables are newer, so a
//! generated exception table ([`crate::tables::lower_exception`]) pins the code
//! points whose mapping changed after 14.0 (`tools/gen/`, package A; plan risk R7).
//! The `ntpath` functions behave as Python 3.11's do, which
//! `tools/fixtures/env.py` records (`tests/fixtures/d/pystr.json`).
//!
//! **Which bytes are characters.** A surrogate-escaped string is UTF-8
//! decoded with `surrogateescape`: every valid UTF-8 sequence is one
//! character, and every byte of an invalid sequence is a lone surrogate
//! (U+DC80-U+DCFF), which `lower()` keeps and the encoder turns back into
//! the byte. Python's decoder and [`str::Utf8Chunks`] split a byte string
//! into the same valid runs (both reject surrogates, overlong forms and code
//! points past U+10FFFF), so a run of [`str::Utf8Chunks`] is lowered as a
//! `str` and its invalid bytes are copied.
//!
//! **Final sigma.** `str.lower()` maps U+03A3 to U+03C2 in the
//! `Final_Sigma` context (`handle_capital_sigma` in Python's
//! `unicodeobject.c`): a cased character before it and none after it,
//! skipping case-ignorable ones. Rust's `str::to_lowercase` applies the same
//! rule with the same definitions, and a lone surrogate (an invalid byte) is
//! neither cased nor case-ignorable to Python, so the context never crosses
//! a run's edge and a run lowered on its own gets Python's answer. Rust's
//! `Cased` and `Case_Ignorable` tables are newer than Python's, which can
//! only matter for a character Unicode assigned after 14.0 beside a capital
//! sigma (`REQUESTS-d.md` asks for the 14.0 tables).
//!
//! **Memory.** Each function that returns a string computes it from an
//! input's name (a directory path, an external path, a resource path, up to
//! the input's size) and charges it to that input's meter before it is
//! allocated ([`RawStr::charged`]'s cost): a refusal allocates nothing. A
//! result equal to its input (a name already lower-case and without a
//! directory) is returned as the input's own bytes, uncopied, and a result
//! that is a part of its input (a basename already lower-case) as a slice of
//! them. Comparisons that only need to know whether two names are equal
//! ([`lower_eq`]) compare without building the lowered string at all. The
//! one transient buffer is Rust's lowering of a valid run that holds a
//! capital sigma, read for the sigma's form; it is charged while it lives.
//!
//! Owner: package D.

use std::sync::Arc;

use crate::budget::DecodeBuf;
use crate::error::LimitKind;
use crate::limits::{Charge, ChargeError, Meter, heap_cost};
use crate::raw::RawStr;
use crate::tables::lower_exception;

/// U+03A3 GREEK CAPITAL LETTER SIGMA, the one character `lower()` maps by
/// context.
const CAPITAL_SIGMA: char = '\u{3A3}';

/// Emits `s.lower()` piece by piece, in order, to `emit`, which returns
/// `false` to stop early. A piece is one lowered character or one run of
/// invalid bytes. Returns whether every piece was emitted.
fn lower_pieces(
    s: &[u8],
    meter: &Arc<Meter>,
    mut emit: impl FnMut(&[u8]) -> bool,
) -> Result<bool, ChargeError> {
    for chunk in s.utf8_chunks() {
        let run = chunk.valid();
        if !run.is_empty() && !lower_run(run, meter, &mut emit)? {
            return Ok(false);
        }
        let bad = chunk.invalid();
        // Lone surrogates: `lower()` keeps them, the encoder restores the bytes.
        if !bad.is_empty() && !emit(bad) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One valid run's lowering, character by character.
fn lower_run(
    run: &str,
    meter: &Arc<Meter>,
    emit: &mut impl FnMut(&[u8]) -> bool,
) -> Result<bool, ChargeError> {
    // Rust's lowering of the whole run, read only for each capital sigma's
    // context-dependent form (module docs), charged while it lives.
    let sigmas = if run.contains(CAPITAL_SIGMA) {
        Some(SigmaForms::of(run, meter)?)
    } else {
        None
    };
    // Where the current character's lowering starts in Rust's lowering.
    let mut rust_at = 0usize;
    let mut buf = [0u8; 12];
    for c in run.chars() {
        let piece: &[u8] = if c == CAPITAL_SIGMA {
            let form = sigmas
                .as_ref()
                .and_then(|s| s.lowered.get(rust_at..))
                .and_then(|t| t.chars().next())
                .unwrap_or('\u{3C3}');
            rust_at += form.len_utf8();
            form.encode_utf8(&mut buf).as_bytes()
        } else {
            rust_at += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            if let Some(py) = lower_exception(c) {
                py.as_bytes()
            } else {
                let mut n = 0;
                for l in c.to_lowercase() {
                    n += l.encode_utf8(buf.get_mut(n..).unwrap_or_default()).len();
                }
                buf.get(..n).unwrap_or_default()
            }
        };
        if !emit(piece) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Rust's lowering of a run that holds a capital sigma, with the charge for
/// the buffers `str::to_lowercase` allocates: it starts at the run's length
/// and grows at most once (a lowered run is at most 1.5 times as long), so
/// the first buffer and its doubling cover it.
struct SigmaForms {
    lowered: String,
    _charge: Charge,
}

impl SigmaForms {
    fn of(run: &str, meter: &Arc<Meter>) -> Result<Self, ChargeError> {
        let n = u64::try_from(run.len()).unwrap_or(u64::MAX);
        let charge = meter.charge(heap_cost(n).saturating_add(heap_cost(n.saturating_mul(2))))?;
        Ok(Self {
            lowered: run.to_lowercase(),
            _charge: charge,
        })
    }
}

/// `sub.lower()`, where `sub` lies inside `whole`: `sub`'s own bytes (a
/// zero-copy slice of `whole`) when lowering changes nothing, else a new
/// string charged to `meter` before it is allocated.
fn lower_within(whole: &RawStr, sub: &[u8], meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    let mut len = 0usize;
    let mut same = true;
    lower_pieces(sub, meter, |piece| {
        let end = len.saturating_add(piece.len());
        same = same && sub.get(len..end) == Some(piece);
        len = end;
        true
    })?;
    if same && len == sub.len() {
        return Ok(slice_of(whole, sub));
    }
    let mut out = DecodeBuf::new(
        meter,
        u64::try_from(len).unwrap_or(u64::MAX),
        u64::MAX,
        LimitKind::InputMemory,
    )?;
    let mut failed = None;
    lower_pieces(sub, meter, |piece| match out.extend_from_slice(piece) {
        Ok(()) => true,
        Err(e) => {
            failed = Some(e);
            false
        }
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    Ok(RawStr::from_shared(out.into_bytes()?))
}

/// `sub` as a zero-copy slice of `whole` (`sub` lies inside it).
fn slice_of(whole: &RawStr, sub: &[u8]) -> RawStr {
    if sub.is_empty() {
        return RawStr::default();
    }
    RawStr::from_shared(whole.clone().into_bytes().slice_ref(sub))
}

/// `str.lower()` on surrogate-escaped bytes, charged to `meter`.
pub fn py_lower(s: &RawStr, meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    lower_within(s, s.as_bytes(), meter)
}

/// Whether `a.lower() == lowered`, compared piece by piece without building
/// `a.lower()` (`classes/PPtr.py:75-76` compares each `files` key so). Only
/// a valid run that holds a capital sigma charges `meter`, while it is
/// compared (module docs).
pub fn lower_eq(a: &[u8], lowered: &[u8], meter: &Arc<Meter>) -> Result<bool, ChargeError> {
    let mut at = 0usize;
    let all = lower_pieces(a, meter, |piece| {
        let end = at.saturating_add(piece.len());
        let same = lowered.get(at..end) == Some(piece);
        at = end;
        same
    })?;
    Ok(all && at == lowered.len())
}

/// The byte length of the first Python character of `p`: a valid UTF-8
/// sequence, or one byte of an invalid one (a lone surrogate).
fn first_char_len(p: &[u8]) -> usize {
    p.utf8_chunks()
        .next()
        .map_or(0, |c| c.valid().chars().next().map_or(1, char::len_utf8))
}

/// `ntpath.splitdrive` as Python 3.11 splits: the drive or UNC share, and
/// the rest. Every rule but the drive letter's compares ASCII bytes at
/// ASCII positions, so it holds on bytes; the drive letter is any one
/// character followed by `:` (`normp[1:2] == colon` indexes characters).
fn splitdrive(p: &[u8]) -> (&[u8], &[u8]) {
    let is_sep = |b: &u8| *b == b'\\' || *b == b'/';
    // `normp[0:2] == sep * 2`: UNC drives and device drives.
    if p.first().is_some_and(is_sep) && p.get(1).is_some_and(is_sep) {
        // `normp[:8].upper() == '\\\\?\\UNC\\'`: only ASCII `u`, `n` and `c`
        // upper-case to the prefix's letters (probed on Python 3.11 over
        // every code point), so the first eight bytes decide it.
        let unc = p.get(..8).is_some_and(|head| {
            head.iter().zip(b"\\\\?\\UNC\\").all(|(&b, &want)| {
                let b = if b == b'/' { b'\\' } else { b };
                b.to_ascii_uppercase() == want
            })
        });
        let start = if unc { 8 } else { 2 };
        let find = |from: usize| {
            p.get(from..)
                .and_then(|t| t.iter().position(is_sep))
                .map(|i| i + from)
        };
        let Some(index) = find(start) else {
            return (p, &[]);
        };
        let Some(index2) = find(index + 1) else {
            return (p, &[]);
        };
        return p.split_at(index2);
    }
    // `normp[1:2] == colon`: the second character.
    let first = first_char_len(p);
    if first > 0 && p.get(first) == Some(&b':') {
        return p.split_at(first + 1);
    }
    (&[], p)
}

/// `ntpath.basename`: after `splitdrive`, the part after the last `/` or `\`.
#[must_use]
pub fn ntpath_basename(p: &[u8]) -> &[u8] {
    let (_, rest) = splitdrive(p);
    let start = rest
        .iter()
        .rposition(|&b| b == b'\\' || b == b'/')
        .map_or(0, |i| i + 1);
    rest.get(start..).unwrap_or_default()
}

/// `ntpath.splitext`: `(root, ext)`, leading dots not counting as an extension.
#[must_use]
pub fn ntpath_splitext(p: &[u8]) -> (&[u8], &[u8]) {
    // `genericpath._splitext(p, '\\', '/', '.')`
    let sep = p.iter().rposition(|&b| b == b'\\' || b == b'/');
    let Some(dot) = p.iter().rposition(|&b| b == b'.') else {
        return (p, &[]);
    };
    if sep.is_none_or(|s| dot > s) {
        let name = sep.map_or(0, |s| s + 1);
        // Leading dots are part of the name, not an extension.
        if p.get(name..dot)
            .is_some_and(|lead| lead.iter().any(|&b| b != b'.'))
        {
            return p.split_at(dot);
        }
    }
    (p, &[])
}

/// `simplify_name`: `ntpath.basename(name).lower()`, the CAB registry's key,
/// charged to `meter`.
pub fn simplify_name(name: &RawStr, meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    // UnityPy environment.py:357-363
    lower_within(name, ntpath_basename(name.as_bytes()), meter)
}

/// `PPtr.deref`'s external path cleaning: strip a leading `archive:/`, then a
/// leading `assets/`, keep what follows the last `/`, lower-case it; charged
/// to `meter`.
pub fn clean_external_path(path: &RawStr, meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    // UnityPy classes/PPtr.py:68-73
    let mut p = path.as_bytes();
    if let Some(rest) = p.strip_prefix(b"archive:/") {
        p = rest;
    }
    if let Some(rest) = p.strip_prefix(b"assets/") {
        p = rest;
    }
    // `rsplit("/")[-1]`
    let start = p.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    lower_within(path, p.get(start..).unwrap_or_default(), meter)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        clean_external_path, lower_eq, ntpath_basename, ntpath_splitext, py_lower, simplify_name,
        splitdrive,
    };
    use crate::budget::{OWNER_COST, Reservation, Unbounded};
    use crate::limits::{Meter, heap_cost};
    use crate::raw::RawStr;

    fn meter() -> Arc<Meter> {
        Meter::new(1 << 30, Arc::new(Unbounded), Reservation::Input)
    }

    fn lower(s: &[u8]) -> Vec<u8> {
        py_lower(&RawStr::from(s), &meter())
            .unwrap()
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn an_unchanged_name_is_its_own_bytes_and_charges_nothing() {
        let m = meter();
        let name = RawStr::from_shared(bytes::Bytes::from(b"dir/cab-0123".to_vec()));
        let key = simplify_name(&name, &m).unwrap();
        assert_eq!(key.as_bytes(), b"cab-0123");
        assert_eq!(
            key.as_bytes().as_ptr(),
            name.as_bytes()[4..].as_ptr(),
            "a slice of the name"
        );
        let same = py_lower(&name, &m).unwrap();
        assert_eq!(same.as_bytes().as_ptr(), name.as_bytes().as_ptr());
        assert_eq!(m.used(), 0);
    }

    #[test]
    fn a_lowered_name_is_charged_and_returns_its_charge() {
        let m = meter();
        let key = simplify_name(&RawStr::from("a\\CAB-ABC"), &m).unwrap();
        assert_eq!(key.as_bytes(), b"cab-abc");
        assert_eq!(m.used(), heap_cost(7) + OWNER_COST);
        drop(key);
        assert_eq!(m.used(), 0);
        let full = Meter::new(OWNER_COST, Arc::new(Unbounded), Reservation::Input);
        assert!(py_lower(&RawStr::from("ABC"), &full).is_err());
        assert_eq!(full.used(), 0);
    }

    #[test]
    fn invalid_bytes_pass_through_and_valid_runs_lower() {
        assert_eq!(
            lower(b"A\xffB\xc3\x84\xed\xa0\x80Z"),
            b"a\xffb\xc3\xa4\xed\xa0\x80z"
        );
        // U+0130 lowers to two characters, as Python's full mapping does.
        assert_eq!(lower("\u{130}".as_bytes()), "i\u{307}".as_bytes());
    }

    #[test]
    fn a_final_sigma_lowers_by_context() {
        // Python 3.11 (tests/fixtures/d/pystr.json records more): 'ΑΣ' ->
        // 'ας', 'ΑΣΑ' -> 'ασα', 'Σ' -> 'σ', 'ΑΣ.txt' -> 'ασ.txt' ('.' is
        // case-ignorable, 't' cased), and a lone surrogate ends the word.
        assert_eq!(lower("ΑΣ".as_bytes()), "ας".as_bytes());
        assert_eq!(lower("ΑΣΑ".as_bytes()), "ασα".as_bytes());
        assert_eq!(lower("Σ".as_bytes()), "σ".as_bytes());
        assert_eq!(lower("ΑΣ.txt".as_bytes()), "ασ.txt".as_bytes());
        assert_eq!(lower("ΑΣ.".as_bytes()), "ας.".as_bytes());
        let mut b = "ΑΣ".as_bytes().to_vec();
        b.push(0xff);
        b.extend_from_slice("Α".as_bytes());
        let mut want = "ας".as_bytes().to_vec();
        want.push(0xff);
        want.extend_from_slice("α".as_bytes());
        assert_eq!(lower(&b), want);
        // The transient lowering a sigma run needs is charged and returned.
        let m = meter();
        let s = py_lower(&RawStr::from("ΑΣ"), &m).unwrap();
        assert_eq!(m.used(), heap_cost(4) + OWNER_COST);
        drop(s);
        assert_eq!(m.used(), 0);
    }

    #[test]
    fn lower_eq_compares_without_building() {
        let m = meter();
        assert!(lower_eq(b"CAB-Abc", b"cab-abc", &m).unwrap());
        assert!(!lower_eq(b"CAB-Abc", b"cab-ab", &m).unwrap());
        assert!(!lower_eq(b"CAB-Ab", b"cab-abc", &m).unwrap());
        assert!(lower_eq("ΑΣ".as_bytes(), "ας".as_bytes(), &m).unwrap());
        assert!(!lower_eq("ΑΣ".as_bytes(), "ασ".as_bytes(), &m).unwrap());
        assert!(lower_eq(b"", b"", &m).unwrap());
        assert_eq!(m.used(), 0);
    }

    #[test]
    fn splitdrive_and_basename_follow_ntpath() {
        let sd = |p: &str| {
            let (a, b) = splitdrive(p.as_bytes());
            (
                String::from_utf8(a.to_vec()).unwrap(),
                String::from_utf8(b.to_vec()).unwrap(),
            )
        };
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        // Probed on Python 3.11.17.
        assert_eq!(sd("c:x/y"), s("c:", "x/y"));
        assert_eq!(sd("//a/b/c"), s("//a/b", "/c"));
        assert_eq!(sd("//a/b"), s("//a/b", ""));
        assert_eq!(sd("///a/b"), s("///a", "/b"));
        assert_eq!(sd("//"), s("//", ""));
        assert_eq!(sd("é:foo"), s("é:", "foo"));
        assert_eq!(sd(":"), s("", ":"));
        assert_eq!(sd("\\\\?\\UNC\\s\\sh\\x"), s("\\\\?\\UNC\\s\\sh", "\\x"));
        assert_eq!(sd("//?/unc/s/sh/x"), s("//?/unc/s/sh", "/x"));
        assert_eq!(ntpath_basename(b"a/b\\c"), b"c");
        assert_eq!(ntpath_basename(b"c:x"), b"x");
        assert_eq!(ntpath_basename(b"//srv/share"), b"");
        assert_eq!(ntpath_basename(b"dir/"), b"");
    }

    #[test]
    fn splitext_keeps_leading_dots() {
        assert_eq!(ntpath_splitext(b"a.b.c"), (&b"a.b"[..], &b".c"[..]));
        assert_eq!(ntpath_splitext(b".resS"), (&b".resS"[..], &b""[..]));
        assert_eq!(ntpath_splitext(b"..x"), (&b"..x"[..], &b""[..]));
        assert_eq!(ntpath_splitext(b"..x.y"), (&b"..x"[..], &b".y"[..]));
        assert_eq!(ntpath_splitext(b"a.d/b"), (&b"a.d/b"[..], &b""[..]));
        assert_eq!(ntpath_splitext(b"x."), (&b"x"[..], &b"."[..]));
    }

    #[test]
    fn an_external_path_is_cleaned_as_pptr_does() {
        let m = meter();
        let c = |p: &str| clean_external_path(&RawStr::from(p), &m).unwrap();
        assert_eq!(c("archive:/CAB-X/CAB-Y").as_bytes(), b"cab-y");
        assert_eq!(c("assets/Foo/Bar.asset").as_bytes(), b"bar.asset");
        assert_eq!(c("archive:/assets/x").as_bytes(), b"x");
        assert_eq!(c("Library/a\\B").as_bytes(), b"a\\b");
        assert_eq!(c("").as_bytes(), b"");
    }
}
