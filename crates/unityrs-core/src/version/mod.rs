// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/UnityVersion.py, UnityPy/files/BundleFile.py, UnityPy/files/SerializedFile.py. See THIRD_PARTY_LICENSES.
//! Unity versions with UnityPy's semantics, and UnityPy's two version chains.
//!
//! `UnityVersion.from_str` (`helpers/UnityVersion.py:62-88`) matches
//! `^(\d+)\.(\d+)\.(\d+)(.+?)?(\d+)?(.*)$` (`helpers/UnityVersion.py:7-10`)
//! with Python `re` (DOTALL; `\d` is any Unicode `Nd` digit). The type is one
//! character (the lazy group takes exactly the non-digit the greedy `build`
//! group leaves), lower-cased: `a` 0, `b` 1,
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
//! **No copy.** A version keeps the bytes it was parsed from ([`UnityVersion::raw`]),
//! and a [`VersionError::NoMatch`] the string that did not match. Both chains
//! and [`UnityVersion::parse`] take the string as the file's own
//! [`RawStr`] (a slice of its buffer, or the parent bundle's engine string,
//! or the policy's fallback) and keep a clone of it, a reference count: a
//! version string can be up to 32,767 bytes (`read_string_to_null`'s
//! window) and every parse of an entry keeps its own version, so a copy
//! would be an uncharged allocation per parse. The one heap a version owns
//! is a packed integer of 2^128 or more ([`UnityVersion::heap_bytes`]),
//! which the chains charge, at [`heap_cost`], to the handle charge of the
//! file that keeps the version before the limbs are allocated.
//!
//! **Work.** Every parse of an entry parses its version again (the
//! `SerializedFile` chain may parse the parent bundle's engine string, which
//! that parse did not read). So the chains count a parse's work on the
//! input's meter, as its `DecodeWork` ([`crate::limits::Meter`]), each step
//! before it runs: the bytes it scans, and [`LIMB_WORK`] for each limb a
//! step multiplies or merges. A group's digits are taken 19 at a time, each
//! chunk one multiply over the limbs the value holds so far, and each merge
//! over the group's own (review round 6 found every step going through all
//! 226 limbs of a fixed array, uncounted: about 12 limbs a digit, and 2,260
//! for the ten bytes of `2022.3.0f1`, ten steps of 226). So a group below
//! 2^128 costs at most three limbs a step, linear in the string (every
//! version a real file holds); a group of `d` significant digits past that
//! costs about `d * d / 730` limbs, quadratic as `CPython`'s own `int()` is,
//! which is why Python caps a group at [`PY_INT_MAX_STR_DIGITS`]: at most
//! about 6 limbs a digit, under 50 counted a byte of the string (`tests`,
//! `a_versions_packing_counts_the_limbs_it_holds_as_the_inputs_work`). A
//! refused step stops the parse, its scanned bytes counted.
//! [`UnityVersion::parse`], for a caller's own string, counts nothing.
//!
//! Owner: package C. The Unicode `Nd` table ([`crate::tables::nd_digit`]) is
//! generated from the oracle's Python 3.11 (`tools/gen/`, package A).

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use crate::error::Error;
use crate::limits::{Charge, heap_cost};
use crate::raw::RawStr;

mod packed;
mod parse;

pub use packed::Packed;
pub use parse::{LIMB_WORK, MAX_LIMBS};

