// Ported from CPython 3.11 (PSF-2.0): Modules/_json.c, scan_once_unicode, _match_number_unicode, scanstring_unicode, _parse_array_unicode and _parse_object_unicode. See THIRD_PARTY_LICENSES.
//! A corpus index, read as `unitypy_dump.py` reads it:
//! `json.loads(fh.read().decode("utf-8"))`, Python 3.11's `json` module
//! (its C scanner) over the file's text, so every index the oracle reads is
//! read, to the same values, and every index it refuses is refused.
//!
//! - **Text.** The file must be UTF-8 (Python's strict decode, which
//!   refuses what [`std::str::from_utf8`] refuses). Whitespace is `' '`,
//!   `\t`, `\n` and `\r`; a byte-order mark is no value.
//! - **Numbers.** JSON's grammar. A number with a fraction or an exponent
//!   is a float, correctly rounded (`float()`, so `1e400` is `inf` and
//!   `1e-400` is `0.0`); any other is an integer of any size, kept as its
//!   digits, up to [`MAX_INT_DIGITS`] digits (`int()`'s default limit,
//!   past which Python raises). `NaN`, `Infinity` and `-Infinity` are
//!   floats.
//! - **Strings** are Python `str`s ([`PyStr`]): the escapes JSON has, a
//!   `\uXXXX` high surrogate followed by a `\uXXXX` low one joined into one
//!   code point and any other surrogate kept alone, and no raw control
//!   character (U+0000-U+001F).
//! - **Objects** are dicts: a repeated key keeps its first place and its
//!   last value.
//! - **Nesting.** Arrays and objects nest at most [`MAX_DEPTH`] deep, the
//!   deepest `json.loads` decodes where `collect_inputs` calls it before
//!   Python's recursion limit stops it (`RecursionError`).
//!
//! A refused index is the run's failure (exit status 1, no dump), as the
//! oracle's exception is. `tests/fixtures/dump/index.json` records the
//! oracle's run on each kind of text (`tools/fixtures/index.py`).

use std::collections::HashMap;

use super::py::PyStr;

/// The deepest nesting of arrays and objects `json.loads` decodes in the
/// oracle's `collect_inputs`: one more raises `RecursionError` (Python's
/// limit of 1,000 frames, less those below the call). Recorded from the
/// oracle's own run (`tests/fixtures/dump/index.json` "depth").
pub const MAX_DEPTH: usize = 993;

/// The most digits an integer may have (`sys.int_info.default_max_str_digits`):
/// `int()` refuses more, so `json.loads` raises `ValueError`.
pub const MAX_INT_DIGITS: usize = 4300;

/// A value as `json.loads` makes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `None`.
    Null,
    /// `True` or `False`.
    Bool(bool),
    /// An `int`: its digits as written, after a `-` for a negative one
    /// (`-0` is the integer 0).
    Int(String),
    /// A `float`.
    Float(f64),
    /// A `str`.
    Str(PyStr),
    /// A `list`.
    List(Vec<Value>),
    /// A `dict`: each key once, in the order of its first appearance, with
    /// its last value.
    Dict(Vec<(PyStr, Value)>),
}

const _: () = crate::send_sync::<Value>();

impl Value {
    /// `d.get(key)` on a dict; `None` for a key it lacks, or a value that
    /// is no dict.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Dict(members) => members
                .iter()
                .find(|(k, _)| k.as_bytes() == key.as_bytes())
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// Python's truthiness: `None`, `False`, a zero (`-0.0` included, not
    /// `nan`), and an empty `str`, `list` or `dict` are false.
    #[must_use]
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(digits) => digits.trim_start_matches('-') != "0",
            Self::Float(x) => *x != 0.0,
            Self::Str(s) => !s.is_empty(),
            Self::List(items) => !items.is_empty(),
            Self::Dict(members) => !members.is_empty(),
        }
    }
}

