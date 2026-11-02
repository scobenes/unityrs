//! G3b's seeded mutator (plan section 6): every builder output, mutated by
//! three operators, and both dumpers read every mutant.
//!
//! - **Fields.** Every field the build of an output writes into it (each
//!   4- and 8-byte integer, [`crate::writer`]: every count, length, size and
//!   offset, zero-valued ones included, and every other integer of those
//!   widths) becomes two mutants: the field set negative (-1 or the type's
//!   minimum) and set huge (the type's maximum, or 2^30 or 2^40). The
//!   output is built again with that one field replaced as it is written
//!   ([`crate::writer::with_fields`]), so a field inside a compressed block,
//!   or in a file a bundle holds, is changed before the block is compressed
//!   and the bundle's sizes are computed. A field belongs to the outputs
//!   whose bytes change when it alone is replaced; one that changes no
//!   output (a build kept only for its length) is no output's.
//! - **Bit flips.** One to eight random bits.
//! - **Truncation.** The input cut at a random length below its own.
//!
//! Each output's field mutants come first, in write order, then
//! [`RANDOM_PER_INPUT`] mutants alternate bit flips and truncations. Each
//! output's generator is [`SEED`] mixed with the FNV-1a hash of its name, and
//! picks each field's value from its pair; a mutant's label says what was
//! done (`f<field>.<le|be><4|8><n|h>`, the field's number among the output's
//! own, `b<bits>`, `t<length>`). The mutants depend only on the builders'
//! code, never on how many threads made them.

use std::collections::HashMap;

use crate::rng::Rng;
use crate::writer::{Field, with_fields};

/// The master seed (recorded in the gate's report).
pub const SEED: u64 = 0x6733_6200_5031_0001;

/// Bit flips and truncations per output, after its field mutants.
pub const RANDOM_PER_INPUT: usize = 80;

/// One mutant: what was done, and the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutant {
    /// The operator and its argument.
    pub label: String,
    /// The mutated bytes.
    pub bytes: Vec<u8>,
}

/// Named outputs, in build order.
pub type Outputs = Vec<(String, Vec<u8>)>;

fn fnv1a(name: &[u8]) -> u64 {
    name.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// All the field's bits set.
fn mask(f: &Field) -> u64 {
    if f.width >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * f.width)) - 1
    }
}

/// The negative pair and the huge pair for a field of `width` bytes.
fn pairs(width: usize) -> ([u64; 2], [u64; 2]) {
    if width == 4 {
        ([0xFFFF_FFFF, 0x8000_0000], [0x7FFF_FFFF, 1 << 30])
    } else {
        ([u64::MAX, 1 << 63], [i64::MAX.unsigned_abs(), 1 << 40])
    }
}

/// `run` over `jobs` on up to `threads` threads; result `i` is `run(&jobs[i])`.
fn parallel<J: Sync, R: Send>(
    jobs: &[J],
    threads: usize,
    run: &(dyn Fn(&J) -> R + Sync),
) -> Vec<R> {
    let threads = threads.clamp(1, jobs.len().max(1));
    if threads == 1 {
        return jobs.iter().map(run).collect();
    }
    let mut all: Vec<(usize, R)> = Vec::with_capacity(jobs.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                s.spawn(move || {
                    jobs.iter()
                        .enumerate()
                        .skip(t)
                        .step_by(threads)
                        .map(|(i, j)| (i, run(j)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            // A builder that panics is a bug in the builders: pass it on.
            match h.join() {
                Ok(part) => all.extend(part),
                Err(e) => std::panic::resume_unwind(e),
            }
        }
    });
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, r)| r).collect()
}

/// The outputs of a build with `overrides`, by name.
fn rebuild(
    build: &(dyn Fn() -> Outputs + Sync),
    overrides: &[(usize, u64)],
) -> HashMap<String, Vec<u8>> {
    with_fields(overrides, build).0.into_iter().collect()
}

/// One field mutant to make: an output, a field, its value, the label.
struct Planned {
    output: usize,
    field: usize,
    value: u64,
    label: String,
}

