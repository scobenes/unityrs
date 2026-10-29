// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/UnityVersion.py, UnityPy/files/BundleFile.py, UnityPy/files/SerializedFile.py. See THIRD_PARTY_LICENSES.
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
//! **No copy.** A version keeps the bytes it was parsed from ([`UnityVersion::raw`]),
//! and a [`VersionError::NoMatch`] the string that did not match. Both chains
//! and [`UnityVersion::parse`] take the string as the file's own
//! [`RawStr`] (a slice of its buffer, or the parent bundle's engine string,
//! or the policy's fallback) and keep a clone of it, a reference count: a
//! version string can be up to 32,767 bytes (`read_string_to_null`'s
//! window) and every parse of an entry keeps its own version, so a copy
//! would be an uncharged allocation per parse. The one heap a version owns
//! is a packed integer of 2^128 or more ([`UnityVersion::heap_bytes`]),
//! which the file that keeps the version charges with its handle.
//!
//! Owner: package C. The Unicode `Nd` table ([`crate::tables::nd_digit`]) is
//! generated from the oracle's Python 3.11 (`tools/gen/`, package A).

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::raw::RawStr;

/// Python 3.11's `sys.get_int_max_str_digits()`: `int()` refuses a longer
/// digit string, so a version group longer than this does not parse.
pub const PY_INT_MAX_STR_DIGITS: usize = 4300;

/// The packed integer, exactly. Most versions fit `u128`; a major of 2^80 or
/// more does not, and the canonical dump writes the exact integer. Ordered,
/// compared and hashed as Python's `int` (implemented by the scaffold).
///
/// The representation is private and always canonical: a value below 2^128
/// is held as a `u128`, and a larger one as little-endian limbs with a
/// non-zero top limb. So [`Packed::from_limbs`] of a value written with
/// leading zero limbs (a 4,300-digit group of leading zeros, sized from its
/// digit count) equals, hashes and orders as the same value from
/// [`Packed::from_u128`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Packed(Repr);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Repr {
    /// Below 2^128.
    Small(u128),
    /// 2^128 or more: little-endian `u64` limbs, more than two, the last
    /// one non-zero.
    Big(Box<[u64]>),
}

impl Packed {
    /// The value `v`.
    #[must_use]
    pub const fn from_u128(v: u128) -> Self {
        Self(Repr::Small(v))
    }

    /// The value of little-endian `u64` limbs, any number of them, leading
    /// (most significant) zero limbs included.
    #[must_use]
    pub fn from_limbs(limbs: &[u64]) -> Self {
        let len = limbs.iter().rposition(|l| *l != 0).map_or(0, |i| i + 1);
        let limbs = limbs.get(..len).unwrap_or_default();
        match limbs {
            [] => Self::from_u128(0),
            [lo] => Self::from_u128(u128::from(*lo)),
            [lo, hi] => Self::from_u128(u128::from(*lo) | (u128::from(*hi) << 64)),
            _ => Self(Repr::Big(limbs.into())),
        }
    }

    /// The value, when it is below 2^128.
    #[must_use]
    pub fn as_u128(&self) -> Option<u128> {
        match &self.0 {
            Repr::Small(v) => Some(*v),
            Repr::Big(_) => None,
        }
    }

    /// The value as little-endian `u64` limbs, without leading zero limbs
    /// (empty for 0).
    #[must_use]
    pub fn to_limbs(&self) -> Vec<u64> {
        match &self.0 {
            Repr::Small(v) => {
                // Both halves of a u128, the high one first cut away when zero.
                let lo = u64::try_from(v & u128::from(u64::MAX)).unwrap_or(u64::MAX);
                let hi = u64::try_from(v >> 64).unwrap_or(u64::MAX);
                match (lo, hi) {
                    (0, 0) => Vec::new(),
                    (lo, 0) => vec![lo],
                    (lo, hi) => vec![lo, hi],
                }
            }
            Repr::Big(limbs) => limbs.to_vec(),
        }
    }