/// `json.loads(text.decode("utf-8"))`, or why Python refuses it.
pub fn parse(text: &[u8]) -> Result<Value, String> {
    let s = std::str::from_utf8(text)
        .map_err(|e| format!("the index is not UTF-8 ({e}), so Python cannot decode it"))?;
    let mut p = Parser {
        s: s.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.value(0)?;
    p.skip_ws();
    if p.pos != p.s.len() {
        return Err(p.err("Extra data"));
    }
    Ok(v)
}

struct Parser<'a> {
    /// Valid UTF-8.
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("{what}: byte {} of the index", self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn at(&self, k: usize) -> Option<u8> {
        self.s.get(self.pos + k).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value, String> {
        if self.s.get(self.pos..self.pos + word.len()) == Some(word.as_bytes()) {
            self.pos += word.len();
            Ok(v)
        } else {
            Err(self.err("Expecting value"))
        }
    }

    /// `scan_once` at `pos`, inside `depth` arrays and objects.
    fn value(&mut self, depth: usize) -> Result<Value, String> {
        match self.peek() {
            Some(b'"') => {
                self.pos += 1;
                self.string().map(Value::Str)
            }
            Some(open @ (b'[' | b'{')) => {
                if depth >= MAX_DEPTH {
                    let what = if open == b'[' { "array" } else { "object" };
                    return Err(self.err(&format!(
                        "maximum recursion depth exceeded while decoding a JSON {what} \
                         (Python decodes {MAX_DEPTH} levels here)"
                    )));
                }
                self.pos += 1;
                if open == b'[' {
                    self.array(depth + 1)
                } else {
                    self.object(depth + 1)
                }
            }
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'N') => self.literal("NaN", Value::Float(f64::NAN)),
            Some(b'I') => self.literal("Infinity", Value::Float(f64::INFINITY)),
            Some(b'-') if self.at(1) == Some(b'I') => {
                self.literal("-Infinity", Value::Float(f64::NEG_INFINITY))
            }
            _ => self.number(),
        }
    }

    /// `_match_number_unicode`.
    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        let digit = |c: Option<u8>| c.is_some_and(|c| c.is_ascii_digit());
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'1'..=b'9') => {
                while digit(self.peek()) {
                    self.pos += 1;
                }
            }
            Some(b'0') => self.pos += 1,
            _ => {
                self.pos = start;
                return Err(self.err("Expecting value"));
            }
        }
        let mut float = false;
        if self.peek() == Some(b'.') && digit(self.at(1)) {
            float = true;
            self.pos += 2;
            while digit(self.peek()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let e = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            let digits = self.pos;
            while digit(self.peek()) {
                self.pos += 1;
            }
            if self.pos > digits {
                float = true;
            } else {
                self.pos = e;
            }
        }
        let text = std::str::from_utf8(self.s.get(start..self.pos).unwrap_or_default())
            .map_err(|e| e.to_string())?;
        if float {
            // Rust's parse rounds correctly, as `float()` does, and saturates
            // to an infinity or a zero as it does.
            return text
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|e| format!("{text}: {e}"));
        }
        let digits = text.trim_start_matches('-').len();
        if digits > MAX_INT_DIGITS {
            self.pos = start;
            return Err(self.err(&format!(
                "Exceeds the limit ({MAX_INT_DIGITS} digits) for integer string conversion: \
                 value has {digits} digits"
            )));
        }
        Ok(Value::Int(text.to_owned()))
    }

    /// Four hex digits at `pos` (`\uXXXX`'s), any case.
    fn hex4(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        for k in 0..4 {
            let d = self
                .at(k)
                .and_then(|c| char::from(c).to_digit(16))
                .ok_or_else(|| self.err("Invalid \\uXXXX escape"))?;
            v = (v << 4) | d;
        }
        self.pos += 4;
        Ok(v)
    }

    /// `scanstring_unicode` from after the opening quote.
    fn string(&mut self) -> Result<PyStr, String> {
        let mut out = PyStr::default();
        loop {
            let run = self.pos;
            while let Some(c) = self.peek() {
                if c == b'"' || c == b'\\' {
                    break;
                }
                if c < 0x20 {
                    return Err(self.err("Invalid control character"));
                }
                self.pos += 1;
            }
            // A run of whole characters: it ends at an ASCII byte or the end.
            let text = std::str::from_utf8(self.s.get(run..self.pos).unwrap_or_default())
                .map_err(|e| e.to_string())?;
            out.push_str(text);
            match self.peek() {
                None => return Err(self.err("Unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                _ => {}
            }
            self.pos += 1; // the backslash
            let c = self.peek().ok_or_else(|| self.err("Unterminated string"))?;
            self.pos += 1;
            let cp = match c {
                b'"' | b'\\' | b'/' => u32::from(c),
                b'b' => 0x08,
                b'f' => 0x0C,
                b'n' => 0x0A,
                b'r' => 0x0D,
                b't' => 0x09,
                b'u' => {
                    let hi = self.hex4()?;
                    if (0xD800..0xDC00).contains(&hi)
                        && self.peek() == Some(b'\\')
                        && self.at(1) == Some(b'u')
                    {
                        let back = self.pos;
                        self.pos += 2;
                        let lo = self.hex4()?;
                        if (0xDC00..0xE000).contains(&lo) {
                            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                        } else {
                            // Not a pair: the second escape is read on its own.
                            self.pos = back;
                            hi
                        }
                    } else {
                        hi
                    }
                }
                _ => {
                    self.pos -= 2;
                    return Err(self.err("Invalid \\escape"));
                }
            };
            out.push(cp);
        }
    }

    /// `_parse_array_unicode` from after the `[`.
    fn array(&mut self, depth: usize) -> Result<Value, String> {
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::List(items));
        }
        loop {
            items.push(self.value(depth)?);
            self.skip_ws();
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::List(items));
                }
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }

    /// `_parse_object_unicode` from after the `{`.
    fn object(&mut self, depth: usize) -> Result<Value, String> {
        let mut members: Vec<(PyStr, Value)> = Vec::new();
        // Each key's place, once there are enough of them to search.
        let mut places: Option<HashMap<PyStr, usize>> = None;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Dict(members));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.err("Expecting property name enclosed in double quotes"));
            }
            self.pos += 1;
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("Expecting ':' delimiter"));
            }
            self.pos += 1;
            self.skip_ws();
            let v = self.value(depth)?;
            // `PyDict_SetItem`: a repeated key keeps its place, takes the value.
            if places.is_none() && members.len() >= 16 {
                places = Some(
                    members
                        .iter()
                        .enumerate()
                        .map(|(i, (k, _))| (k.clone(), i))
                        .collect(),
                );
            }
            let at = match &places {
                Some(map) => map.get(&key).copied(),
                None => members.iter().position(|(k, _)| *k == key),
            };
            if let Some(slot) = at.and_then(|i| members.get_mut(i)) {
                slot.1 = v;
            } else {
                if let Some(map) = &mut places {
                    map.insert(key.clone(), members.len());
                }
                members.push((key, v));
            }
            self.skip_ws();
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Dict(members));
                }
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::Str(PyStr::from(v))
    }

    #[test]
    fn values_read_as_json_loads_reads_them() {
        let ok = |t: &str| parse(t.as_bytes()).unwrap();
        assert_eq!(ok(" -0 "), Value::Int("-0".into()));
        assert!(matches!(ok("NaN"), Value::Float(x) if x.is_nan()));
        assert_eq!(ok("-Infinity"), Value::Float(f64::NEG_INFINITY));
        assert_eq!(ok("1e400"), Value::Float(f64::INFINITY));
        assert_eq!(ok("1E-400"), Value::Float(0.0));
        assert_eq!(
            ok(r#"{"a":1,"b":2,"a":3}"#),
            Value::Dict(vec![
                (PyStr::from("a"), Value::Int("3".into())),
                (PyStr::from("b"), Value::Int("2".into())),
            ])
        );
        assert_eq!(ok(r#""\ud83d\ude00""#), s("\u{1F600}"));
        let Value::Str(lone) = ok(r#""\ud800\u0041""#) else {
            panic!()
        };
        assert_eq!(lone.chars().collect::<Vec<_>>(), [0xD800, 0x41]);
        for bad in [
            "",
            "\u{FEFF}{}",
            "01",
            "1.",
            ".5",
            "+1",
            "[1,]",
            "{\"a\":1,}",
            "\"\t\"",
            "\"\\x\"",
            "\"\\u12\"",
            "nul",
            "-NaN",
            "[] x",
            "\"\\ud800\\uZZZZ\"",
        ] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert!(parse(b"\"\xff\"").is_err());
    }

    #[test]
    fn many_keys_keep_their_first_place_and_last_value() {
        // Past 16 keys the places are looked up in a map: a key repeated
        // before and after that point keeps its place and takes its last value.
        let text: Vec<String> = (0..40)
            .map(|i| (i, i))
            .chain([(3, 103), (30, 130), (3, 203)])
            .map(|(k, v)| format!("\"k{k}\":{v}"))
            .collect();
        let Value::Dict(m) = parse(format!("{{{}}}", text.join(",")).as_bytes()).unwrap() else {
            panic!()
        };
        assert_eq!(m.len(), 40);
        assert_eq!(m[3], (PyStr::from("k3"), Value::Int("203".into())));
        assert_eq!(m[30], (PyStr::from("k30"), Value::Int("130".into())));
        assert_eq!(m[39], (PyStr::from("k39"), Value::Int("39".into())));
    }
}
