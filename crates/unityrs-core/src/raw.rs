// Ported from UnityPy 1.25.2 (MIT): UnityPy/streams/EndianBinaryReader.py. See THIRD_PARTY_LICENSES.
//! Raw strings: the bytes UnityPy holds as a `str` decoded with `surrogateescape`.
//!
//! Every string unityrs reads from a file is kept as bytes, never as `String`:
//! UnityPy decodes with `surrogateescape` (`streams/EndianBinaryReader.py:144`,
//! `:282`), so its `str` can hold any bytes, and the canonical dump writes a
//! name that is not valid UTF-8 as hex (`docs/CANONICAL_DUMP.md` section 4.2).
//!
//! **Zero-copy.** An input `RawStr` borrows [`Bytes`]: a string read from a file is a
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

use crate::budget::OWNER_COST;
use crate::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost, reserve_exact};

/// Bytes UnityPy holds as a `str`. Valid UTF-8 or not; computed strings use
/// fallible shared storage, while input strings borrow their `Bytes`.
#[derive(Clone)]
pub struct RawStr(Storage);

#[derive(Clone)]
enum Storage {
    Bytes(Bytes),
    Computed {
        owner: triomphe::Arc<Computed>,
        start: usize,
        end: usize,
    },
}
struct Computed {
    bytes: Vec<u8>,
    _charge: Charge,
}

impl RawStr {
    /// Wraps bytes (a `Vec`, a boxed slice, a static slice or a shared buffer).
    #[must_use]
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self(Storage::Bytes(bytes.into()))
    }

    /// A slice of a file's shared buffer, kept without a copy.
    #[must_use]
    pub fn from_shared(bytes: Bytes) -> Self {
        Self(Storage::Bytes(bytes))
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
        reserve_exact(&mut buf, len, n, meter).map_err(ChargeError::Budget)?;
        for p in parts {
            buf.extend_from_slice(p);
        }
        Self::from_metered(MeteredVec::from_charged_parts(buf, charge)?, meter)
    }

    /// Keep computed bytes and their charge in a fallibly allocated shared
    /// owner. Cloning or slicing the resulting name never allocates.
    pub(crate) fn from_metered(
        bytes: MeteredVec<u8>,
        meter: &Arc<Meter>,
    ) -> Result<Self, ChargeError> {
        let (bytes, mut charge) = bytes.into_parts();
        let end = bytes.len();
        charge.grow(OWNER_COST)?;
        let owner = triomphe::Arc::try_new(Computed {
            bytes,
            _charge: charge,
        })
        .map_err(|_| ChargeError::Allocator(meter.allocator_refused(OWNER_COST)))?;
        Ok(Self(Storage::Computed {
            owner,
            start: 0,
            end,
        }))
    }

    /// A borrowed subrange of this name, preserving either shared owner.
    pub(crate) fn slice_ref(&self, sub: &[u8]) -> Self {
        let start = (sub.as_ptr() as usize).saturating_sub(self.as_bytes().as_ptr() as usize);
        let end = start.saturating_add(sub.len());
        match &self.0 {
            Storage::Bytes(bytes) => Self::from_shared(bytes.slice(start..end)),
            Storage::Computed {
                owner, start: base, ..
            } => Self(Storage::Computed {
                owner: owner.clone(),
                start: base + start,
                end: base + end,
            }),
        }
    }

    /// A static string (a common string, a constant name).
    #[must_use]
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self(Storage::Bytes(Bytes::from_static(bytes)))
    }

    /// The bytes, as read.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            Storage::Bytes(bytes) => bytes,
            Storage::Computed { owner, start, end } => {
                owner.bytes.get(*start..*end).unwrap_or_default()
            }
        }
    }

    /// The string when the bytes are strict UTF-8 (overlong forms and encoded
    /// surrogates are invalid, as `CANONICAL_DUMP.md` section 3 defines it).
    #[must_use]
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(self.as_bytes()).ok()
    }

    /// Length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Whether the string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    /// The shared bytes, consuming the string. Computed strings need a
    /// `Bytes` owner adapter here; name lookups and slices avoid that adapter.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        match self.0 {
            Storage::Bytes(bytes) => bytes,
            computed @ Storage::Computed { .. } => Bytes::from_owner(Self(computed)),
        }
    }
}

impl Default for RawStr {
    fn default() -> Self {
        Self::from_static(b"")
    }
}
impl PartialEq for RawStr {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}
impl Eq for RawStr {}
impl std::hash::Hash for RawStr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}
impl PartialOrd for RawStr {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for RawStr {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl fmt::Debug for RawStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.as_bytes().escape_ascii())
    }
}

impl fmt::Display for RawStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_bytes().escape_ascii())
    }
}

impl AsRef<[u8]> for RawStr {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Borrow<[u8]> for RawStr {
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl From<&[u8]> for RawStr {
    /// Copies: a borrowed slice has no shared buffer to keep.
    fn from(b: &[u8]) -> Self {
        Self::from_shared(Bytes::copy_from_slice(b))
    }
}

impl From<Vec<u8>> for RawStr {
    fn from(b: Vec<u8>) -> Self {
        Self::from_shared(Bytes::from(b))
    }
}

impl From<&str> for RawStr {
    /// Copies (use [`RawStr::from_static`] for a constant).
    fn from(s: &str) -> Self {
        Self::from_shared(Bytes::copy_from_slice(s.as_bytes()))
    }
}

impl From<Bytes> for RawStr {
    fn from(b: Bytes) -> Self {
        Self::from_shared(b)
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