/// Python 3.11's `sys.get_int_max_str_digits()`: `int()` refuses a longer
/// digit string, so a version group longer than this does not parse.
pub const PY_INT_MAX_STR_DIGITS: usize = 4300;

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
    /// is [`VersionError::NoMatch`] (module docs). The version, or the
    /// `NoMatch`, keeps a clone of `raw` (no copy: module docs). Its work is
    /// counted nowhere: this is for a caller's own string; an input's are
    /// parsed by the chains, which count it (module docs, "Work").
    pub fn parse(raw: &RawStr) -> Result<Self, VersionError> {
        match parse::parse_packed(raw.as_bytes(), parse::Work::new(None)) {
            Ok(Some(packed)) => Ok(Self {
                raw: raw.clone(),
                packed,
            }),
            // Uncounted and uncharged, the parse is never refused.
            Ok(None) | Err(_) => Err(VersionError::NoMatch(raw.clone())),
        }
    }

    /// [`Self::parse`] of an input's string, its work counted on `charge`'s
    /// meter (the input's) before each step runs, and a packed integer of
    /// 2^128 or more charged to `charge` before its limbs are allocated
    /// (module docs, "Work"). The outer error is the meter's refusal
    /// ([`crate::LimitKind::DecodeWork`], or the charge's).
    pub(crate) fn parse_charged(
        raw: &RawStr,
        charge: &mut Charge,
    ) -> Result<Result<Self, VersionError>, Error> {
        Ok(
            match parse::parse_packed(raw.as_bytes(), parse::Work::new(Some(charge)))? {
                Some(packed) => Ok(Self {
                    raw: raw.clone(),
                    packed,
                }),
                None => Err(VersionError::NoMatch(raw.clone())),
            },
        )
    }

    /// `UnityVersion(int)`: a version from a packed integer, as a TPK stores
    /// its versions (`helpers/Tpk.py:464`). It has no raw string (`raw()` is
    /// empty). Implemented by the scaffold, so `unityrs-tpk` (package A) can
    /// pick a TPK entry by core's ordering before package C lands.
    #[must_use]
    pub fn from_packed(packed: u64) -> Self {
        Self {
            raw: RawStr::default(),
            packed: Packed::from_u128(u128::from(packed)),
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

    /// The heap this version owns: a packed integer of 2^128 or more's
    /// limbs. The raw string is a shared slice and owns none. Implemented
    /// by the scaffold; the chains charge it, at [`heap_cost`], to the
    /// handle of the file that keeps it.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.packed.heap_bytes()
    }

    /// What its heap is charged at: the limbs' block, at [`heap_cost`].
    fn heap_charge(&self) -> u64 {
        match self.heap_bytes() {
            0 => 0,
            n => heap_cost(u64::try_from(n).unwrap_or(u64::MAX)),
        }
    }

    /// The packed value when it fits `u64` (TPK lookups compare these).
    /// Implemented by the scaffold.
    pub fn packed_u64(&self) -> Option<u64> {
        self.packed.as_u128().and_then(|v| u64::try_from(v).ok())
    }

    /// `(major, minor, build, type, number)`, each cut to its slot (16, 16, 16,
    /// 8 and 8 bits). A type byte other than 0-5 and 255 raises, as
    /// `UnityVersionType(...)` does (`UnityVersion.py:41-42`, `:109-110`).
    pub fn as_tuple(&self) -> Result<(u16, u16, u16, u8, u8), VersionError> {
        // UnityPy helpers/UnityVersion.py:28-54, :109-110
        let ty = u8::try_from(self.packed.bits(8, 8)).unwrap_or(u8::MAX);
        if !(ty <= 5 || ty == 255) {
            return Err(VersionError::TypeByte(ty));
        }
        let field = |shift: u32| u16::try_from(self.packed.bits(shift, 16)).unwrap_or(u16::MAX);
        Ok((
            field(48),
            field(32),
            field(16),
            ty,
            u8::try_from(self.packed.bits(0, 8)).unwrap_or(u8::MAX),
        ))
    }

    /// The 16-bit major, as UnityPy's `major` property cuts it.
    #[must_use]
    pub fn major(&self) -> u16 {
        // UnityPy helpers/UnityVersion.py:28-30
        u16::try_from(self.packed.bits(48, 16)).unwrap_or(u16::MAX)
    }

    /// `self < other` as Python compares `as_tuple()` with a shorter tuple
    /// (`version < (2020, 3, 34)`).
    pub fn lt_tuple(&self, other: &[u16]) -> Result<bool, VersionError> {
        // UnityPy helpers/UnityVersion.py:122-127: tuple order, a shorter
        // tuple that is a prefix being the smaller.
        let (major, minor, build, ty, number) = self.as_tuple()?;
        let mine = [major, minor, build, u16::from(ty), u16::from(number)];
        Ok(mine.as_slice() < other)
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

/// Chain 1: `BundleFile.parse_version`. `engine` is the header's
/// `version_engine`, kept by the result without a copy (module docs). Each
/// parse's work is counted on `charge`'s meter, the input's, and the
/// result's heap is charged to `charge`, the handle charge of the bundle
/// that keeps it, before it is allocated (module docs, "Work"): the error
/// is a [`VersionError`] or such a refusal.
pub fn resolve_bundle(
    engine: &RawStr,
    policy: &VersionPolicy,
    charge: &mut Charge,
) -> Result<ResolvedVersion, Error> {
    if let Some(v) = &policy.force_bundle {
        return forced(v, charge);
    }
    // UnityPy files/BundleFile.py:523-536: a string that does not parse, or
    // whose 16-bit major is 0, takes the fallback.
    match UnityVersion::parse_charged(engine, charge)? {
        Ok(version) if version.major() != 0 => {
            return Ok(ResolvedVersion {
                version,
                source: VersionSource::Own,
            });
        }
        Ok(dropped) => charge.release(dropped.heap_charge()),
        Err(_) => {}
    }
    fallback(policy, charge)
}

/// A forced version, its heap charged before the copy.
fn forced(v: &UnityVersion, charge: &mut Charge) -> Result<ResolvedVersion, Error> {
    charge.grow(v.heap_charge())?;
    Ok(ResolvedVersion {
        version: v.clone(),
        source: VersionSource::Forced,
    })
}

/// `UnityVersion.from_str(config.get_fallback_version())` (`config.py:25-32`):
/// no fallback raises; a fallback that does not match raises too.
fn fallback(policy: &VersionPolicy, charge: &mut Charge) -> Result<ResolvedVersion, Error> {
    let s = policy.fallback.as_ref().ok_or(VersionError::NoFallback)?;
    Ok(ResolvedVersion {
        version: UnityVersion::parse_charged(s, charge)??,
        source: VersionSource::Fallback,
    })
}

/// `not s or s == "0.0.0"` (`SerializedFile.py:353`, `:358`).
fn unusable(s: &RawStr) -> bool {
    s.is_empty() || s.as_bytes() == b"0.0.0"
}

/// Chain 2: `SerializedFile.set_version`. `own` is the file's string (format
/// 7 and up); `parent_engine` is `Some` only when the parent is a bundle
/// (its header's `version_engine`). The string the chain picks is kept by
/// the result without a copy (module docs). The parse's work and the
/// result's heap go to `charge`, as [`resolve_bundle`]'s do.
pub fn resolve_serialized(
    own: &RawStr,
    parent_engine: Option<&RawStr>,
    policy: &VersionPolicy,
    charge: &mut Charge,
) -> Result<ResolvedVersion, Error> {
    if let Some(v) = &policy.force_serialized {
        return forced(v, charge);
    }
    // UnityPy files/SerializedFile.py:351-360
    let (mut s, mut source) = (own, VersionSource::Own);
    if unusable(s) {
        if let Some(parent) = parent_engine {
            (s, source) = (parent, VersionSource::ParentBundle);
        }
        if unusable(s) {
            return fallback(policy, charge);
        }
    }
    Ok(ResolvedVersion {
        version: UnityVersion::parse_charged(s, charge)??,
        source,
    })
}

/// `BundleFile.py:113-121`: `ArchiveFlagsOld` below 2020, and below 2020.3.34,
/// 2021.3.2 and 2022.1.1 on those lines.
pub fn flags_class(engine: &UnityVersion) -> Result<FlagsClass, VersionError> {
    // UnityPy files/BundleFile.py:113-121; `version[0]` is `as_tuple()[0]`,
    // which raises as the comparison before it does.
    let v = engine;
    let major = v.as_tuple()?.0;
    let old = v.lt_tuple(&[2020])?
        || (major == 2020 && v.lt_tuple(&[2020, 3, 34])?)
        || (major == 2021 && v.lt_tuple(&[2021, 3, 2])?)
        || (major == 2022 && v.lt_tuple(&[2022, 1, 1])?);
    Ok(if old {
        FlagsClass::Old
    } else {
        FlagsClass::New
    })
}

/// `BundleFile.py:129-131`: the 16-byte alignment after the header applies at
/// format 7 and up, or with a 2019 engine at 2019.4.15 or later.
pub fn uses_block_alignment(format: u32, engine: &UnityVersion) -> Result<bool, VersionError> {
    // UnityPy files/BundleFile.py:129: `or` short-circuits, so from format 7
    // the version is never compared.
    if format >= 7 {
        return Ok(true);
    }
    Ok(engine.as_tuple()?.0 == 2019 && !engine.lt_tuple(&[2019, 4, 15])?)
}

#[cfg(test)]
mod tests;
