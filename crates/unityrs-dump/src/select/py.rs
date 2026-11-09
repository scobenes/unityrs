//! Python `str`s, and Python's `str()` and `repr()` of the values an index
//! holds, as `unitypy_dump.py` compares and prints them: `--kind` and
//! `--build` against an entry's `kind` and `str(build)`, an entry's `path`
//! as the bytes `os.fsencode` gives, and an index pin's `str()` in the
//! mismatch message. `tests/fixtures/dump/index.json` records Python's
//! answers (`tools/fixtures/index.py`).

use std::fmt::Write as _;

use super::index::Value;
use super::printable::is_printable;

/// A Python `str`: a sequence of code points, any of U+0000-U+10FFFF, lone
/// surrogates included (a `\udcff` escape in an index, an argument byte
/// that is not UTF-8). Held as its UTF-8, a surrogate written as the three
/// bytes Python's `surrogatepass` writes, so two are equal exactly when
/// their code points are, and a `str` with no surrogate is held as its
/// plain UTF-8.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PyStr(Vec<u8>);

const _: () = crate::send_sync::<PyStr>();

impl PyStr {
    /// `bytes.decode("utf-8", "surrogateescape")`, as Python reads `argv`
    /// and file names: each byte of an invalid sequence is U+DC00 plus it.
    #[must_use]
    pub fn from_os_bytes(bytes: &[u8]) -> Self {
        let mut s = Self(Vec::with_capacity(bytes.len()));
        for chunk in bytes.utf8_chunks() {
            s.0.extend_from_slice(chunk.valid().as_bytes());
            for b in chunk.invalid() {
                s.push(0xDC00 + u32::from(*b));
            }
        }
        s
    }

    /// `self.encode("utf-8", "surrogateescape")`, as `os.fsencode` gives a
    /// path: U+DC80-U+DCFF are the bytes 0x80-0xFF again, and `None` for
    /// any other surrogate, which Python refuses (`UnicodeEncodeError`).
    #[must_use]
    pub fn to_os_bytes(&self) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(self.0.len());
        for cp in self.chars() {
            match cp {
                0xDC80..=0xDCFF => out.push(u8::try_from(cp - 0xDC00).ok()?),
                0xD800..=0xDFFF => return None,
                _ => encode(cp, &mut out),
            }
        }
        Some(out)
    }

    /// Its UTF-8, a surrogate as `surrogatepass` writes it.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Whether it holds no code point.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Appends the code point `cp` (at most U+10FFFF).
    pub fn push(&mut self, cp: u32) {
        encode(cp, &mut self.0);
    }

    /// Appends `s`.
    pub fn push_str(&mut self, s: &str) {
        self.0.extend_from_slice(s.as_bytes());
    }

    /// Its code points.
    pub fn chars(&self) -> impl Iterator<Item = u32> + '_ {
        // Only `push` and valid UTF-8 build one, so every lead byte is
        // followed by its continuation bytes.
        let mut rest = self.0.as_slice();
        std::iter::from_fn(move || {
            let (&lead, tail) = rest.split_first()?;
            let (len, bits) = match lead {
                0x00..=0x7F => (0, u32::from(lead)),
                0xC0..=0xDF => (1, u32::from(lead & 0x1F)),
                0xE0..=0xEF => (2, u32::from(lead & 0x0F)),
                _ => (3, u32::from(lead & 0x07)),
            };
            let (cont, after) = tail.split_at(len.min(tail.len()));
            rest = after;
            Some(
                cont.iter()
                    .fold(bits, |cp, b| (cp << 6) | u32::from(b & 0x3F)),
            )
        })
    }
}

impl From<&str> for PyStr {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

/// As Python's `print` writes it to `stderr`, whose error handler is
/// `backslashreplace`: a lone surrogate as `\udcXX`, the rest as UTF-8.
impl std::fmt::Display for PyStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for cp in self.chars() {
            match char::from_u32(cp) {
                Some(c) => f.write_char(c)?,
                None => write!(f, "\\u{cp:04x}")?,
            }
        }
        Ok(())
    }
}

/// `cp`'s UTF-8 (a surrogate's three bytes as `surrogatepass` writes them).
fn encode(cp: u32, out: &mut Vec<u8>) {
    // Each shift leaves at most 6 bits under the mask, so the casts are exact.
    #[allow(clippy::cast_possible_truncation)]
    let byte = |v: u32| v as u8;
    match cp {
        0..=0x7F => out.push(byte(cp)),
        0x80..=0x7FF => out.extend([0xC0 | byte(cp >> 6), 0x80 | byte(cp & 0x3F)]),
        0x800..=0xFFFF => out.extend([
            0xE0 | byte(cp >> 12),
            0x80 | byte((cp >> 6) & 0x3F),
            0x80 | byte(cp & 0x3F),
        ]),
        _ => out.extend([
            0xF0 | byte((cp >> 18) & 0x07),
            0x80 | byte((cp >> 12) & 0x3F),
            0x80 | byte((cp >> 6) & 0x3F),
            0x80 | byte(cp & 0x3F),
        ]),
    }
}

