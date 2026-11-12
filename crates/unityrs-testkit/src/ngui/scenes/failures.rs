//! The scenes whose inputs fail (`docs/BATCH_SPEC.md` 7.3's `inputs` rows,
//! `docs/plans/batch.md` 2.6 and 2.7): a member that does not open or is no
//! bundle, an input that is no bundle, and an input whose top-level row
//! fails at the reach or at selection's open. The reference records an
//! input's results, not its failure, so these are listed apart from
//! [`super::scenes`], in [`failure_scenes`], for the cutter's own tests,
//! which take their answers from the spec.

use super::super::{BuildError, file, mono_script};
use super::{AtlasScene, Built, CutOptions, by_name, one, plain, rgba, scene};
use crate::env::bundle;

/// A bundle of `cab`, cut to its first 48 bytes: its signature and the head
/// of its header, so a peek of 21 bytes takes it for a bundle and its open
/// fails.
fn broken_bundle() -> Vec<u8> {
    let mut b = bundle(&[("CAB-n", vec![0; 64])]);
    b.truncate(48);
    b
}

fn cab(id: &str) -> Result<Vec<u8>, BuildError> {
    file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, id, 0xF0, 1, 1),
            by_name(3, id, vec![plain("s", (0, 0, 1, 1))]),
        ],
        true,
    )
}

/// `x.bundle`, depending on `d.bundle`, whose bytes do not open:
/// `dependency_failed(d.bundle, container_error)`.
pub fn member_fails() -> Built {
    Ok(scene(
        vec![
            one("x.bundle", &[("CAB-x", cab("x")?)], &["d.bundle"]),
            super::SceneBundle {
                id: "d.bundle".to_owned(),
                data: broken_bundle(),
                deps: Vec::new(),
            },
        ],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// `x.bundle`, depending on `d.bundle`, a SerializedFile and no bundle:
/// `dependency_failed(d.bundle, not_a_bundle)`.
pub fn member_not_a_bundle() -> Built {
    Ok(scene(
        vec![
            one("x.bundle", &[("CAB-x", cab("x")?)], &["d.bundle"]),
            super::SceneBundle {
                id: "d.bundle".to_owned(),
                data: cab("d")?,
                deps: Vec::new(),
            },
        ],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// `x.bundle`, a SerializedFile and no bundle: `not_a_bundle`.
pub fn input_not_a_bundle() -> Built {
    Ok(scene(
        vec![super::SceneBundle {
            id: "x.bundle".to_owned(),
            data: cab("x")?,
            deps: Vec::new(),
        }],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// `x.bundle` with a top-level row `CAB-n` that peeks as a bundle and does
/// not open: the reach of the input's containers fails, its `entry_error`.
pub fn reach_fails() -> Built {
    Ok(scene(
        vec![one(
            "x.bundle",
            &[("CAB-x", cab("x")?), ("CAB-n", broken_bundle())],
            &[],
        )],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// `x.bundle` with two top-level rows `CAB-n`, the first a bundle that does
/// not open and the second raw bytes: the reach takes the later row, which
/// is no bundle, and selection's open of every row fails at the first, the
/// input's `entry_error`.
pub fn selection_fails() -> Built {
    Ok(scene(
        vec![one(
            "x.bundle",
            &[
                ("CAB-x", cab("x")?),
                ("CAB-n", broken_bundle()),
                ("CAB-n", b"raw bytes".to_vec()),
            ],
            &[],
        )],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// Every failure scene, by name.
pub fn failure_scenes() -> Result<Vec<(&'static str, AtlasScene)>, BuildError> {
    let all: [(&'static str, super::Make); 5] = [
        ("member_fails", member_fails),
        ("member_not_a_bundle", member_not_a_bundle),
        ("input_not_a_bundle", input_not_a_bundle),
        ("reach_fails", reach_fails),
        ("selection_fails", selection_fails),
    ];
    all.into_iter().map(|(name, f)| Ok((name, f()?))).collect()
}