/// Every output of `build` with its mutants (module docs), the work spread
/// over `threads` threads. `build` must give the same outputs, under names
/// that do not repeat, each time it runs.
#[must_use]
pub fn mutants_of(
    build: &(dyn Fn() -> Outputs + Sync),
    threads: usize,
) -> Vec<(String, Vec<Mutant>)> {
    let (base, fields) = with_fields(&[], build);
    // The outputs each field changes when it alone is replaced by its
    // complement.
    let ids: Vec<usize> = (0..fields.len()).collect();
    let owners: Vec<Vec<usize>> = parallel(&ids, threads, &|k| {
        let f = &fields[*k];
        let again = rebuild(build, &[(*k, !f.value & mask(f))]);
        base.iter()
            .enumerate()
            .filter(|(_, (name, bytes))| again.get(name).is_some_and(|b| b != bytes))
            .map(|(o, _)| o)
            .collect()
    });
    let mut own: Vec<Vec<usize>> = vec![Vec::new(); base.len()];
    for (k, os) in owners.iter().enumerate() {
        for o in os {
            own[*o].push(k);
        }
    }
    // Each output's field values come from its own generator, which then
    // drives its random operators.
    let mut rngs: Vec<Rng> = base
        .iter()
        .map(|(name, _)| Rng::new(SEED ^ fnv1a(name.as_bytes())))
        .collect();
    let mut planned: Vec<Vec<Planned>> = Vec::with_capacity(base.len());
    for (o, ks) in own.iter().enumerate() {
        let mut row = Vec::with_capacity(2 * ks.len());
        for (i, k) in ks.iter().enumerate() {
            let f = &fields[*k];
            let end = if f.big_endian { "be" } else { "le" };
            let (neg, huge) = pairs(f.width);
            for (tag, pair) in [("n", neg), ("h", huge)] {
                let pick = rngs[o].below(2);
                // A field already at the picked value takes the other one.
                let value = if pair[pick] == f.value {
                    pair[1 - pick]
                } else {
                    pair[pick]
                };
                row.push(Planned {
                    output: o,
                    field: *k,
                    value,
                    label: format!("f{i}.{end}{}{tag}", f.width),
                });
            }
        }
        planned.push(row);
    }
    // A field one output alone holds is replaced together with one such
    // field of each other output (no output sees another's: [`make`]); a
    // field that several outputs hold is replaced on its own.
    let rounds = planned.iter().map(Vec::len).max().unwrap_or(0);
    let mut batches: Vec<Vec<&Planned>> = (0..rounds)
        .map(|r| {
            planned
                .iter()
                .filter_map(|row| row.get(r))
                .filter(|p| owners[p.field].len() == 1)
                .collect()
        })
        .collect();
    batches.extend(
        planned
            .iter()
            .flatten()
            .filter(|p| owners[p.field].len() > 1)
            .map(|p| vec![p]),
    );
    let made: Vec<Vec<(usize, usize, Mutant)>> = parallel(&batches, threads, &|batch| {
        make(build, &base, &fields, &planned, batch)
    });
    let mut by_output: Vec<Vec<(usize, Mutant)>> = vec![Vec::new(); base.len()];
    for (o, order, m) in made.into_iter().flatten() {
        by_output[o].push((order, m));
    }
    base.into_iter()
        .zip(by_output)
        .zip(rngs.iter_mut())
        .map(|(((name, data), mut fm), rng)| {
            fm.sort_by_key(|(order, _)| *order);
            let mut out: Vec<Mutant> = fm.into_iter().map(|(_, m)| m).collect();
            out.extend(random_mutants(&data, rng));
            (name, out)
        })
        .collect()
}

