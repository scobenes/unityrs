//! Freeze v8's chain from the Animator node to the hips (plan 2.3, B4b):
//! the hips parent in `A` from the rig's Transforms, the F5 features, and
//! the admission of each against the root configurations.
use super::{
    body_model::{full, refused, verdict},
    file,
};
use unityrs_anim::{
    AnimError, BodyModel, RigPaths,
    humanoid::{
        body_avatar,
        body_solve::HipsParent,
        chain::{AnimatorChain, ChainFeature, admit_chain},
        rig_avatar,
        root::{Cell, RootRules, RootSettings},
    },
};
use unityrs_core::limits::Meter;
use unityrs_scene::{GraphOptions, graph_from_game_object};
use unityrs_testkit::{
    ngui::Object,
    scene3d::{
        Layout,
        anim::humanoid::{self as h, ANIMATOR, Rig},
        set, xyz,
    },
};

/// The chain of `rig`'s prefab after `edit`, which takes the objects: the
/// Animator node's Transform is 101, the hips' 102.
fn chain(rig: &Rig, edit: impl FnOnce(&mut [Object])) -> Result<AnimatorChain, AnimError> {
    let meter = Meter::unlimited();
    let mut objects = h::prefab(rig);
    edit(&mut objects);
    let (env, file) = file(Layout::Unity2022, &objects);
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        &meter,
    )
    .unwrap();
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    let avatar = rig_avatar(&env, &file.object(ANIMATOR).unwrap(), &meter)
        .unwrap()
        .avatar
        .unwrap();
    let body = body_avatar(&avatar, &meter).unwrap();
    body.animator_chain(&paths, &meter)
}

fn scale(objects: &mut [Object], transform: i64, value: [f32; 3]) {
    let node = objects.iter_mut().find(|o| o.path_id == transform).unwrap();
    set(&mut node.value, "m_LocalScale", xyz("Vector3f", &value));
}

fn rig(armature: Vec<([f32; 3], [f32; 4], f32)>) -> Rig {
    Rig {
        hierarchy: true,
        armature,
        ..full()
    }
}

/// A quarter turn about up, as f32 components.
const TURN: [f32; 4] = [0., 0.707_106_77, 0., 0.707_106_77];

#[test]
fn the_plain_chain_is_the_animator_node() {
    let found = chain(&rig(Vec::new()), |_| {}).unwrap();
    assert_eq!(found.parent, HipsParent::IDENTITY);
    assert_eq!(found.hips_scale, 1.);
    assert_eq!(found.feature, ChainFeature::Plain);
}

/// Each F5 feature alone: one node between (its transform is the hips
/// parent's, exactly, as the reference's `link` from the identity), a
/// scaled Animator node (which never enters `A`) and a hips scale.
#[test]
fn each_recorded_feature_composes_alone() {
    let between = ([0., 0.05, 0.1], TURN, 0.8);
    let found = chain(&rig(vec![between]), |_| {}).unwrap();
    assert_eq!(found.feature, ChainFeature::Intermediate);
    assert_eq!(found.parent.position, [0_f32, 0.05, 0.1].map(f64::from));
    assert_eq!(found.parent.rotation, TURN.map(f64::from));
    assert_eq!(found.parent.scale, f64::from(0.8_f32));
    let found = chain(&rig(Vec::new()), |o| scale(o, 101, [1.25; 3])).unwrap();
    assert_eq!(found.feature, ChainFeature::ScaledAnimator);
    assert_eq!(found.parent, HipsParent::IDENTITY);
    let found = chain(&rig(Vec::new()), |o| scale(o, 102, [1.2; 3])).unwrap();
    assert_eq!(found.feature, ChainFeature::HipsScale);
    assert_eq!(found.hips_scale, f64::from(1.2_f32));
    // Within `uniform`'s tolerance of one is still a hips scale.
    let near = [1., 1., 1. + f32::EPSILON];
    let found = chain(&rig(Vec::new()), |o| scale(o, 102, near)).unwrap();
    assert_eq!(found.feature, ChainFeature::HipsScale);
}

/// Two nodes compose as the reference's `link` does: the second's
/// translation turned by the first's rotation and scaled by its scale.
/// F5 recorded no such chain, so it refuses after composing.
#[test]
fn unrecorded_chains_refuse() {
    let root = Err(AnimError::UnsupportedAnimatorRoot.to_string());
    let deep = vec![
        ([0., 1., 0.], TURN, 2.),
        ([1., 0., 0.], [0., 0., 0., 1.], 1.),
    ];
    assert_eq!(verdict(chain(&rig(deep), |_| {})), root);
    // Two features together.
    let between = vec![([0., 0.05, 0.1], TURN, 0.8)];
    assert_eq!(
        verdict(chain(&rig(between), |o| scale(o, 102, [1.2; 3]))),
        root
    );
    assert_eq!(
        verdict(chain(&rig(Vec::new()), |o| {
            scale(o, 101, [1.25; 3]);
            scale(o, 102, [1.2; 3]);
        })),
        root
    );
    // Nonuniform or nonpositive scales anywhere on the chain.
    let squashed = vec![([0.; 3], [0., 0., 0., 1.], 1.)];
    assert_eq!(
        verdict(chain(&rig(squashed), |o| scale(o, 190, [1., 2., 1.]))),
        root
    );
    assert_eq!(
        verdict(chain(&rig(Vec::new()), |o| scale(o, 101, [1., 1., 2.]))),
        root
    );
    assert_eq!(
        verdict(chain(&rig(Vec::new()), |o| scale(o, 102, [-1.; 3]))),
        root
    );
    // A link that overflows refuses as it composes: nine scales of 3e38.
    let huge = vec![([0.; 3], [0., 0., 0., 1.], 3e38); 9];
    assert_eq!(
        verdict(chain(&rig(huge), |_| {})),
        Err(refused(BodyModel::NonFiniteBody))
    );
}

fn settings(cell: Cell) -> RootSettings {
    RootSettings {
        rules: RootRules {
            yaw: cell,
            height: cell,
            xz: cell,
        },
        start: ([0.; 3], [0., 0., 0., 1.]),
        level: 0.,
        offset_degrees: 0.,
    }
}

/// A plain chain takes every admitted configuration; an F5 chain only all
/// Original, the one E4 passed on it.
#[test]
fn f5_chains_admit_only_all_original() {
    let with = |feature| AnimatorChain {
        parent: HipsParent::IDENTITY,
        hips_scale: 1.,
        feature,
    };
    for cell in [Cell::Bake, Cell::Body, Cell::Original] {
        assert!(admit_chain(&with(ChainFeature::Plain), &settings(cell)).is_ok());
    }
    for feature in [
        ChainFeature::ScaledAnimator,
        ChainFeature::Intermediate,
        ChainFeature::HipsScale,
    ] {
        assert!(admit_chain(&with(feature), &settings(Cell::Original)).is_ok());
        for cell in [Cell::Bake, Cell::Body] {
            assert_eq!(
                verdict(admit_chain(&with(feature), &settings(cell))),
                Err(AnimError::UnsupportedAnimatorRoot.to_string())
            );
        }
    }
}
