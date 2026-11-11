// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/UnityVersion.py's indexing and tuple comparisons, as MeshHandler uses them. See THIRD_PARTY_LICENSES.
//! The version `MeshHandler` compares with.
//!
//! UnityPy hands the handler the object's `UnityVersion`, an `int`, and
//! the handler reads it three ways: `version[0]`, `version[:2]` and
//! `version >= (2017, 4)`. Each builds the tuple `(major, minor, build,
//! type, number)` (`__getitem__`, `helpers/UnityVersion.py:99-107`;
//! `as_tuple`, `:109-110`; `__gt__` and `__ge__`, `:132-140`), whose
//! `type` is `UnityVersionType(byte)` (`:41-42`), so every read raises
//! `ValueError` for a type byte that is no member (of a version string's
//! type number, `:57-60`, the number's bits 8 to 15 are OR-ed into the type
//! byte: `a` first reads as `b` at 256, `p` first raises at 512, `f` at
//! 1024). [`Version`]
//! keeps the tuple or that byte, and each read returns the tuple's answer
//! or [`MeshError::VersionType`].
//!
//! A 5-tuple compared with a shorter one is Python's tuple order: equal
//! on the shorter one's length, the longer is the greater, so
//! `version >= (2017, 4)` holds exactly when `(major, minor)` is at least
//! `(2017, 4)`, and `==` with a shorter tuple never holds.

use unityrs_core::UnityVersion;
use unityrs_core::version::VersionError;

use crate::error::MeshError;

/// The version a handler reads (module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    /// `(major, minor, build, type, number)`, or the type byte that is no
    /// `UnityVersionType`.
    parts: Result<[u16; 5], u8>,
}

const _: () = crate::send_sync::<Version>();

impl Version {
    /// The version of an object's file, as UnityPy's `object_reader.version`
    /// is (the caller's to pick: the handler is given one, as `SpriteHelper`
    /// gives it, `export/SpriteHelper.py:117`).
    #[must_use]
    pub fn of(version: &UnityVersion) -> Self {
        let parts = match version.as_tuple() {
            Ok((major, minor, build, ty, number)) => {
                Ok([major, minor, build, u16::from(ty), u16::from(number)])
            }
            Err(VersionError::TypeByte(b)) => Err(b),
            // `as_tuple` raises nothing else; were it to, the type byte
            // 6 is no member either.
            Err(_) => Err(6),
        };
        Self { parts }
    }

    /// A version from its parts, `type` UnityPy's `UnityVersionType` value
    /// (0 to 5, or 255; another is kept as the byte every read raises
    /// for).
    #[must_use]
    pub const fn new(major: u16, minor: u16, build: u16, ty: u8, number: u8) -> Self {
        let parts = if ty <= 5 || ty == 255 {
            Ok([major, minor, build, ty as u16, number as u16])
        } else {
            Err(ty)
        };
        Self { parts }
    }

    /// The tuple, or `ValueError` for its type byte.
    pub fn as_tuple(self) -> Result<[u16; 5], MeshError> {
        self.parts.map_err(MeshError::VersionType)
    }

    /// `version[0]`.
    pub fn major(self) -> Result<u16, MeshError> {
        self.as_tuple().map(|[major, ..]| major)
    }

    /// `version >= other`, `other` a tuple of at most five parts.
    pub fn at_least(self, other: &[u16]) -> Result<bool, MeshError> {
        // UnityPy helpers/UnityVersion.py:132-140: `>` or `==`, each on
        // `as_tuple()`, so Python's tuple order.
        self.as_tuple().map(|t| t.as_slice() >= other)
    }

    /// `version[:2] == (major, minor)`.
    pub fn minor_is(self, major: u16, minor: u16) -> Result<bool, MeshError> {
        self.as_tuple().map(|[a, b, ..]| a == major && b == minor)
    }
}

#[cfg(test)]
mod tests {
    use unityrs_core::{RawStr, UnityVersion};

    use super::Version;
    use crate::error::MeshError;

    fn parsed(s: &str) -> Version {
        Version::of(&UnityVersion::parse(&RawStr::from(s)).unwrap())
    }

    #[test]
    fn a_file_s_version_reads_as_unitypy_s_tuple_or_its_type_byte_s_error() {
        assert_eq!(parsed("2019.4.0f1"), Version::new(2019, 4, 0, 3, 1));
        // A type number of 256 or more is OR-ed into the type byte (module
        // docs): `a` (0) reads as `b` (1), and `f` (3) as 7 from 1024, no
        // member, which every read raises for.
        assert_eq!(parsed("2019.4.0a256"), Version::new(2019, 4, 0, 1, 0));
        let v = parsed("2019.4.0f1024");
        assert!(matches!(v.major(), Err(MeshError::VersionType(7))));
        assert_eq!(
            v.as_tuple().unwrap_err().python(),
            Some(("ValueError", "7 is not a valid UnityVersionType".to_owned()))
        );
        // A packed version of type 6, and of `u` (255), which is a member.
        let packed = |ty: u64| {
            Version::of(&UnityVersion::from_packed(
                (2019 << 48) | (4 << 32) | (ty << 8) | 1,
            ))
        };
        assert!(matches!(
            packed(6).at_least(&[2017, 4]),
            Err(MeshError::VersionType(6))
        ));
        assert_eq!(packed(6), Version::new(2019, 4, 0, 6, 1));
        assert_eq!(packed(255).as_tuple().unwrap(), [2019, 4, 0, 255, 1]);
    }
}