    /// `(self >> shift) & ((1 << width) - 1)`, for `width` up to 64.
    fn bits(&self, shift: u32, width: u32) -> u64 {
        let mask = if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        match &self.0 {
            Repr::Small(v) => {
                u64::try_from(v.checked_shr(shift).unwrap_or(0) & u128::from(mask)).unwrap_or(0)
            }
            Repr::Big(limbs) => {
                let word = usize::try_from(shift / 64).unwrap_or(usize::MAX);
                let bit = shift % 64;
                let lo = limbs.get(word).copied().unwrap_or(0);
                let hi = limbs.get(word.saturating_add(1)).copied().unwrap_or(0);
                let v = (u128::from(hi) << 64 | u128::from(lo)) >> bit;
                u64::try_from(v & u128::from(mask)).unwrap_or(0)
            }
        }
    }

    /// The heap bytes the value owns (0 below 2^128).
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        match &self.0 {
            Repr::Small(_) => 0,
            Repr::Big(limbs) => limbs.len() * 8,
        }
    }
}

impl Ord for Packed {
    fn cmp(&self, other: &Self) -> Ordering {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => a.cmp(b),
            // Canonical: every `Big` is at least 2^128, above every `Small`.
            (Repr::Small(_), Repr::Big(_)) => Ordering::Less,
            (Repr::Big(_), Repr::Small(_)) => Ordering::Greater,
            // Limbs are little-endian with a non-zero last limb: more limbs is
            // larger, else compare from the most significant limb down.
            (Repr::Big(a), Repr::Big(b)) => a
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
        // Repeated division by 10^19, least significant chunk first.
        const CHUNK: u64 = 10_000_000_000_000_000_000;
        let limbs = match &self.0 {
            Repr::Small(v) => return write!(f, "{v}"),
            Repr::Big(limbs) => limbs,
        };
        let mut n: Vec<u64> = limbs.to_vec();
        let mut chunks: Vec<u64> = Vec::new();
        while n.iter().any(|l| *l != 0) {
            let mut rem: u128 = 0;
            for l in n.iter_mut().rev() {
                let cur = (rem << 64) | u128::from(*l);
                *l = u64::try_from(cur / u128::from(CHUNK)).unwrap_or(u64::MAX);
                rem = cur % u128::from(CHUNK);
            }
            chunks.push(u64::try_from(rem).unwrap_or(0));
            while n.last() == Some(&0) {
                n.pop();
            }
        }
        let mut it = chunks.iter().rev();
        if let Some(top) = it.next() {
            write!(f, "{top}")?;
        }
        for c in it {
            write!(f, "{c:019}")?;
        }
        Ok(())
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
    /// is [`VersionError::NoMatch`] (module docs). The version, or the
    /// `NoMatch`, keeps a clone of `raw` (no copy: module docs).
    pub fn parse(raw: &RawStr) -> Result<Self, VersionError> {
        parse_packed(raw.as_bytes())
            .map(|packed| Self {
                raw: raw.clone(),
                packed,
            })
            .ok_or_else(|| VersionError::NoMatch(raw.clone()))
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
    /// by the scaffold; a file charges it with its handle.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.packed.heap_bytes()
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
/// `version_engine`, kept by the result without a copy (module docs).
pub fn resolve_bundle(
    engine: &RawStr,
    policy: &VersionPolicy,
) -> Result<ResolvedVersion, VersionError> {
    if let Some(v) = &policy.force_bundle {
        return Ok(ResolvedVersion {
            version: v.clone(),
            source: VersionSource::Forced,
        });
    }
    // UnityPy files/BundleFile.py:523-536: a string that does not parse, or
    // whose 16-bit major is 0, takes the fallback.
    if let Ok(version) = UnityVersion::parse(engine)
        && version.major() != 0
    {
        return Ok(ResolvedVersion {
            version,
            source: VersionSource::Own,
        });
    }
    fallback(policy)
}

/// `UnityVersion.from_str(config.get_fallback_version())` (`config.py:25-32`):
/// no fallback raises; a fallback that does not match raises too.
fn fallback(policy: &VersionPolicy) -> Result<ResolvedVersion, VersionError> {
    let s = policy.fallback.as_ref().ok_or(VersionError::NoFallback)?;
    Ok(ResolvedVersion {
        version: UnityVersion::parse(s)?,
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
/// the result without a copy (module docs).
pub fn resolve_serialized(
    own: &RawStr,
    parent_engine: Option<&RawStr>,
    policy: &VersionPolicy,
) -> Result<ResolvedVersion, VersionError> {
    if let Some(v) = &policy.force_serialized {
        return Ok(ResolvedVersion {
            version: v.clone(),
            source: VersionSource::Forced,
        });
    }
    // UnityPy files/SerializedFile.py:351-360
    let (mut s, mut source) = (own, VersionSource::Own);
    if unusable(s) {
        if let Some(parent) = parent_engine {
            (s, source) = (parent, VersionSource::ParentBundle);
        }
        if unusable(s) {
            return fallback(policy);
        }
    }
    Ok(ResolvedVersion {
        version: UnityVersion::parse(s)?,
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

/// Limbs of a packed version: 4,300 decimal digits are 14,284 bits (224
/// limbs), shifted by at most 48 more.
const LIMBS: usize = 226;

/// A non-negative integer of up to [`LIMBS`] little-endian limbs, on the
/// stack, so parsing allocates nothing but a big result.
struct Wide([u64; LIMBS]);

impl Wide {
    fn zero() -> Self {
        Self([0; LIMBS])
    }

    /// `self = self * mul + add`.
    fn mul_add(&mut self, mul: u64, add: u64) {
        let mut carry = u128::from(add);
        for l in &mut self.0 {
            let cur = u128::from(*l) * u128::from(mul) + carry;
            *l = u64::try_from(cur & u128::from(u64::MAX)).unwrap_or(0);
            carry = cur >> 64;
        }
    }

    /// `self |= other << shift`.
    fn or_shifted(&mut self, other: &Self, shift: u32) {
        let words = usize::try_from(shift / 64).unwrap_or(LIMBS);
        let bits = shift % 64;
        for (i, l) in other.0.iter().enumerate() {
            if *l == 0 {
                continue;
            }
            if let Some(dst) = self.0.get_mut(i + words) {
                *dst |= l << bits;
            }
            if bits > 0
                && let Some(dst) = self.0.get_mut(i + words + 1)
            {
                *dst |= l >> (64 - bits);
            }
        }
    }
}

/// One unit of the string Python sees: a code point, or a byte that is not
/// valid UTF-8 (a lone surrogate under `surrogateescape`).
#[derive(Clone, Copy)]
enum Unit {
    Char(char),
    Byte,
}

fn units(bytes: &[u8]) -> impl Iterator<Item = Unit> + '_ {
    bytes.utf8_chunks().flat_map(|chunk| {
        chunk
            .valid()
            .chars()
            .map(Unit::Char)
            .chain(chunk.invalid().iter().map(|_| Unit::Byte))
    })
}

fn digit(u: Unit) -> Option<u8> {
    match u {
        Unit::Char(c) => crate::tables::nd_digit(c),
        Unit::Byte => None,
    }
}

/// A run of `\d` from `it`, through Python's `int()`: `None` for more than
/// [`PY_INT_MAX_STR_DIGITS`] digits; the value and the digit count.
fn digits<I: Iterator<Item = Unit>>(it: &mut std::iter::Peekable<I>) -> Option<(Wide, usize)> {
    let mut value = Wide::zero();
    let (mut count, mut chunk, mut chunk_len) = (0usize, 0u64, 0u32);
    while let Some(d) = it.peek().copied().and_then(digit) {
        it.next();
        count += 1;
        if count > PY_INT_MAX_STR_DIGITS {
            return None;
        }
        chunk = chunk * 10 + u64::from(d);
        chunk_len += 1;
        if chunk_len == 19 {
            value.mul_add(10_u64.pow(19), chunk);
            (chunk, chunk_len) = (0, 0);
        }
    }
    if chunk_len > 0 {
        value.mul_add(10_u64.pow(chunk_len), chunk);
    }
    Some((value, count))
}

/// `UnityVersion.from_str` (`helpers/UnityVersion.py:62-88`): the pattern
/// `^(\d+)\.(\d+)\.(\d+)(.+?)?(\d+)?(.*)$` under DOTALL, whose lazy type
/// group takes exactly the one character after `build`'s digits whenever
/// there is one (the greedy `build` leaves a non-digit there and the rest
/// always matches), then `from_list`'s unbounded OR. `None` where it raises
/// `ValueError`.
fn parse_packed(raw: &[u8]) -> Option<Packed> {
    let mut it = units(raw).peekable();
    let mut packed = Wide::zero();
    for (i, shift) in [48u32, 32, 16].into_iter().enumerate() {
        let (group, n) = digits(&mut it)?;
        if n == 0 {
            return None;
        }
        if i < 2 && !matches!(it.next(), Some(Unit::Char('.'))) {
            return None;
        }
        packed.or_shifted(&group, shift);
    }
    // No type character packs as `f` (3); `getattr(UnityVersionType,
    // c.lower(), u)` names a type only for these six letters (no other code
    // point lowers to one of them).
    let (ty, number) = match it.next() {
        None => (3u64, None),
        Some(unit) => {
            let ty = match unit {
                Unit::Char('a' | 'A') => 0,
                Unit::Char('b' | 'B') => 1,
                Unit::Char('c' | 'C') => 2,
                Unit::Char('f' | 'F') => 3,
                Unit::Char('p' | 'P') => 4,
                Unit::Char('x' | 'X') => 5,
                _ => 255,
            };
            let (number, _) = digits(&mut it)?;
            (ty, Some(number))
        }
    };
    let mut t = Wide::zero();
    t.mul_add(1, ty);
    packed.or_shifted(&t, 8);
    if let Some(number) = number {
        packed.or_shifted(&number, 0);
    }
    Some(Packed::from_limbs(&packed.0))
}

#[cfg(test)]
mod tests {
    use super::{
        Packed, ResolvedVersion, UnityVersion, VersionError, VersionPolicy, resolve_bundle,
        resolve_serialized,
    };
    use crate::raw::RawStr;

    #[test]
    fn the_chains_take_the_files_string_so_a_version_keeps_it_uncopied() {
        // Round 4: they took `&[u8]`, so the `RawStr` a version (or a
        // `NoMatch`) keeps could only be a copy, uncharged, of a string up to
        // 32,767 bytes, made again by every parse of an entry. Package C
        // implements the bodies; these signatures are what lets them share.
        let _: fn(&RawStr) -> Result<UnityVersion, VersionError> = UnityVersion::parse;
        let _: fn(&RawStr, &VersionPolicy) -> Result<ResolvedVersion, VersionError> =
            resolve_bundle;
        let _: fn(
            &RawStr,
            Option<&RawStr>,
            &VersionPolicy,
        ) -> Result<ResolvedVersion, VersionError> = resolve_serialized;
    }

    #[test]
    fn a_versions_heap_is_its_big_packed_integer_only() {
        let file = bytes::Bytes::from(vec![b'7'; 32_767]);
        let shared = UnityVersion {
            raw: RawStr::from_shared(file.clone()),
            packed: Packed::from_u128(1),
        };
        assert_eq!(shared.heap_bytes(), 0, "the raw string is the file's");
        assert_eq!(shared.raw().as_bytes().as_ptr(), file.as_ptr());
        let big = UnityVersion {
            raw: RawStr::default(),
            packed: Packed::from_limbs(&[1, 2, 3]),
        };
        assert_eq!(big.heap_bytes(), 24);
    }

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
    fn a_value_has_one_form_whatever_its_leading_zeros() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let hash = |p: &Packed| {
            let mut h = DefaultHasher::new();
            p.hash(&mut h);
            h.finish()
        };
        // "000…01" sized from its 4,300 digits: 224 limbs, value 1.
        let mut limbs = vec![0_u64; 224];
        limbs[0] = 1;
        let padded = Packed::from_limbs(&limbs);
        let one = Packed::from_u128(1);
        assert_eq!(padded, one);
        assert_eq!(hash(&padded), hash(&one));
        assert_eq!(padded.cmp(&one), std::cmp::Ordering::Equal);
        assert!(padded < Packed::from_u128(2));
        // Two limbs fit u128; a zero top limb is dropped.
        assert_eq!(
            Packed::from_limbs(&[5, 7, 0]),
            Packed::from_u128(5 | (7 << 64))
        );
        assert_eq!(Packed::from_limbs(&[]), Packed::from_u128(0));
        assert_eq!(Packed::from_limbs(&[0, 0, 1]).to_limbs(), [0, 0, 1]);
        assert_eq!(
            Packed::from_u128(u128::MAX).to_limbs(),
            [u64::MAX, u64::MAX]
        );
        assert_eq!(Packed::from_u128(0).to_limbs(), Vec::<u64>::new());
    }

    #[test]
    fn big_packed_values_order_by_magnitude() {
        let small = Packed::from_u128(u128::MAX);
        let big_lo = Packed::from_limbs(&[u64::MAX, 0, 1]);
        let big_hi = Packed::from_limbs(&[0, 1, 1]);
        let bigger = Packed::from_limbs(&[0, 0, 0, 1]);
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
