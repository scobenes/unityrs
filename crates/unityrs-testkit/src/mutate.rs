//! G3b's seeded mutator (plan section 6): each builder output is mutated
//! [`PER_INPUT`] times, by three operators, and both dumpers read every
//! mutant.
//!
//! - **Fields.** Every 4- and 8-byte window, at every offset and in both
//!   byte orders, whose value lies in `1..=len` (the file's length) is taken
//!   for a count, a length, a size or an offset: the shapes the builders
//!   write for each such field. Each chosen window becomes two mutants, one
//!   with the field set negative (-1 or the type's minimum) and one huge (the
//!   type's maximum, or 2^30 or 2^40). When an input has more candidates than
//!   [`FIELD_SHARE`] mutants can cover, a seeded sample of them is taken.
//! - **Bit flips.** One to eight random bits.
//! - **Truncation.** The input cut at a random length below its own.
//!
//! The field mutants come first, then the rest of the budget alternates bit
//! flips and truncations. Each input's generator is [`SEED`] mixed with the
//! FNV-1a hash of its name, so a mutant reproduces from its input's name and
//! its index; its label says what was done (`f<offset>.<le|be><4|8><n|h>`,
//! `b<bits>`, `t<length>`).

use crate::rng::Rng;

/// The master seed (recorded in the gate's report).
pub const SEED: u64 = 0x6733_6200_5031_0001;

/// Mutants per input.
pub const PER_INPUT: usize = 200;

/// At most this many of an input's mutants change a field.
pub const FIELD_SHARE: usize = 120;

/// One mutant: what was done, and the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutant {
    /// The operator and its argument.
    pub label: String,
    /// The mutated bytes.
    pub bytes: Vec<u8>,
}

/// A count, length, size or offset field candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Field {
    at: usize,
    width: usize,
    big: bool,
}

fn fnv1a(name: &[u8]) -> u64 {
    name.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

fn read(data: &[u8], f: Field) -> Option<u64> {
    let w = data.get(f.at..f.at.checked_add(f.width)?)?;
    let mut v = 0u64;
    for i in 0..f.width {
        let b = if f.big { w[i] } else { w[f.width - 1 - i] };
        v = (v << 8) | u64::from(b);
    }
    Some(v)
}

fn write(data: &mut [u8], f: Field, v: u64) {
    let bytes = if f.big {
        v.to_be_bytes()
    } else {
        v.to_le_bytes()
    };
    // The field's `width` bytes: the low ones of `v`.
    let src = if f.big {
        bytes.get(8 - f.width..)
    } else {
        bytes.get(..f.width)
    };
    if let (Some(w), Some(src)) = (data.get_mut(f.at..f.at + f.width), src) {
        w.copy_from_slice(src);
    }
}

fn candidates(data: &[u8]) -> Vec<Field> {
    let len = data.len() as u64;
    let mut out = Vec::new();
    for width in [4, 8] {
        for at in 0..data.len().saturating_sub(width - 1) {
            for big in [false, true] {
                let f = Field { at, width, big };
                if read(data, f).is_some_and(|v| (1..=len).contains(&v)) {
                    out.push(f);
                }
            }
        }
    }
    out
}

/// [`PER_INPUT`] mutants of `data`, the builder output named `name`.
#[must_use]
pub fn mutants(name: &[u8], data: &[u8]) -> Vec<Mutant> {
    let mut rng = Rng::new(SEED ^ fnv1a(name));
    let mut fields = candidates(data);
    // A seeded sample when there are more than the field share covers.
    let take = (FIELD_SHARE / 2).min(fields.len());
    for i in 0..take {
        let j = i + rng.below(fields.len() - i);
        fields.swap(i, j);
    }
    fields.truncate(take);
    fields.sort_by_key(|f| (f.at, f.width, f.big));
    let mut out = Vec::with_capacity(PER_INPUT);
    for f in fields {
        let end = if f.big { "be" } else { "le" };
        let (neg, huge): ([u64; 2], [u64; 2]) = if f.width == 4 {
            ([0xFFFF_FFFF, 0x8000_0000], [0x7FFF_FFFF, 1 << 30])
        } else {
            ([u64::MAX, 1 << 63], [i64::MAX as u64, 1 << 40])
        };
        for (tag, pick) in [("n", neg), ("h", huge)] {
            let mut bytes = data.to_vec();
            write(&mut bytes, f, pick[rng.below(2)]);
            out.push(Mutant {
                label: format!("f{:x}.{end}{}{tag}", f.at, f.width),
                bytes,
            });
        }
    }
    let mut flip = true;
    while out.len() < PER_INPUT && !data.is_empty() {
        let mut bytes = data.to_vec();
        let label = if flip {
            let bits = 1 + rng.below(8);
            rng.flip_bits(&mut bytes, bits);
            // Two flips of one bit cancel: flip another until it differs.
            while bytes == data {
                rng.flip_bits(&mut bytes, 1);
            }
            format!("b{bits}")
        } else {
            let n = rng.below(data.len());
            bytes.truncate(n);
            format!("t{n}")
        };
        flip = !flip;
        out.push(Mutant { label, bytes });
    }
    out
}

/// G3b's inputs: every builder output the gate mutates, `(name, bytes)`:
/// the format matrix ([`crate::bundle::matrix`]) and every file of every
/// Environment scene ([`crate::env::scenes`], named `scene-<scene>-<file>`).
#[must_use]
pub fn builder_outputs() -> Vec<(String, Vec<u8>)> {
    let mut out = crate::bundle::matrix();
    for (scene, s) in crate::env::scenes() {
        for (name, bytes) in s.files.into_iter().chain(s.resolver) {
            let name = String::from_utf8_lossy(&name).replace('/', "_");
            out.push((format!("scene-{scene}-{name}"), bytes));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Field, PER_INPUT, builder_outputs, candidates, mutants, read};

    #[test]
    fn mutants_are_seeded_counted_and_change_their_input() {
        let inputs = builder_outputs();
        assert!(inputs.len() > 98, "{}", inputs.len());
        let mut names: Vec<&String> = inputs.iter().map(|(n, _)| n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), inputs.len(), "names repeat");
        for (name, data) in inputs.iter().take(12) {
            let a = mutants(name.as_bytes(), data);
            assert_eq!(a, mutants(name.as_bytes(), data), "{name}: not seeded");
            assert_eq!(a.len(), PER_INPUT, "{name}");
            assert!(
                a.iter().all(|m| m.bytes != *data),
                "{name}: a mutant is its input"
            );
            assert!(
                a.iter().any(|m| m.label.starts_with('f')),
                "{name}: no field"
            );
            assert!(a.iter().any(|m| m.label.starts_with('t')), "{name}");
        }
    }

    #[test]
    fn a_field_is_found_in_either_order_and_width() {
        let mut d = vec![0u8; 24];
        d[3..7].copy_from_slice(&5u32.to_be_bytes());
        d[12..20].copy_from_slice(&9u64.to_le_bytes());
        let c = candidates(&d);
        assert!(c.contains(&Field {
            at: 3,
            width: 4,
            big: true
        }));
        assert!(c.contains(&Field {
            at: 12,
            width: 8,
            big: false
        }));
        assert!(c.contains(&Field {
            at: 12,
            width: 4,
            big: false
        }));
        assert_eq!(
            read(
                &d,
                Field {
                    at: 3,
                    width: 4,
                    big: false
                }
            ),
            Some(0x0500_0000)
        );
    }
}
