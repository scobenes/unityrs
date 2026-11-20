//! The texture blocks are legal by construction (`docs/plans/bench.md`
//! section 2), checked at both scales on every texture the fixtures hold.

use std::collections::BTreeMap;

use unityrs_bench::astc::{self, Class, PATTERN};
use unityrs_bench::blocks::{self, BC6H_MODES, Kind};
use unityrs_bench::textures::{self, FORMATS, Format};
use unityrs_bench::{Scale, seed_of};
use unityrs_testkit::Rng;

const SCALES: [Scale; 2] = [Scale::Gate, Scale::Full];

fn of(kind: fn(Kind) -> bool) -> Vec<&'static Format> {
    FORMATS.iter().filter(|f| kind(f.kind)).collect()
}

/// Every texture's data of `format` at `scale`.
fn each(format: &Format, scale: Scale) -> Vec<Vec<u8>> {
    let (count, side) = textures::dims(format, scale);
    (0..count)
        .map(|i| textures::data(format, i, side).unwrap())
        .collect()
}

/// Every tiled ASTC block is no error block by the LDR decoder's error
/// status, and the class mix is the pattern's, exactly, in every texture.
#[test]
fn astc_tiles_are_legal_blocks_in_the_stated_mix() {
    let formats = of(|k| matches!(k, Kind::Astc(..)));
    assert_eq!(formats.len(), 2);
    for format in formats {
        let Kind::Astc(bw, bh) = format.kind else {
            unreachable!()
        };
        for scale in SCALES {
            for data in each(format, scale) {
                let (_, side) = textures::dims(format, scale);
                let blocks = format.kind.blocks(side, side);
                assert_eq!(data.len(), blocks * 16);
                assert_eq!(blocks % PATTERN.len(), 0, "{}", format.name);
                let errors = unityrs_texture::astc_ldr_error_blocks(bw, bh, &data).unwrap();
                assert_eq!(errors.len(), blocks);
                assert!(errors.iter().all(|e| !e), "{} {scale:?}", format.name);
                let mut mix: BTreeMap<Class, usize> = BTreeMap::new();
                for b in data.as_chunks::<16>().0 {
                    *mix.entry(Class::of(b)).or_default() += 1;
                }
                for class in Class::ALL {
                    let want = blocks / PATTERN.len() * astc::share(class);
                    assert_eq!(mix.get(&class).copied().unwrap_or(0), want, "{class:?}");
                }
            }
        }
    }
}

/// The pool is a choice: the recorded cases hold error blocks too, which
/// the decoder's status keeps out, so the legality check above has work to
/// do.
#[test]
fn the_recorded_cases_hold_error_blocks_the_pool_leaves_out() {
    for (bw, bh) in [(4, 4), (8, 8)] {
        let recorded = astc::recorded(bw, bh).unwrap();
        let flat: Vec<u8> = recorded.iter().flatten().copied().collect();
        let errors = unityrs_texture::astc_ldr_error_blocks(bw, bh, &flat).unwrap();
        let bad: Vec<_> = recorded.iter().zip(&errors).filter(|(_, e)| **e).collect();
        assert_ne!(bad.len(), 0);
        let pool: Vec<[u8; 16]> = astc::pool(bw, bh).unwrap().concat();
        assert!(bad.iter().all(|(b, _)| !pool.contains(b)));
        assert_eq!(
            Class::ALL.map(astc::share).iter().sum::<usize>(),
            PATTERN.len()
        );
    }
}

/// ETC1: every differential block's base plus delta is within 0..31, and
/// the generator moves an out-of-range delta rather than keeping it.
#[test]
fn etc1_blocks_have_legal_differential_endpoints() {
    let formats = of(|k| k == Kind::Etc1);
    assert_eq!(formats.len(), 1);
    let mut differential = 0;
    for scale in SCALES {
        for data in each(formats[0], scale) {
            for b in data.as_chunks::<8>().0 {
                assert!(blocks::etc1_legal(b), "{b:02x?}");
                differential += usize::from(b[3] & 2 != 0);
            }
        }
    }
    assert!(differential > 0);
    // Base 0 with delta -4, base 31 with delta +3: both out of range.
    let illegal = [0x04, 0xfb, 0x00, 0x02, 0, 0, 0, 0];
    assert!(!blocks::etc1_legal(&illegal));
    let mut rng = Rng::new(seed_of("etc1-probe"));
    let fixed: Vec<[u8; 8]> = (0..4096).map(|_| blocks::etc1_block(&mut rng)).collect();
    assert!(fixed.iter().all(blocks::etc1_legal));
}

/// BC7: modes 1, 5 and 6 only, at 40%, 30% and 30% within a point.
#[test]
fn bc7_blocks_use_modes_1_5_and_6_in_proportion() {
    let formats = of(|k| k == Kind::Bc7);
    for scale in SCALES {
        for data in each(formats[0], scale) {
            let blocks = data.len() / 16;
            let mut mix: BTreeMap<u8, usize> = BTreeMap::new();
            for b in data.as_chunks::<16>().0 {
                *mix.entry(blocks::bc7_mode(b).unwrap()).or_default() += 1;
            }
            assert_eq!(mix.keys().copied().collect::<Vec<_>>(), [1, 5, 6]);
            for (mode, pct) in [(1, 40), (5, 30), (6, 30)] {
                let got = mix[&mode] * 100;
                assert!(
                    got.abs_diff(pct * blocks) < blocks,
                    "mode {mode}: {got} / {blocks}"
                );
            }
        }
    }
}

/// BC6H: every block's mode is legal (the decoder refuses the reserved
/// ones), and every legal mode occurs.
#[test]
fn bc6h_blocks_use_legal_modes_only() {
    let formats = of(|k| k == Kind::Bc6h);
    let mut seen = vec![false; BC6H_MODES.len()];
    for scale in SCALES {
        for data in each(formats[0], scale) {
            for b in data.as_chunks::<16>().0 {
                assert!(unityrs_texture::decode_bc6h_block(b, false).is_ok());
                let at = BC6H_MODES
                    .iter()
                    .position(|(bits, value)| b[0] & ((1u8 << bits) - 1) == *value);
                seen[at.unwrap()] = true;
            }
        }
    }
    assert!(seen.iter().all(|s| *s));
    // A reserved mode (0b10011) is refused, so the check above can fail.
    let mut reserved = [0u8; 16];
    reserved[0] = 0b10011;
    assert!(unityrs_texture::decode_bc6h_block(&reserved, false).is_err());
}

/// Every format's data is the size its blocks take, and the formats the
/// plan names are all here, raw ones 4 at full scale and the rest 8.
#[test]
fn every_format_has_its_count_and_size() {
    let names: Vec<_> = FORMATS.iter().map(|f| f.name).collect();
    assert_eq!(names.len(), 16);
    for format in &FORMATS {
        let raw = matches!(format.kind, Kind::Raw(_));
        assert_eq!(format.full, if raw { 4 } else { 8 }, "{}", format.name);
        for scale in SCALES {
            let (count, side) = textures::dims(format, scale);
            assert_eq!(count, scale.pick(format.full, 2));
            let data = textures::data(format, 0, side).unwrap();
            assert_eq!(
                data.len(),
                format.kind.blocks(side, side) * format.kind.block_bytes()
            );
        }
    }
}