/// Python's `str()` of a value `json.loads` made (`str(e.get("build"))`,
/// and an index pin in the oracle's message): `None`, `True` and `False`;
/// an integer's digits, any length (`-0` is the integer 0); a float's
/// `repr`; a string itself; a list's or a dict's `repr`, each item's `repr`
/// between `", "`, a dict's keys before `": "`.
#[must_use]
pub fn py_str(v: &Value) -> PyStr {
    match v {
        Value::Str(s) => s.clone(),
        other => {
            let mut out = PyStr::default();
            repr(other, &mut out);
            out
        }
    }
}

/// Python's `repr()` of `v`, appended to `out`. The recursion is as deep as
/// the value, which [`super::index::MAX_DEPTH`] bounds.
fn repr(v: &Value, out: &mut PyStr) {
    match v {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Int(digits) => out.push_str(if digits == "-0" { "0" } else { digits }),
        Value::Float(x) => out.push_str(&py_float_repr(*x)),
        Value::Str(s) => str_repr(s, out),
        Value::List(items) => {
            out.push_str("[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                repr(item, out);
            }
            out.push_str("]");
        }
        Value::Dict(members) => {
            out.push_str("{");
            for (i, (k, item)) in members.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                str_repr(k, out);
                out.push_str(": ");
                repr(item, out);
            }
            out.push_str("}");
        }
    }
}

/// `repr(str)` (`unicode_repr` in `CPython` 3.11): single quotes unless the
/// string holds a `'` and no `"`; a backslash before the quote and `\`;
/// `\t`, `\n`, `\r`; `\xhh` for the other ASCII controls and DEL; past
/// ASCII, what `str.isprintable()` refuses as `\xhh`, `\uhhhh` or
/// `\Uhhhhhhhh`, lone surrogates included, and the rest as it is.
fn str_repr(s: &PyStr, out: &mut PyStr) {
    let has = |q: u32| s.chars().any(|c| c == q);
    let quote = if has(u32::from(b'\'')) && !has(u32::from(b'"')) {
        u32::from(b'"')
    } else {
        u32::from(b'\'')
    };
    out.push(quote);
    let mut esc = String::new();
    for c in s.chars() {
        esc.clear();
        match c {
            _ if c == quote || c == u32::from(b'\\') => {
                out.push(u32::from(b'\\'));
                out.push(c);
                continue;
            }
            0x09 => esc.push_str("\\t"),
            0x0A => esc.push_str("\\n"),
            0x0D => esc.push_str("\\r"),
            0x00..=0x1F | 0x7F => {
                let _ = write!(esc, "\\x{c:02x}");
            }
            0x20..=0x7E => {
                out.push(c);
                continue;
            }
            _ if is_printable(c) => {
                out.push(c);
                continue;
            }
            0x80..=0xFF => {
                let _ = write!(esc, "\\x{c:02x}");
            }
            0x100..=0xFFFF => {
                let _ = write!(esc, "\\u{c:04x}");
            }
            _ => {
                let _ = write!(esc, "\\U{c:08x}");
            }
        }
        out.push_str(&esc);
    }
    out.push(quote);
}

/// `repr(float)`: the shortest digits that round-trip, positional when the
/// decimal exponent is in -4..16, else `d.ddde+XX` (`PyOS_double_to_string`
/// with mode `'r'`); `nan`, `inf` and `-inf`.
///
/// Of the shortest digits that round-trip, Python's `repr` (David Gay's
/// `dtoa` in mode 0) gives those nearest the double, and of two equally
/// near the even one. Rust's shortest `{:e}` breaks that tie upwards
/// (`1551563132437320.25`: Python `…320.2`, `{:e}` `…320.3`; review round
/// 8), so its length only is taken: the double rounded to that many digits
/// (`{:.*e}`: exact, ties to even) is the nearest of that length, and is
/// Python's when it round-trips. When it does not (a power of two, whose
/// doubles that round-trip reach half as far below it as above, can have
/// its nearest digits below them), `{:e}`'s digits are kept: they are then
/// the nearest that round-trip, as Python's are, with no tie between two
/// (`tests/fixtures/dump/index.json` "str" holds ties and such powers).
#[must_use]
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    // `{:e}` gives the shortest round-trip digits: "-1.2345e-7".
    let mut sci = format!("{x:e}");
    let len = sci
        .split('e')
        .next()
        .map_or(0, |m| m.bytes().filter(u8::is_ascii_digit).count());
    if len > 1 {
        let nearest = format!("{:.*e}", len - 1, x);
        if nearest.parse::<f64>() == Ok(x) {
            sci = nearest;
        }
    }
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
            (-0.0, "-0.0"),
            (1.5e300, "1.5e+300"),
        ] {
            assert_eq!(py_float_repr(x), s, "{x}");
        }
    }

    #[test]
    fn a_str_goes_to_bytes_and_back_as_surrogateescape_has_it() {
        for raw in [
            &b""[..],
            b"abc",
            b"\xff",
            b"a\xc3",
            "é\u{1F600}".as_bytes(),
            b"\xed\xa0\x80",
        ] {
            let s = PyStr::from_os_bytes(raw);
            assert_eq!(s.to_os_bytes().as_deref(), Some(raw), "{raw:?}");
        }
        let mut lone = PyStr::default();
        lone.push(0xD800);
        assert_eq!(lone.to_os_bytes(), None);
        assert_eq!(lone.chars().collect::<Vec<_>>(), [0xD800]);
        assert_eq!(PyStr::from_os_bytes(b"a\xff").to_string(), "a\\udcff");
    }
}
