//! B5's release gate (B5b): an empty verified table refuses every
//! configuration with today's refusal; a class admits only the
//! configurations it holds, so one B4 admits by argument but E5 never
//! recorded stays refused, as does any unknown class.
use super::{
    body_model::verdict,
    chain::{offset, read, rig, scale},
    root::variant_clip,
};
use unityrs_anim::{
    AnimError, BodyMotion,
    humanoid::{
        RECORDED_ABSENT,
        chain::{ANIMATOR_SCALE, CHAIN_VARIANT, ChainFeature},
        gate::{Configuration, EDITORS, VERIFIED, VerifiedClass, configuration, release_gate},
        root::{self, ADMITTED},
    },
};
use unityrs_scene::GraphOptions;
use unityrs_testkit::ngui::Object;

/// Today's refusal, by name.
fn today() -> Result<(), String> {
    verdict::<()>(Err(AnimError::UnsupportedBodyMotion(BodyMotion::BodySlots)))
}

/// The configuration of the test rig after `edit`, playing `variant`'s
/// clip in a bundle of `version`.
fn configured(
    edit: impl FnOnce(&mut Vec<Object>),
    variant: &str,
    version: &str,
) -> Result<Configuration, AnimError> {
    read(
        &rig(Vec::new()),
        GraphOptions::default(),
        false,
        edit,
        |body, paths, meter| {
            let chain = body.animator_chain(paths, meter)?;
            let settings = root::root_settings(&variant_clip(variant), meter)?;
            configuration(body, &chain, &settings, version, meter)
        },
    )
    .0
}

/// The class recorded at exactly `found`'s configuration.
fn class_of(found: &Configuration) -> VerifiedClass {
    VerifiedClass {
        absent: found.absent,
        feature: found.feature,
        offset: found.offset,
        human_scale: [found.human_scale; 2],
        variant: found.variant,
    }
}

/// The runtime configuration of each admitted variant at both placements,
/// in both Editor families: what B3 and B4 check, then the gate.
#[test]
fn the_configuration_is_b3s_and_b4s_scope() {
    for row in &ADMITTED {
        for (placed, edit) in [(false, None), (true, Some(offset as fn(&mut [Object])))] {
            for (version, family) in [("2022.3.1f1", "2022"), ("6000.0.1f1", "6000")] {
                let found = configured(
                    |o| edit.into_iter().for_each(|f| f(o)),
                    row.variant,
                    version,
                )
                .unwrap();
                assert_eq!(found.feature, ChainFeature::Plain);
                assert_eq!(found.offset, placed);
                assert_eq!(found.variant, row.variant);
                assert_eq!(found.editor, family);
                assert!(
                    RECORDED_ABSENT.contains(&found.absent),
                    "{:#x}",
                    found.absent
                );
            }
        }
    }
    assert_eq!(EDITORS, ["2022", "6000"]);
    // An Editor family outside EDITORS, a chain out of scope and an F5
    // feature with another variant refuse before the gate.
    assert_eq!(
        verdict(configured(|_| {}, CHAIN_VARIANT, "2021.3.1f1")),
        today()
    );
    let outside = configured(|o| scale(o, 102, [10.; 3]), CHAIN_VARIANT, "2022.3.1f1");
    assert_eq!(
        verdict(outside),
        Err(AnimError::UnsupportedAnimatorRoot.to_string())
    );
    let scaled = |o: &mut Vec<Object>| scale(o, 101, [ANIMATOR_SCALE; 3]);
    let found = configured(scaled, CHAIN_VARIANT, "2022.3.1f1").unwrap();
    assert_eq!(found.feature, ChainFeature::ScaledAnimator);
    let other = ADMITTED
        .iter()
        .find(|row| row.variant != CHAIN_VARIANT)
        .unwrap();
    let refused = configured(scaled, other.variant, "2022.3.1f1");
    assert_eq!(
        verdict(refused),
        Err(AnimError::UnsupportedAnimatorRoot.to_string())
    );
}

/// Until B5b fills it, the table is empty and every configuration keeps
/// today's refusal.
#[test]
fn the_empty_table_refuses_everything() {
    let none: [VerifiedClass; 0] = [];
    assert_eq!(VERIFIED, none);
    for row in &ADMITTED {
        let found = configured(|_| {}, row.variant, "2022.3.1f1").unwrap();
        assert_eq!(
            verdict(release_gate(&VERIFIED, &found)),
            today(),
            "{}",
            row.variant
        );
    }
}

/// A verified class admits its configurations, the ends of its
/// human-scale interval included, in either Editor family.
#[test]
fn a_verified_class_admits_what_it_holds() {
    let found = configured(|_| {}, CHAIN_VARIANT, "2022.3.1f1").unwrap();
    let h = found.human_scale;
    let mut class = class_of(&found);
    assert_eq!(verdict(release_gate(&[class], &found)), Ok(()));
    class.human_scale = [h * 0.75, h];
    assert_eq!(verdict(release_gate(&[class], &found)), Ok(()));
    let later = Configuration {
        editor: "6000",
        ..found
    };
    assert_eq!(verdict(release_gate(&[class], &later)), Ok(()));
}

/// A class B4 admits by argument but E5 did not record keeps today's
/// refusal: another topology (a rig without toes), an F5 feature on it (a
/// scaled Animator), the other placement, another variant, a human scale
/// one ulp outside the recorded interval, an unknown Editor family.
#[test]
fn an_unrecorded_class_keeps_todays_refusal() {
    let found = configured(|_| {}, CHAIN_VARIANT, "2022.3.1f1").unwrap();
    let table = [class_of(&found)];
    let toes = RECORDED_ABSENT
        .iter()
        .copied()
        .find(|&absent| absent != found.absent)
        .unwrap();
    let up = f32::from_bits(found.human_scale.to_bits() + 1);
    let down = f32::from_bits(found.human_scale.to_bits() - 1);
    let other = ADMITTED
        .iter()
        .find(|row| row.variant != CHAIN_VARIANT)
        .unwrap()
        .variant;
    for (label, unrecorded) in [
        (
            "topology",
            Configuration {
                absent: toes,
                ..found
            },
        ),
        (
            "feature",
            Configuration {
                absent: toes,
                feature: ChainFeature::ScaledAnimator,
                ..found
            },
        ),
        (
            "placement",
            Configuration {
                offset: !found.offset,
                ..found
            },
        ),
        (
            "variant",
            Configuration {
                variant: other,
                ..found
            },
        ),
        (
            "above",
            Configuration {
                human_scale: up,
                ..found
            },
        ),
        (
            "below",
            Configuration {
                human_scale: down,
                ..found
            },
        ),
        (
            "editor",
            Configuration {
                editor: "2021",
                ..found
            },
        ),
    ] {
        assert_eq!(
            verdict(release_gate(&table, &unrecorded)),
            today(),
            "{label}"
        );
    }
}