/// The mutants `batch` makes, `(output, order, mutant)`, its fields replaced
/// in one build. A replaced field whose value changes how many fields its
/// build writes after it (the LZ4-compressed length of a nested bundle sets
/// how many blocks the bundle holding it has) numbers every later field
/// otherwise, so the batch's later replacements would land on other fields
/// than planned, of other outputs or of none: a batch whose build writes
/// fields of other widths, or another count of them, than the plain build,
/// or gives a replaced field another value, is made one field at a time.
fn make(
    build: &(dyn Fn() -> Outputs + Sync),
    base: &Outputs,
    fields: &[Field],
    planned: &[Vec<Planned>],
    batch: &[&Planned],
) -> Vec<(usize, usize, Mutant)> {
    let overrides: Vec<(usize, u64)> = batch.iter().map(|p| (p.field, p.value)).collect();
    let (outputs, written) = with_fields(&overrides, build);
    let shape = |f: &Field| (f.width, f.big_endian);
    let numbered = written.len() == fields.len()
        && written.iter().map(shape).eq(fields.iter().map(shape))
        && batch
            .iter()
            .all(|p| written.get(p.field) == fields.get(p.field));
    if batch.len() > 1 && !numbered {
        return batch
            .iter()
            .flat_map(|p| make(build, base, fields, planned, &[*p]))
            .collect();
    }
    let again: HashMap<String, Vec<u8>> = outputs.into_iter().collect();
    batch
        .iter()
        .filter_map(|p| {
            let (name, bytes) = &base[p.output];
            // A replacement that leaves the output as it was is no mutant.
            let got = again.get(name).filter(|b| *b != bytes)?;
            let order = planned[p.output].iter().position(|q| q.label == p.label)?;
            Some((
                p.output,
                order,
                Mutant {
                    label: p.label.clone(),
                    bytes: got.clone(),
                },
            ))
        })
        .collect()
}

