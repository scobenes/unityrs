use std::sync::Arc;
use unityrs_anim::{AnimError, ClipSelector, SelectionError, discover};
use unityrs_core::env::{DerefError, EnvOptions, Environment, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, RawStr};
use unityrs_scene::{GraphOptions, graph_from_scene_file};
use unityrs_testkit::ngui::{Object, V, set_field};
use unityrs_testkit::scene3d::{self as s, Layout, anim as a};

fn load(
    env: &Environment,
    name: &'static [u8],
    externals: &[&str],
    objects: &[Object],
) -> Arc<SerializedFile> {
    load_layout(env, name, externals, objects, Layout::Unity2022)
}

fn load_layout(
    env: &Environment,
    name: &'static [u8],
    externals: &[&str],
    objects: &[Object],
    layout: Layout,
) -> Arc<SerializedFile> {
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(name),
            data: Bytes::from(s::file(layout, externals, objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("serialized fixture");
    };
    file
}

fn cross_file_rigs(
    env: &Environment,
) -> (
    Arc<SerializedFile>,
    Arc<SerializedFile>,
    Arc<SerializedFile>,
) {
    let clips = load(
        env,
        b"clips",
        &["base"],
        &[
            a::clip(-7),
            a::clip(-8),
            a::clip(-9),
            a::overrides(41, (1, 30), &[((0, -7), (0, -9))]),
        ],
    );
    let base = load(
        env,
        b"base",
        &["clips", "outside"],
        &[
            a::controller(
                30,
                &[(0, -7), (1, -7), (1, -8), (1, -8), (2, -7), (1, -9)],
                &[5, 0, 9, u32::MAX],
            ),
            a::clip(-7),
        ],
    );
    let rig = load(
        env,
        b"rig",
        &["base", "clips", "outside"],
        &[
            Object::new(
                1,
                1,
                s::game_object(b"same", &[(0, 2), (0, 3), (0, 4), (0, 3)]),
            ),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::legacy(3, (2, -7), &[(1, -7), (2, -7)]),
            a::animator(4, (0, 40)),
            a::overrides(40, (2, 41), &[((2, -9), (3, -8)), ((2, -8), (0, 0))]),
            Object::new(5, 1, s::game_object(b"same", &[(0, 6), (0, 7)])),
            Object::new(6, 4, s::transform((0, 5), (0, 0), &[], false)),
            a::legacy(7, (2, -7), &[]),
        ],
    );
    (clips, base, rig)
}

#[test]
fn ordered_scoped_slots_and_direct_selection() {
    let env = Environment::new(EnvOptions::unitypy());
    let (clips, base, rig) = cross_file_rigs(&env);
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &rig, &GraphOptions::default(), &meter).unwrap();
    let baseline = meter.used();
    let found = discover(&env, &graph, &meter).unwrap();
    let rigs = found.rigs();
    assert_eq!(
        rigs.iter()
            .map(|rig| rig.component.path_id())
            .collect::<Vec<_>>(),
        [3, 4, 7]
    );
    let legacy = &rigs[0];
    assert_eq!(
        legacy
            .slots()
            .iter()
            .map(|slot| slot.source.field)
            .collect::<Vec<_>>(),
        ["m_Animation", "m_Animations", "m_Animations"]
    );
    assert_eq!(legacy.clips().count(), 2);
    let controller = &rigs[1];
    let slots = controller.slots();
    assert_eq!(slots.len(), 6);
    assert_eq!(slots[0].clip().unwrap().path_id(), -7);
    assert!(Arc::ptr_eq(slots[0].clip().unwrap().file(), &base));
    assert_eq!(slots[2].clip().unwrap().path_id(), -8);
    assert!(!slots[3].is_first());
    assert_eq!(controller.clips().count(), 2);
    for (i, field, owner) in [
        (1, "m_OverrideClip", 40),
        (4, "m_AnimationClips", 30),
        (5, "m_OverrideClip", 40),
    ] {
        let err = slots[i].clip().unwrap_err();
        assert!(matches!(
            &err.cause,
            AnimError::OutsideView(DerefError::NotFound { .. })
        ));
        assert_eq!(err.source.field, field);
        assert_eq!(err.source.owner.path_id(), owner);
        assert_eq!(slots[i].source.ordinal, i);
    }
    let leaves = controller.controller().unwrap().leaves();
    assert_eq!(leaves[0].slot.as_ref().unwrap(), &5);
    assert!(matches!(leaves[2].slot, Err(AnimError::InvalidClipSlot(9))));
    assert_eq!(leaves.len(), 3);
    let root = &graph.nodes()[legacy.node].transform;
    let expected = clips.object(-7).unwrap();
    let selected = found
        .select(
            root,
            &legacy.component,
            ClipSelector::Clip(&expected),
            &meter,
        )
        .unwrap();
    assert_eq!(selected.source.field, "m_Animation");
    let alias = found
        .select(
            &graph.nodes()[controller.node].transform,
            &controller.component,
            ClipSelector::Source(&slots[3].source),
            &meter,
        )
        .unwrap();
    assert_eq!(alias.source.ordinal, 3);
    let error = found
        .select(
            &graph.nodes()[controller.node].transform,
            &controller.component,
            ClipSelector::Source(&slots[1].source),
            &meter,
        )
        .unwrap_err();
    assert!(matches!(error, SelectionError::Unresolved(err)
        if std::ptr::eq(err, slots[1].clip().unwrap_err())));
    assert!(
        found
            .select(
                root,
                &rigs[2].component,
                ClipSelector::Clip(&expected),
                &meter
            )
            .is_err()
    );
    assert_eq!(rigs[2].clips().count(), 1);
    assert!(meter.used() > baseline);
    drop(found);
    assert_eq!(meter.used(), baseline);
}

