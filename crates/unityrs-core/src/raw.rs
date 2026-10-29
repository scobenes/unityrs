//! Raw strings: the bytes UnityPy holds as a `str` decoded with `surrogateescape`.
//!
//! Every string unityrs reads from a file is kept as bytes, never as `String`:
//! UnityPy decodes with `surrogateescape` (`streams/EndianBinaryReader.py:144`,
//! `:282`), so its `str` can hold any bytes, and the canonical dump writes a
//! name that is not valid UTF-8 as hex (`docs/CANONICAL_DUMP.md` section 4.2).
//!
//! Owner: the scaffold (implemented). Python string semantics on top of
//! these bytes (`str.lower`, `ntpath`) are in [`crate::pystr`].

use std::borrow::Borrow;
use std::fmt;

/// Bytes UnityPy holds as a `str`. Valid UTF-8 or not.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RawStr(Box<[u8]>);

impl RawStr {
    /// Wraps bytes.
    #[must_use]
    pub fn new(bytes: impl Into<Box<[u8]>>) -> Self {
        Self(bytes.into())
    }

    /// The bytes, as read.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The string when the bytes are strict UTF-8 (overlong forms and encoded
    /// surrogates are invalid, as `CANONICAL_DUMP.md` section 3 defines it).
    #[must_use]
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The bytes, consuming the string.
    #[must_use]
    pub fn into_bytes(self) -> Box<[u8]> {
        self.0
    }
}

impl fmt::Debug for RawStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.0.escape_ascii())
    }
}

impl fmt::Display for RawStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.escape_ascii())
    }
}

impl AsRef<[u8]> for RawStr {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Borrow<[u8]> for RawStr {
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for RawStr {
    fn from(b: &[u8]) -> Self {
        Self(b.into())
    }
}

impl From<Vec<u8>> for RawStr {
    fn from(b: Vec<u8>) -> Self {
        Self(b.into_boxed_slice())
    }
}

impl From<&str> for RawStr {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().into())
    }
}

#[cfg(test)]
mod tests {
    use super::RawStr;

    #[test]
    fn keeps_invalid_utf8() {
        let s = RawStr::from(&b"a\xffb"[..]);
        assert_eq!(s.as_bytes(), b"a\xffb");
        assert_eq!(s.to_str(), None);
        assert_eq!(format!("{s:?}"), "\"a\\xffb\"");
    }

    #[test]
    fn surrogate_encodings_are_invalid() {
        // ED A0 80 encodes U+D800: Python's strict decoder and Rust's agree it is invalid.
        assert_eq!(RawStr::from(&b"\xed\xa0\x80"[..]).to_str(), None);
        assert_eq!(RawStr::from("ok").to_str(), Some("ok"));
    }
}
