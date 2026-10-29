//! Unity versions with UnityPy's semantics, and UnityPy's two version chains.
//!
//! `UnityVersion.from_str` (`helpers/UnityVersion.py:62-88`) matches
//! `^(\d+)\.(\d+)\.(\d+)(.)?(\d+)?(.*)$` with Python `re` (DOTALL; `\d` is any
//! Unicode `Nd` digit). The type is one character, lower-cased: `a` 0, `b` 1,
//! `c` 2, `f` 3, `p` 4, `x` 5, anything else 255, nothing at all `f`. The packed
//! value is `major<<48 | minor<<32 | build<<16 | type<<8 | number`, a plain OR
//! of unbounded integers, so wide fields spill (`docs/CANONICAL_DUMP.md` 4.2).
//! Each group goes through Python 3.11's `int()`, which refuses a string of
//! more than [`PY_INT_MAX_STR_DIGITS`] digits (`sys.get_int_max_str_digits()`,
//! leading zeros and any `Nd` digit counted): `from_str` then raises
//! `ValueError`, exactly as for a string that does not match, so both chains
//! treat it as [`VersionError::NoMatch`] (the bundle chain falls back, the
//! `SerializedFile` chain fails).
//!
//! Chain 1, the bundle (`files/BundleFile.py:523-536`): the engine string; when
//! it does not parse or its 16-bit major is 0, the fallback; with no fallback,
//! an error. It decides the flag set (`:113-121`) and the format-6 alignment
//! (`:129-131`).
//!
//! Chain 2, the `SerializedFile` (`files/SerializedFile.py:351-360`): its own
//! string (format 7 and up); when empty or `0.0.0`, the parent *bundle's*
//! engine string (a `WebFile` parent does not count); when still empty or
//! `0.0.0`, the fallback; then `from_str`, which fails on a string that does
//! not match whatever the fallback holds.
//!
//! Owner: package C. The Unicode `Nd` table ([`crate::tables::nd_digit`]) is
//! generated from the oracle's Python 3.11 (`tools/gen/`, package A).

// STUB(C): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::raw::RawStr;

/// Python 3.11's `sys.get_int_max_str_digits()`: `int()` refuses a longer
/// digit string, so a version group longer than this does not parse.
pub const PY_INT_MAX_STR_DIGITS: usize = 4300;

/// The packed integer, exactly. Most versions fit `u128`; a major of 2^80 or
/// more does not, and the canonical dump writes the exact integer. Ordered as
/// Python orders the integers (implemented by the scaffold).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Packed {
    /// Below 2^128.
    Small(u128),
    /// 2^128 or more: little-endian `u64` limbs, the last one non-zero.
    Big(Box<[u64]>),
}

impl Ord for Packed {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Small(a), Self::Small(b)) => a.cmp(b),
            (Self::Small(_), Self::Big(_)) => Ordering::Less,
            (Self::Big(_), Self::Small(_)) => Ordering::Greater,
            // Limbs are little-endian with a non-zero last limb: more limbs is
            // larger, else compare from the most significant limb down.
            (Self::Big(a), Self::Big(b)) => a
                .len()
                .cmp(&b.len())
                .then_with(|| a.iter().rev().cmp(b.iter().rev())),
        }
    }
}

impl PartialOrd for Packed {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Packed {
    /// The exact decimal integer, as the dump writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// A parsed Unity version: the raw bytes and UnityPy's packed integer.
#[derive(Clone, Debug)]
pub struct UnityVersion {
    raw: RawStr,
    packed: Packed,
}

impl UnityVersion {
    /// `UnityVersion.from_str` on the raw bytes. The pattern is applied to the
    /// surrogate-escaped string Python sees, so invalid UTF-8 bytes match `.`
    /// and never `\d`; a group of more than [`PY_INT_MAX_STR_DIGITS`] digits
    /// is [`VersionError::NoMatch`] (module docs).
    pub fn parse(raw: &[u8]) -> Result<Self, VersionError> {
        todo!()
    }

    /// `UnityVersion(int)`: a version from a packed integer, as a TPK stores
    /// its versions (`helpers/Tpk.py:464`). It has no raw string (`raw()` is
    /// empty). Implemented by the scaffold, so `unityrs-tpk` (package A) can
    /// pick a TPK entry by core's ordering before package C lands.
    #[must_use]
    pub fn from_packed(packed: u64) -> Self {
        Self {
            raw: RawStr::default(),
            packed: Packed::Small(u128::from(packed)),
        }
    }

    /// The bytes it was parsed from.
    pub fn raw(&self) -> &RawStr {
        &self.raw
    }

    /// The packed integer, exactly.
    pub fn packed(&self) -> &Packed {
        &self.packed
    }

    /// The packed value when it fits `u64` (TPK lookups compare these).
    /// Implemented by the scaffold.
    pub fn packed_u64(&self) -> Option<u64> {
        match &self.packed {
            Packed::Small(v) => u64::try_from(*v).ok(),
            Packed::Big(_) => None,
        }
    }

    /// `(major, minor, build, type, number)`, each cut to its slot (16, 16, 16,
    /// 8 and 8 bits). A type byte other than 0-5 and 255 raises, as
    /// `UnityVersionType(...)` does (`UnityVersion.py:41-42`, `:109-110`).
    pub fn as_tuple(&self) -> Result<(u16, u16, u16, u8, u8), VersionError> {
        todo!()
    }

