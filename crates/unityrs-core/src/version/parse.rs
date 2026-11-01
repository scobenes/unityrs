//! `UnityVersion.from_str`'s pattern and `from_list`'s packing, each step's
//! work counted before it runs (the parent module's docs, "Work").

use std::iter::Peekable;

use super::Packed;
use super::packed::significant;
use crate::error::Error;
use crate::limits::{Charge, heap_cost};

/// The work counted for each limb a step multiplies or merges: a limb's 8
/// bytes, as a bundle read counts 8 bytes a step (and as the dump counts
/// each limb its decimal conversion divides).
pub const LIMB_WORK: u64 = 8;

/// Limbs of a packed version: 4,300 decimal digits are 14,284 bits (224
/// limbs), shifted by at most 48 more.
const LIMBS: usize = 226;

/// The most limbs a packed version a parse makes holds
/// ([`super::Packed::big_limbs`]): every step stays within the 226-limb
/// scratch it packs in, so a writer can convert any input's version in a
/// scratch of its own of this size (`unityrs_dump::json::write_packed`).
pub const MAX_LIMBS: usize = LIMBS;

/// 10^19, the most digits a `u64` chunk takes.
const CHUNK: u64 = 10_000_000_000_000_000_000;

/// A non-negative integer of up to [`LIMBS`] little-endian limbs, on the
/// stack, so parsing allocates nothing but a big result; `len` limbs are in
/// use (the rest are zero), and a step touches only those.
struct Wide {
    limbs: [u64; LIMBS],
    len: usize,
}

impl Wide {
    fn zero() -> Self {
        Self {
            limbs: [0; LIMBS],
            len: 0,
        }
    }

    /// Back to zero, clearing only the limbs in use.
    fn clear(&mut self) {
        self.limbs.iter_mut().take(self.len).for_each(|l| *l = 0);
        self.len = 0;
    }

    /// The limbs in use.
    fn used(&self) -> &[u64] {
        self.limbs.get(..self.len).unwrap_or_default()
    }

    /// `self = self * mul + add`: the limbs in use and the one a carry
    /// reaches (a `len`-limb value times a `u64`, plus one, fits `len + 1`
    /// limbs), counted first.
    fn mul_add(&mut self, mul: u64, add: u64, work: &mut Work<'_>) -> Result<(), Error> {
        let n = (self.len + 1).min(LIMBS);
        work.step(n)?;
        let mut carry = u128::from(add);
        for l in self.limbs.iter_mut().take(n) {
            let cur = u128::from(*l) * u128::from(mul) + carry;
            *l = u64::try_from(cur & u128::from(u64::MAX)).unwrap_or(0);
            carry = cur >> 64;
        }
        self.len = significant(self.limbs.get(..n).unwrap_or_default());
        Ok(())
    }

    /// `self |= other << shift`, for a `shift` below 64: each limb `other`
    /// uses, counted first.
    fn or_shifted(&mut self, other: &Self, shift: u32, work: &mut Work<'_>) -> Result<(), Error> {
        work.step(other.len)?;
        let bits = shift % 64;
        for (i, l) in other.used().iter().enumerate() {
            if let Some(dst) = self.limbs.get_mut(i) {
                *dst |= l << bits;
            }
            if bits > 0
                && let Some(dst) = self.limbs.get_mut(i + 1)
            {
                *dst |= l >> (64 - bits);
            }
        }
        let reach = (other.len + 1).max(self.len).min(LIMBS);
        self.len = significant(self.limbs.get(..reach).unwrap_or_default());
        Ok(())
    }
}

/// A parse's work: the bytes it scanned since its last count, counted with
/// its next step, and each step's limbs ([`LIMB_WORK`] each), counted
/// before the step runs. With no charge, nothing is counted (a caller's own
/// string, [`super::UnityVersion::parse`]).
pub(super) struct Work<'a> {
    charge: Option<&'a mut Charge>,
    scanned: u64,
}

impl<'a> Work<'a> {
    pub(super) fn new(charge: Option<&'a mut Charge>) -> Self {
        Self { charge, scanned: 0 }
    }