/// [`RANDOM_PER_INPUT`] mutants of `data`, bit flips and truncations by turns.
fn random_mutants(data: &[u8], rng: &mut Rng) -> Vec<Mutant> {
    let mut out = Vec::with_capacity(RANDOM_PER_INPUT);
    let mut flip = true;
    while out.len() < RANDOM_PER_INPUT && !data.is_empty() {
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
/// Environment scene, those some of whose loads are dependencies, those
/// `find_file` is recorded on and those whose walk races another load
/// (its file among the scene's) included ([`crate::env::scenes`],
/// [`crate::env::dependency_scenes`], [`crate::env::find_scenes`],
/// [`crate::env::race_scenes`]; named `scene-<scene>-<file>`), but a
/// WebFile ([`is_web_file`]): P1 refuses one (plan section 1), so the dump
/// writes no record of the entries UnityPy reads from it.
#[must_use]
pub fn builder_outputs() -> Outputs {
    let mut out = crate::bundle::matrix();
    for (scene, s) in every_scene() {
        for (name, bytes) in s.files.into_iter().chain(s.resolver) {
            if is_web_file(&bytes) {
                continue;
            }
            let name = String::from_utf8_lossy(&name).replace('/', "_");
            out.push((format!("scene-{scene}-{name}"), bytes));
        }
    }
    out
}

/// Every Environment scene of every list, by name, a race scene's other
/// file among its files.
fn every_scene() -> Vec<(&'static str, crate::env::Scene)> {
    let dependency = crate::env::dependency_scenes()
        .into_iter()
        .map(|(scene, d)| (scene, d.scene));
    let find = crate::env::find_scenes()
        .into_iter()
        .map(|(scene, f)| (scene, f.scene));
    let race = crate::env::race_scenes().into_iter().map(|(scene, r)| {
        let mut s = r.scene;
        s.files.push(r.other);
        (scene, s)
    });
    crate::env::scenes()
        .into_iter()
        .chain(dependency)
        .chain(find)
        .chain(race)
        .collect()
}

/// Whether `bytes` are a WebFile by UnityPy's detection: a signature that
/// starts `UnityWebData` or `TuanjieWebData` (`docs/CANONICAL_DUMP.md`
/// section 4.2, `l0`'s rule 4).
#[must_use]
pub fn is_web_file(bytes: &[u8]) -> bool {
    bytes.starts_with(b"UnityWebData") || bytes.starts_with(b"TuanjieWebData")
}

/// G3b's mutants: [`mutants_of`] the [`builder_outputs`].
#[must_use]
pub fn gate_mutants(threads: usize) -> Vec<(String, Vec<Mutant>)> {
    mutants_of(&builder_outputs, threads)
}

#[cfg(test)]
mod tests {
    use super::{Outputs, RANDOM_PER_INPUT, builder_outputs, mutants_of};
    use crate::bundle::{TestCodec, compress};
    use crate::writer::{ByteWriter, with_fields};

    /// A count, a `u16` (no field) and a big-endian length.
    fn body(count: u32) -> Vec<u8> {
        let mut w = ByteWriter::le();
        w.u32(count)
            .u16(0xAAAA)
            .set_big_endian(true)
            .u64(3)
            .bytes(b"xyz");
        w.into_inner()
    }

    /// A zero count and a length, plain; the same inside an LZ4 block whose
    /// stored size follows it; and a build kept only for its length.
    fn small() -> Outputs {
        let plain = body(0);
        let packed = compress(TestCodec::Lz4, &body(3));
        let mut w = ByteWriter::be();
        w.u32(u32::try_from(packed.len()).unwrap()).bytes(&packed);
        let wrapped = w.into_inner();
        let only_its_length = body(9).len();
        vec![
            ("plain".into(), plain),
            ("wrapped".into(), wrapped),
            ("sized".into(), vec![0; only_its_length]),
        ]
    }

    #[test]
    fn every_field_of_every_output_is_set_negative_and_huge() {
        let all = mutants_of(&small, 1);
        let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["plain", "wrapped", "sized"]);
        let labels = |o: usize| -> Vec<&str> {
            all[o]
                .1
                .iter()
                .filter(|m| m.label.starts_with('f'))
                .map(|m| m.label.as_str())
                .collect()
        };
        // The zero count and the big-endian length, each both ways.
        assert_eq!(labels(0), ["f0.le4n", "f0.le4h", "f1.be8n", "f1.be8h"]);
        // The two inside the block, then the block's stored size, written last.
        assert_eq!(
            labels(1),
            [
                "f0.le4n", "f0.le4h", "f1.be8n", "f1.be8h", "f2.be4n", "f2.be4h"
            ]
        );
        // The third output holds no field of the build it was sized by.
        assert_eq!(labels(2), Vec::<&str>::new());
        // The zero count set negative, the rest unchanged.
        let base = small();
        let m = &all[0].1[0];
        assert!(
            [[0xFF; 4], [0, 0, 0, 0x80]].contains(&<[u8; 4]>::try_from(&m.bytes[..4]).unwrap())
        );
        assert_eq!(m.bytes[4..], base[0].1[4..]);
        // The count inside the block is replaced before it is compressed.
        let w = &all[1].1[1];
        let size = usize::try_from(u32::from_be_bytes(w.bytes[..4].try_into().unwrap())).unwrap();
        let inner = lz4_flex::block::decompress(&w.bytes[4..4 + size], 17).unwrap();
        let count = u32::from_le_bytes(inner[..4].try_into().unwrap());
        assert!([0x7FFF_FFFF, 1 << 30].contains(&count), "{count:#x}");
        assert_eq!(inner[4..], body(3)[4..]);
        // Then the random operators.
        for (_, ms) in &all {
            let random = ms.iter().filter(|m| !m.label.starts_with('f')).count();
            assert_eq!(random, RANDOM_PER_INPUT);
        }
    }

    /// Two outputs: the first holds sixteen zero counts LZ4-packed, then a
    /// field per byte of the packing, so replacing a count changes how many
    /// fields it writes, as a nested bundle's packed length sets how many
    /// blocks the bundle holding it has.
    fn renumbering() -> Outputs {
        let mut counts = ByteWriter::le();
        for _ in 0..16 {
            counts.u32(0);
        }
        let packed = compress(TestCodec::Lz4, &counts.into_inner());
        let mut per_byte = ByteWriter::le();
        for _ in 0..packed.len() {
            per_byte.u32(1);
        }
        let mut second = ByteWriter::le();
        second.u32(2).u32(3);
        vec![
            ("first".into(), [packed, per_byte.into_inner()].concat()),
            ("second".into(), second.into_inner()),
        ]
    }

    /// A field whose replacement renumbers the fields after it is not
    /// replaced together with another output's field (review round 4: the
    /// batch replaced whatever field took the other's number, so the second
    /// output's mutants were lost or replaced another field than labelled).
    #[test]
    fn a_field_that_renumbers_later_fields_is_replaced_alone() {
        let base = renumbering();
        let all = mutants_of(&renumbering, 1);
        let fields = |o: usize| all[o].1.iter().filter(|m| m.label.starts_with('f'));
        let second: Vec<&str> = fields(1).map(|m| m.label.as_str()).collect();
        assert_eq!(second, ["f0.le4n", "f0.le4h", "f1.le4n", "f1.le4h"]);
        for m in fields(1) {
            // The one field its label names replaced, the other as built.
            let at = if m.label.starts_with("f0") { 0 } else { 4 };
            let keep = 4 - at;
            assert_eq!(
                m.bytes[keep..keep + 4],
                base[1].1[keep..keep + 4],
                "{}",
                m.label
            );
            assert_ne!(m.bytes[at..at + 4], base[1].1[at..at + 4], "{}", m.label);
        }
        let (_, written) = with_fields(&[], renumbering);
        assert_eq!(fields(0).count(), 2 * (written.len() - 2));
    }

    #[test]
    fn the_mutants_do_not_depend_on_the_threads() {
        assert_eq!(mutants_of(&small, 1), mutants_of(&small, 3));
    }

    #[test]
    fn the_gate_mutates_every_field_of_its_outputs() {
        let inputs = builder_outputs();
        assert!(inputs.len() > 98, "{}", inputs.len());
        let mut names: Vec<&String> = inputs.iter().map(|(n, _)| n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), inputs.len(), "names repeat");
        // The builders give the same bytes and fields each time, which the
        // attribution of fields to outputs rests on.
        assert_eq!(
            with_fields(&[], builder_outputs),
            with_fields(&[], builder_outputs)
        );
        let all = super::gate_mutants(4);
        assert_eq!(all.len(), inputs.len());
        let mut no_field = Vec::new();
        for ((name, data), (n, ms)) in inputs.iter().zip(&all) {
            assert_eq!(name, n);
            assert!(
                ms.iter().all(|m| m.bytes != *data),
                "{name}: a mutant is its input"
            );
            let fields = ms.iter().filter(|m| m.label.starts_with('f')).count();
            assert_eq!(fields % 2, 0, "{name}: {fields} field mutants");
            if fields == 0 {
                no_field.push(name.as_str());
            }
            assert_eq!(ms.len(), fields + RANDOM_PER_INPUT, "{name}");
        }
        // Only a resource file of raw bytes holds no field.
        assert_eq!(
            no_field,
            [
                "scene-resolver_loads-CAB-H2.resS",
                "scene-find_file_kinds-top.resS",
                "scene-find_file_kinds-r.resS",
                "scene-find_file_listing-x.resS"
            ]
        );
    }

    /// Every file of every Environment scene is an output, a dependency
    /// scene's, a `find_file` scene's and a race scene's included (fix
    /// passes 6 and 11 each added the first scene of a list of its own,
    /// which G3b's outputs left out), but the one WebFile, which P1 refuses.
    #[test]
    fn every_file_of_every_scene_is_an_output() {
        let inputs = builder_outputs();
        let (mut files, mut web) = (0, Vec::new());
        // Each race scene's other file, by the lists themselves.
        let race = crate::env::race_scenes();
        assert_ne!(race.len(), 0);
        for (scene, r) in race {
            let name = String::from_utf8_lossy(&r.other.0).replace('/', "_");
            let name = format!("scene-{scene}-{name}");
            assert!(
                inputs.iter().any(|(n, b)| *n == name && *b == r.other.1),
                "{name} is no output"
            );
        }
        for (scene, s) in super::every_scene() {
            for (name, bytes) in s.files.iter().chain(&s.resolver) {
                let name = String::from_utf8_lossy(name).replace('/', "_");
                let name = format!("scene-{scene}-{name}");
                if super::is_web_file(bytes) {
                    web.push(name);
                    continue;
                }
                assert!(
                    inputs.iter().any(|(n, b)| *n == name && b == bytes),
                    "{name} is no output"
                );
                files += 1;
            }
        }
        assert_eq!(web, ["scene-find_file_kinds-w.web"]);
        assert_eq!(inputs.len(), crate::bundle::matrix().len() + files);
    }
}
