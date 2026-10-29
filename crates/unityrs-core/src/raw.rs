//! Raw strings: the bytes UnityPy holds as a `str` decoded with `surrogateescape`.
//!
//! Every string unityrs reads from a file is kept as bytes, never as `String`:
//! UnityPy decodes with `surrogateescape` (`streams/EndianBinaryReader.py:144`,
//! `:282`), so its `str` can hold any bytes, and the canonical dump writes a
//! name that is not valid UTF-8 as hex (`docs/CANONICAL_DUMP.md` section 4.2).
//!
//! **Zero-copy.** A `RawStr` is a [`Bytes`]: a string read from a file is a
//! slice of the file's buffer ([`RawStr::from_shared`]), never a copy, so a
//! typetree's node names (each row naming an offset in one string buffer),
//! metadata strings and `Value::Str` cost a handle each, whatever their
//! length and however often a hostile file repeats them (plan section 3.15).
//! Common strings are static. Only strings the core computes (a decimal
//! common-string offset, `py_lower`) allocate.
//!
//! Owner: the scaffold (implemented). Python string semantics on top of
//! these bytes (`str.lower`, `ntpath`) are in [`crate::pystr`].

use std::borrow::Borrow;
use std::fmt;

use bytes::Bytes;

/// Bytes UnityPy holds as a `str`. Valid UTF-8 or not.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RawStr(Bytes);

impl RawStr {
    /// Wraps bytes (a `Vec`, a boxed slice, a static slice or a shared buffer).
    #[must_use]
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self(bytes.into())
    }

    /// A slice of a file's shared buffer, kept without a copy.
    #[must_use]
    pub fn from_shared(bytes: Bytes) -> Self {
        Self(bytes)
    }

    /// A static string (a common string, a constant name).
    #[must_use]
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self(Bytes::from_static(bytes))
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

    /// The shared bytes, consuming the string.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
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
    /// Copies: a borrowed slice has no shared buffer to keep.
    fn from(b: &[u8]) -> Self {
        Self(Bytes::copy_from_slice(b))
    }
}

impl From<Vec<u8>> for RawStr {
    fn from(b: Vec<u8>) -> Self {
        Self(Bytes::from(b))
    }
}

impl From<&str> for RawStr {
    /// Copies (use [`RawStr::from_static`] for a constant).
    fn from(s: &str) -> Self {
        Self(Bytes::copy_from_slice(s.as_bytes()))
    }
}

impl From<Bytes> for RawStr {
    fn from(b: Bytes) -> Self {
        Self(b)
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
    fn a_string_read_from_a_buffer_shares_it() {
        // A thousand rows naming one 32 KiB string cost a handle each, not a copy.
        let buf = bytes::Bytes::from(vec![b'x'; 32 * 1024]);
        let names: Vec<RawStr> = (0..1000).map(|_| RawStr::from_shared(buf.slice(..))).collect();
        assert!(names.iter().all(|n| n.as_bytes().as_ptr() == buf.as_ptr()));
        assert_eq!(names[999].len(), 32 * 1024);
    }

    #[test]
    fn surrogate_encodings_are_invalid() {
        // ED A0 80 encodes U+D800: Python's strict decoder and Rust's agree it is invalid.
        assert_eq!(RawStr::from(&b"\xed\xa0\x80"[..]).to_str(), None);
        assert_eq!(RawStr::from("ok").to_str(), Some("ok"));
    }
}
