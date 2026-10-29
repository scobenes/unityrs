//! The canonical JSON layer (docs/CANONICAL_DUMP.md section 2).
//!
//! Records are small trees built from [`J`]; typetree values are long, so the
//! value encoder writes their text straight into the line with [`write_str`]
//! and splices it in as [`J::Raw`].

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// Raw-byte payloads above this many bytes are summarised as length + sha256.
pub const INLINE_MAX: usize = 64;

/// A JSON value with members in insertion order.
#[derive(Clone)]
pub enum J {
    Null,
    Bool(bool),
    I(i64),
    U(u64),
    S(String),
    /// Already-canonical JSON text.
    Raw(String),
    A(Vec<J>),
    O(Vec<(&'static str, J)>),
}

impl J {
    pub fn s(s: &str) -> J {
        J::S(s.to_owned())
    }

    pub fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> J) -> J {
        v.map_or(J::Null, f)
    }

    pub fn write(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::I(i) => {
                let _ = write!(out, "{i}");
            }
            J::U(u) => {
                let _ = write!(out, "{u}");
            }
            J::S(s) => write_str(out, s),
            J::Raw(r) => out.push_str(r),
            J::A(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            J::O(members) => {
                out.push('{');
                for (i, (k, v)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(out, k);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn line(&self) -> String {
        let mut out = String::with_capacity(256);
        self.write(&mut out);
        out.push('\n');
        out
    }
}

/// A JSON string exactly as Python's `json.dumps(ensure_ascii=False)` writes it.
pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    let mut start = 0;
    for (i, b) in s.bytes().enumerate() {
        let esc: &str = match b {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0x08 => "\\b",
            0x0c => "\\f",
            0..0x20 => "",
            _ => continue,
        };
        out.push_str(&s[start..i]);
        if esc.is_empty() {
            let _ = write!(out, "\\u{b:04x}");
        } else {
            out.push_str(esc);
        }
        start = i + 1;
    }
    out.push_str(&s[start..]);
    out.push('"');
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The raw-bytes rule: inline hex up to [`INLINE_MAX`] bytes, else length + sha256.
pub fn write_raw(out: &mut String, kind: &str, bytes: &[u8]) {
    if bytes.len() <= INLINE_MAX {
        let _ = write!(out, "{{\"t\":\"{kind}\",\"hex\":\"{}\"}}", hex(bytes));
    } else {
        let _ = write!(
            out,
            "{{\"t\":\"{kind}\",\"len\":{},\"sha256\":\"{}\"}}",
            bytes.len(),
            sha256_hex(bytes)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_like_python() {
        let mut out = String::new();
        write_str(&mut out, "a\"\\\n\r\t\u{8}\u{c}\u{1}\u{1f}/\u{7f}\u{2028}é");
        assert_eq!(
            out,
            "\"a\\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u001f/\u{7f}\u{2028}é\""
        );
    }

    #[test]
    fn raw_rule_switches_at_64_bytes() {
        let mut a = String::new();
        write_raw(&mut a, "bytes", &[0xab; 64]);
        assert!(a.starts_with("{\"t\":\"bytes\",\"hex\":\"abab"));
        let mut b = String::new();
        write_raw(&mut b, "bytes", &[0xab; 65]);
        assert!(b.starts_with("{\"t\":\"bytes\",\"len\":65,\"sha256\":\""));
    }
}