#[test]
fn override_cycles_are_retained_as_rig_issues() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load(
        &env,
        b"cycle",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            a::overrides(4, (0, 5), &[]),
            a::overrides(5, (0, 4), &[]),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let issue = &catalog.rigs()[0].issues()[0];
    assert!(matches!(issue.cause, AnimError::ControllerCycle));
    assert_eq!(issue.source.owner.path_id(), 5);
    assert_eq!(issue.source.field, "m_Controller");
    assert_eq!(issue.source.pointer.unwrap().path_id, 4);
}

#[test]
fn malformed_controller_edge_retains_its_owner() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load(
        &env,
        b"bad-edge",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            set_field(
                a::overrides(4, (0, 0), &[]),
                "m_Controller",
                V::Int(1),
                None,
            ),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let issue = &catalog.rigs()[0].issues()[0];
    assert_eq!(issue.source.owner.path_id(), 4);
    assert_eq!(issue.source.field, "m_Controller");
    assert!(matches!(issue.cause, AnimError::Malformed(_)));
}

#[test]
fn pre_52_controller_leaves_use_the_state_offset_table() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load_layout(
        &env,
        b"legacy-controller",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            a::controller_with_leaf_info(
                4,
                &[(0, -7), (0, -8), (0, -9)],
                &[102, 101, 999],
                Some((&[101, 102], 1)),
            ),
            a::clip(-7),
            a::clip(-8),
            a::clip(-9),
        ],
        Layout::Unity51,
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let rig = &catalog.rigs()[0];
    let controller = rig.controller().unwrap();
    assert_eq!(
        controller.strings(0, &meter).unwrap().as_slice(),
        &[b"".as_slice()]
    );
    let leaves = controller.leaves();
    assert_eq!(leaves[0].slot.as_ref().unwrap(), &2);
    assert_eq!(leaves[1].slot.as_ref().unwrap(), &1);
    assert!(matches!(
        leaves[2].slot,
        Err(AnimError::UnmappedControllerLeaf(999))
    ));
    assert_eq!(rig.slots()[0].clip().unwrap().path_id(), -7);
}