    fn scan(&mut self, bytes: usize) {
        self.scanned = self
            .scanned
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    /// Counts what was scanned and `limbs` limbs about to be stepped
    /// through; refused, the scanned bytes count all the same (they were
    /// read), and the step does not run.
    fn step(&mut self, limbs: usize) -> Result<(), Error> {
        let todo = u64::try_from(limbs)
            .unwrap_or(u64::MAX)
            .saturating_mul(LIMB_WORK);
        let done = std::mem::take(&mut self.scanned);
        let Some(charge) = self.charge.as_deref().filter(|_| done > 0 || todo > 0) else {
            return Ok(());
        };
        let meter = charge.meter();
        meter.count_work(done.saturating_add(todo)).map_err(|e| {
            meter.add_work(done);
            Error::Limit(e)
        })
    }

    /// Counts what was scanned since the last step.
    fn finish(&mut self) -> Result<(), Error> {
        self.step(0)
    }

    /// Charges a big packed integer's limbs before they are allocated.
    fn charge_heap(&mut self, limbs: usize) -> Result<(), Error> {
        let Some(charge) = self.charge.as_deref_mut() else {
            return Ok(());
        };
        let bytes = u64::try_from(limbs.saturating_mul(8)).unwrap_or(u64::MAX);
        Ok(charge.grow(heap_cost(bytes))?)
    }
}

/// One unit of the string Python sees: a code point, or a byte that is not
/// valid UTF-8 (a lone surrogate under `surrogateescape`).
#[derive(Clone, Copy)]
enum Unit {
    Char(char),
    Byte,
}

impl Unit {
    /// The bytes it takes in the raw string.
    fn len(self) -> usize {
        match self {
            Self::Char(c) => c.len_utf8(),
            Self::Byte => 1,
        }
    }
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

/// The next unit of `it`, scanned.
fn next<I: Iterator<Item = Unit>>(it: &mut Peekable<I>, work: &mut Work<'_>) -> Option<Unit> {
    let u = it.next()?;
    work.scan(u.len());
    Some(u)
}

/// A run of `\d` from `it` into `value`, through Python's `int()`: `None`
/// for more than [`super::PY_INT_MAX_STR_DIGITS`] digits; else the digit
/// count. Each 19 digits are one step, over the limbs `value` uses.
fn digits<I: Iterator<Item = Unit>>(
    it: &mut Peekable<I>,
    value: &mut Wide,
    work: &mut Work<'_>,
) -> Result<Option<usize>, Error> {
    value.clear();
    let (mut count, mut chunk, mut chunk_len) = (0usize, 0u64, 0u32);
    while let Some(d) = it.peek().copied().and_then(digit) {
        next(it, work);
        count += 1;
        if count > super::PY_INT_MAX_STR_DIGITS {
            return Ok(None);
        }
        chunk = chunk * 10 + u64::from(d);
        chunk_len += 1;
        if chunk_len == 19 {
            value.mul_add(CHUNK, chunk, work)?;
            (chunk, chunk_len) = (0, 0);
        }
    }
    if chunk_len > 0 {
        value.mul_add(10_u64.pow(chunk_len), chunk, work)?;
    }
    Ok(Some(count))
}

/// `UnityVersion.from_str` (`helpers/UnityVersion.py:62-88`): the pattern
/// `^(\d+)\.(\d+)\.(\d+)(.+?)?(\d+)?(.*)$` under DOTALL, whose lazy type
/// group takes exactly the one character after `build`'s digits whenever
/// there is one (the greedy `build` leaves a non-digit there and the rest
/// always matches), then `from_list`'s unbounded OR. `Ok(None)` where it
/// raises `ValueError`; an error where `work` refuses a step, or the limbs
/// of a packed integer of 2^128 or more.
pub(super) fn parse_packed(raw: &[u8], mut work: Work<'_>) -> Result<Option<Packed>, Error> {
    let got = pack(raw, &mut work);
    let rest = work.finish();
    let packed = got?;
    rest?;
    let Some(packed) = packed else {
        return Ok(None);
    };
    if packed.len > 2 {
        work.charge_heap(packed.len)?;
    }
    Ok(Some(Packed::from_limbs(packed.used())))
}

fn pack(raw: &[u8], work: &mut Work<'_>) -> Result<Option<Wide>, Error> {
    let mut it = units(raw).peekable();
    let mut packed = Wide::zero();
    let mut group = Wide::zero();
    for (i, shift) in [48u32, 32, 16].into_iter().enumerate() {
        match digits(&mut it, &mut group, work)? {
            None | Some(0) => return Ok(None),
            Some(_) => {}
        }
        if i < 2 && !matches!(next(&mut it, work), Some(Unit::Char('.'))) {
            return Ok(None);
        }
        packed.or_shifted(&group, shift, work)?;
    }
    // No type character packs as `f` (3); `getattr(UnityVersionType,
    // c.lower(), u)` names a type only for these six letters (no other code
    // point lowers to one of them).
    let ty = match next(&mut it, work) {
        None => 3u64,
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
            if digits(&mut it, &mut group, work)?.is_none() {
                return Ok(None);
            }
            packed.or_shifted(&group, 0, work)?;
            ty
        }
    };
    group.clear();
    group.mul_add(1, ty, work)?;
    packed.or_shifted(&group, 8, work)?;
    Ok(Some(packed))
}
