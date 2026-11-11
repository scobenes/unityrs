//! The scenes of several inputs, for the winners across them
//! (`docs/BATCH_SPEC.md` 4.5).

use super::super::{PropName, V, atlas, file, material, mono_script};
use super::{AtlasScene, CutOptions, SCRIPT, SceneBundle, one, plain, rgba, with};
use crate::ngui::BuildError;

/// One input's bundle for [`winners`]: an atlas `atlas` cutting its own
/// sheet `sheet` of base `base`, with `sprites`, and `blob` when its JSON is
/// to be unwritable.
fn winner_input(
    id: &str,
    atlas_name: &str,
    base: u8,
    sprites: &[&str],
    blob: bool,
) -> Result<SceneBundle, BuildError> {
    let list = sprites
        .iter()
        .zip([0, 1].into_iter().cycle())
        .map(|(n, x)| {
            let s = plain(n, (x, 0, 1, 1));
            if blob {
                with(s, vec![("blob", V::Bytes(vec![0]))])
            } else {
                s
            }
        })
        .collect();
    let cab_name = format!("CAB-{}", id.trim_end_matches(".bundle"));
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "sheet", base, 2, 1),
            material(3, "wmat", PropName::Plain, &[("_MainTex", (0, 2))]),
            atlas(4, atlas_name.as_bytes(), SCRIPT, (0, 3), list, Vec::new()),
        ],
        true,
    )?;
    Ok(one(id, &[(cab_name.as_str(), cab)], &[]))
}

/// Several inputs cutting atlases into the same keys, for the winners
/// (`docs/BATCH_SPEC.md` 4.5), the sprite key `{sprite}.png`: `a` and `b`
/// alike, `c` with other pixels and one more sprite (so its JSON differs),
/// `d` whose JSON is unwritable (its crops plain candidates), `e`, an atlas
/// `other` whose unit shares the key `k.png` with the others', `f`, whose
/// JSON is unwritable too, with a plain candidate for `c`'s `i.png`, and
/// `g`, like `c` with other pixels again (a second holder of `i.png`).
pub fn winners() -> Result<AtlasScene, BuildError> {
    let bundles = vec![
        winner_input("a.bundle", "shared", 1, &["k", "j"], false)?,
        winner_input("b.bundle", "shared", 1, &["k", "j"], false)?,
        winner_input("c.bundle", "shared", 0x31, &["k", "j", "i"], false)?,
        winner_input("d.bundle", "shared", 0x41, &["k", "h"], true)?,
        winner_input("e.bundle", "other", 0x51, &["k"], false)?,
        winner_input("f.bundle", "shared", 0x61, &["i"], true)?,
        winner_input("g.bundle", "shared", 0x71, &["k", "j", "i"], false)?,
    ];
    let inputs = bundles.iter().map(|b| b.id.clone()).collect();
    Ok(AtlasScene {
        bundles,
        inputs,
        options: CutOptions {
            sprite_key: "{sprite}.png".to_owned(),
            ..CutOptions::default()
        },
    })
}

/// Several inputs with textures exported, for the cross-kind rule (E26,
/// E32): `p` and `q` each a unit keyed by their sheet's texture key
/// `sheet.png` (an atlas `at` cutting a sheet `sheet`, with other pixels in
/// `q`), and `r`, a texture `sheet` that no atlas cuts, a plain candidate
/// for `sheet.png`.
pub fn textured_winners() -> Result<AtlasScene, BuildError> {
    let unit = |id: &str, base: u8| -> Result<SceneBundle, BuildError> {
        let cab = file(
            &[],
            &[
                mono_script(1, "NGUIAtlas"),
                rgba(2, "sheet", base, 2, 1),
                material(3, "tm", PropName::Plain, &[("_MainTex", (0, 2))]),
                atlas(
                    4,
                    b"at",
                    SCRIPT,
                    (0, 3),
                    vec![plain("s", (0, 0, 1, 1))],
                    Vec::new(),
                ),
            ],
            true,
        )?;
        let cab_name = format!("CAB-{}", id.trim_end_matches(".bundle"));
        Ok(one(id, &[(cab_name.as_str(), cab)], &[]))
    };
    let lone = file(&[], &[rgba(2, "sheet", 0x23, 2, 1)], true)?;
    let bundles = vec![
        unit("p.bundle", 0x21)?,
        unit("q.bundle", 0x22)?,
        one("r.bundle", &[("CAB-r", lone)], &[]),
    ];
    let inputs = bundles.iter().map(|b| b.id.clone()).collect();
    Ok(AtlasScene {
        bundles,
        inputs,
        options: CutOptions::textures("{name}.png"),
    })
}
