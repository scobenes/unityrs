//! `sprite::affine::solve` against `numpy.linalg.solve` bit for bit
//! (`docs/plans/extract.md` section 3, item 6): the systems of
//! `copy_triangle`'s shape that `tools/fixtures/sprite_solve.py` lays out,
//! with the solutions numpy 2.4.6 gave on `OpenBLAS`'s Haswell kernels
//! (`tests/fixtures/sprite_solve.json`, written by
//! `tools/fixtures/sprite.py`): each solution's six `f64`s, or a singular
//! system's `LinAlgError`, the shipped port's own.

use serde_json::Value as J;
use unityrs_texture::sprite::affine::solve;

/// The systems `sprite_solve.py` records, and how many are singular.
const SYSTEMS: usize = 2000;
const SINGULAR: usize = 116;

fn corner(p: &J) -> [f64; 2] {
    // Integers within 2^13 either way: exact as `f64`s.
    let c = |i: usize| {
        let v = i32::try_from(p[i].as_i64().unwrap()).unwrap();
        f64::from(v)
    };
    [c(0), c(1)]
}

fn bits(a: &[f64; 6]) -> String {
    use std::fmt::Write;
    a.iter().fold(String::new(), |mut s, v| {
        write!(s, "{:016x}", v.to_bits()).unwrap();
        s
    })
}

#[test]
fn every_solution_is_numpy_s_bit_for_bit() {
    let body: J = serde_json::from_str(include_str!("fixtures/sprite_solve.json")).unwrap();
    assert_eq!(body["openblas_core"], "Haswell");
    let cases = body["cases"].as_array().unwrap();
    assert_eq!(cases.len(), SYSTEMS);
    let (mut singular, mut bad) = (0, Vec::new());
    for (k, case) in cases.iter().enumerate() {
        let src: Vec<[f64; 2]> = case["src"].as_array().unwrap().iter().map(corner).collect();
        let dst: Vec<[f64; 2]> = case["dst"].as_array().unwrap().iter().map(corner).collect();
        // `copy_triangle`'s M and y (`export/SpriteHelper.py:273-287`).
        let row = |p: [f64; 2], left: bool| {
            if left {
                [p[0], p[1], 1.0, 0.0, 0.0, 0.0]
            } else {
                [0.0, 0.0, 0.0, p[0], p[1], 1.0]
            }
        };
        let m = [
            row(dst[0], true),
            row(dst[1], true),
            row(dst[2], true),
            row(dst[0], false),
            row(dst[1], false),
            row(dst[2], false),
        ];
        let y = [
            src[0][0], src[1][0], src[2][0], src[0][1], src[1][1], src[2][1],
        ];
        let have = solve(&m, &y).map(|a| bits(&a));
        let want = case["a"].as_str().map(str::to_owned);
        if want.is_none() {
            singular += 1;
        }
        if have != want {
            bad.push(format!("system {k}: want {want:?}, have {have:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {SYSTEMS} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert_eq!(singular, SINGULAR);
}