    /// The 16-bit major, as UnityPy's `major` property cuts it.
    pub fn major(&self) -> u16 {
        todo!()
    }

    /// `self < other` as Python compares `as_tuple()` with a shorter tuple
    /// (`version < (2020, 3, 34)`).
    pub fn lt_tuple(&self, other: &[u16]) -> Result<bool, VersionError> {
        todo!()
    }
}

impl PartialEq for UnityVersion {
    /// Equality is on the packed integer, as Python's `int` equality.
    fn eq(&self, other: &Self) -> bool {
        self.packed == other.packed
    }
}

impl Eq for UnityVersion {}

impl Hash for UnityVersion {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.packed.hash(state);
    }
}

impl Ord for UnityVersion {
    /// As Python compares the `int`s (`Tpk.py:472-481`'s `exactVersion >= version`).
    fn cmp(&self, other: &Self) -> Ordering {
        self.packed.cmp(&other.packed)
    }
}

impl PartialOrd for UnityVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Why a version is what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VersionSource {
    /// The file's own string.
    Own,
    /// The parent bundle's engine string (chain 2 only).
    ParentBundle,
    /// `config.FALLBACK_UNITY_VERSION`.
    Fallback,
    /// An opt-in override, never silent: reported in the API, the batch
    /// manifest and the dump's `producer`.
    Forced,
}

/// A chain's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedVersion {
    /// The version.
    pub version: UnityVersion,
    /// Where it came from.
    pub source: VersionSource,
}

/// The fallback and the overrides.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct VersionPolicy {
    /// UnityPy's `config.FALLBACK_UNITY_VERSION`, as a string (UnityPy parses it
    /// each time it is used; a bad string fails there). `None` raises where
    /// UnityPy raises (`config.py:25-32`); the oracle leaves it unset.
    pub fallback: Option<RawStr>,
    /// Replaces chain 1's result.
    pub force_bundle: Option<UnityVersion>,
    /// Replaces chain 2's result.
    pub force_serialized: Option<UnityVersion>,
}

/// UnityPy's choice between `ArchiveFlags` and `ArchiveFlagsOld`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagsClass {
    /// `ArchiveFlags`.
    New,
    /// `ArchiveFlagsOld`.
    Old,
}

/// Version failures.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum VersionError {
    /// The string does not match the pattern.
    #[error("invalid version string {0}")]
    NoMatch(RawStr),
    /// A type byte outside 0-5 and 255 in a comparison.
    #[error("invalid version type byte {0}")]
    TypeByte(u8),
    /// No usable version and no fallback set.
    #[error("no valid Unity version, and no fallback version set")]
    NoFallback,
}

/// Chain 1: `BundleFile.parse_version`.
pub fn resolve_bundle(
    engine: &[u8],
    policy: &VersionPolicy,
) -> Result<ResolvedVersion, VersionError> {
    todo!()
}

/// Chain 2: `SerializedFile.set_version`. `own` is the file's string (format
/// 7 and up); `parent_engine` is `Some` only when the parent is a bundle.
pub fn resolve_serialized(
    own: &[u8],
    parent_engine: Option<&[u8]>,
    policy: &VersionPolicy,
) -> Result<ResolvedVersion, VersionError> {
    todo!()
}

/// `BundleFile.py:113-121`: `ArchiveFlagsOld` below 2020, and below 2020.3.34,
/// 2021.3.2 and 2022.1.1 on those lines.
pub fn flags_class(engine: &UnityVersion) -> Result<FlagsClass, VersionError> {
    todo!()
}

/// `BundleFile.py:129-131`: the 16-byte alignment after the header applies at
/// format 7 and up, or with a 2019 engine at 2019.4.15 or later.
pub fn uses_block_alignment(format: u32, engine: &UnityVersion) -> Result<bool, VersionError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::{Packed, UnityVersion};

    #[test]
    fn a_tpk_version_round_trips_and_orders_as_python_ints() {
        // 2022.3.0f1 packed as UnityPy packs it.
        let v2022 = (2022u64 << 48) | (3 << 32) | (3 << 8) | 1;
        let v2019 = (2019u64 << 48) | (4 << 32) | (15 << 16) | (3 << 8) | 1;
        let a = UnityVersion::from_packed(v2022);
        let b = UnityVersion::from_packed(v2019);
        assert_eq!(a.packed_u64(), Some(v2022));
        assert!(b < a);
        assert_eq!(a, UnityVersion::from_packed(v2022));
        assert!(a.raw().is_empty());
        let mut v = vec![a.clone(), b.clone()];
        v.sort();
        assert_eq!(v, [b, a]);
    }

    #[test]
    fn big_packed_values_order_by_magnitude() {
        let small = Packed::Small(u128::MAX);
        let big_lo = Packed::Big(vec![u64::MAX, 0, 1].into());
        let big_hi = Packed::Big(vec![0, 1, 1].into());
        let bigger = Packed::Big(vec![0, 0, 0, 1].into());
        assert!(small < big_lo);
        assert!(big_lo < big_hi, "compared from the most significant limb");
        assert!(big_hi < bigger);
        assert_eq!(
            UnityVersion {
                raw: "x".into(),
                packed: big_lo.clone()
            }
            .packed_u64(),
            None
        );
    }
}
