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
//! common-string offset, `py_lower`, a registry key) allocate, and each is
//! made with [`RawStr::charged`], which charges the input's meter for its
//! buffer and its owner record and keeps the charge inside the buffer, so a
//! clone of it that outlives its row still counts against the input.
//!
//! Owner: the scaffold (implemented). Python string semantics on top of
//! these bytes (`str.lower`, `ntpath`) are in [`crate::pystr`].

use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use bytes::Bytes;

use crate::budget::{BudgetExceeded, charged_bytes};
use crate::limits::{ChargeError, Meter, heap_cost};

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

    /// A string the core computes from an input (a decimal common-string
    /// offset, a lower-cased name), charged to `meter` before it is
    /// allocated: its buffer at [`heap_cost`] and the owner record
    /// ([`crate::budget::OWNER_COST`]) that keeps the charge inside it, so
    /// every clone counts until the last one drops. An empty string is
    /// static and charges nothing.
    pub fn charged(bytes: &[u8], meter: &Arc<Meter>) -> Result<Self, ChargeError> {
        Self::charged_concat(&[bytes], meter)
    }

    /// The concatenation of `parts` (a resource candidate's stem and its
    /// suffix), computed and charged as [`Self::charged`] is: one buffer,
    /// charged before it is allocated.
    pub fn charged_concat(parts: &[&[u8]], meter: &Arc<Meter>) -> Result<Self, ChargeError> {
        let len = parts.iter().fold(0_usize, |n, p| n.saturating_add(p.len()));
        if len == 0 {
            return Ok(Self::default());
        }
        let n = u64::try_from(len).unwrap_or(u64::MAX);
        let charge = meter.charge(heap_cost(n))?;
        let mut buf = Vec::new();
        if buf.try_reserve_exact(len).is_err() {
            return Err(ChargeError::Budget(BudgetExceeded {
                requested: n,
                what: meter.reservation(),
            }));
        }
        for p in parts {
            buf.extend_from_slice(p);
        }
        Ok(Self(charged_bytes(buf, charge)?))
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
    fn a_computed_string_carries_its_charge() {
        use std::sync::Arc;

        use crate::budget::{OWNER_COST, Reservation, Unbounded};
        use crate::limits::{Meter, heap_cost};
        // Round 3: an unknown common-string offset's decimal string (and any
        // string the core computes) was allocated outside the meter.
        let m = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let s = RawStr::charged(b"2147483661", &m).unwrap();
        assert_eq!(s.as_bytes(), b"2147483661");
        assert_eq!(m.used(), heap_cost(10) + OWNER_COST);
        let kept = s.clone();
        drop(s);
        assert_eq!(m.used(), heap_cost(10) + OWNER_COST, "a clone keeps it");
        drop(kept);
        assert_eq!(m.used(), 0);
        let full = Meter::new(OWNER_COST, Arc::new(Unbounded), Reservation::Input);
        assert!(RawStr::charged(b"0123456789", &full).is_err());
        assert_eq!(full.used(), 0);
        assert_eq!(RawStr::charged(b"", &full).unwrap(), RawStr::default());
    }

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
        let names: Vec<RawStr> = (0..1000)
            .map(|_| RawStr::from_shared(buf.slice(..)))
            .collect();
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
