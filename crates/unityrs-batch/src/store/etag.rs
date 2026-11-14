//! `ETag`s as the engine keeps and compares them: the bytes as
//! the store returned them, at most 100 (`docs/plans/batch.md` 2.9), compared
//! with one surrounding pair of quotes stripped and hex digits in any case.

/// The longest `ETag` the engine keeps; a longer one fails its try as a 2xx
/// the call returns as an error (`docs/BATCH_SPEC.md` 1.6).
pub const ETAG_MAX: usize = 100;

/// An `ETag` as the store returned it, in fixed storage.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Etag {
    bytes: [u8; ETAG_MAX],
    len: u8,
}

impl Etag {
    /// Keeps `raw`, or `None` when it is over [`ETAG_MAX`] bytes.
    #[must_use]
    pub fn new(raw: &[u8]) -> Option<Self> {
        let mut bytes = [0; ETAG_MAX];
        bytes.get_mut(..raw.len())?.copy_from_slice(raw);
        Some(Self {
            bytes,
            len: u8::try_from(raw.len()).ok()?,
        })
    }
    /// The bytes as returned.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or_default()
    }
    /// Whether it names `expected` (a digest's lowercase hex, or a
    /// multipart `ETag`'s `<hex>-<parts>`), by [`matches`].
    #[must_use]
    pub fn matches(&self, expected: &[u8]) -> bool {
        matches(self.as_bytes(), expected)
    }
}

/// Its length alone: an `ETag` is the store's, not the caller's, and nothing
/// renders it.
impl std::fmt::Debug for Etag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Etag({} bytes)", self.len)
    }
}

/// `raw` without one surrounding pair of double quotes.
#[must_use]
pub fn normalize(raw: &[u8]) -> &[u8] {
    match raw {
        [b'"', inner @ .., b'"'] => inner,
        _ => raw,
    }
}

/// Whether a returned `raw` `ETag` names `expected`: quotes stripped once,
/// then equal but for ASCII case.
#[must_use]
pub fn matches(raw: &[u8], expected: &[u8]) -> bool {
    normalize(raw).eq_ignore_ascii_case(normalize(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pair_of_quotes_is_stripped_and_hex_compares_in_any_case() {
        let md5 = b"9b2cf535f27731c974343645a3985328";
        assert!(matches(b"\"9B2CF535F27731C974343645A3985328\"", md5));
        assert!(matches(md5, md5));
        assert!(matches(
            b"\"d41d8cd98f00b204e9800998ecf8427e-3\"",
            b"D41D8CD98F00B204E9800998ECF8427E-3"
        ));
        // One pair only, and only a surrounding one.
        assert!(!matches(b"\"\"9b2cf535f27731c974343645a3985328\"\"", md5));
        assert!(!matches(b"\"9b2cf535f27731c974343645a3985328", md5));
        assert!(!matches(b"W/\"9b2cf535f27731c974343645a3985328\"", md5));
        assert!(!matches(b"9b2cf535f27731c974343645a3985329", md5));
        assert!(!matches(b"9b2cf535f27731c974343645a3985328-1", md5));
        assert_eq!(normalize(b"\""), b"\"");
        assert_eq!(normalize(b"\"\""), b"");
    }

    #[test]
    fn the_bytes_are_kept_as_returned_up_to_one_hundred() {
        let raw = [b'a'; ETAG_MAX];
        let kept = Etag::new(&raw).unwrap();
        assert_eq!(kept.as_bytes(), raw);
        assert!(Etag::new(&[b'a'; ETAG_MAX + 1]).is_none());
        let quoted = Etag::new(b"\"ABC\"").unwrap();
        assert_eq!(quoted.as_bytes(), b"\"ABC\"");
        assert!(quoted.matches(b"abc"));
        assert_eq!(format!("{quoted:?}"), "Etag(5 bytes)");
        assert_eq!(Etag::new(b"").unwrap().as_bytes(), b"");
    }
}
