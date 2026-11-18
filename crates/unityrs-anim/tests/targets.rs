use unityrs_anim::{
    AnimError, Attachment, BindingTarget, MorphProperty, PathQuery, RendererChannel,
    RendererChannels, RigPaths, SlotCounts, TransformProperty, read_bindings,
};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, RawStr};
use unityrs_scene::{Graph, GraphOptions, graph_from_game_object};
use unityrs_testkit::ngui::Object;
use unityrs_testkit::scene3d::{self as s, Layout, anim as a};

// IEEE CRC32 from zlib over the literal UTF-8 names.
// Built 2022.3 clips hash the bare name; AssetStudio hashes the property.
const SMILE_NAME_HASH: u32 = 0xc34b_2c0b; // Smile
const SMILE_PREFIXED_HASH: u32 = 0xa7cb_48af; // blendShape.Smile
const FROWN_NAME_HASH: u32 = 0x5958_1d90; // Frown, which no channel here has

fn fixture(meter: &std::sync::Arc<Meter>) -> (Graph, std::sync::Arc<SerializedFile>) {
    let env = Environment::new(EnvOptions::unitypy());
    let mut objects = vec![
        Object::new(1, 1, s::game_object(b"owner", &[(0, 2)])),
        Object::new(
            2,
            4,
            s::transform(
                (0, 1),
                (0, 0),
                &[
                    (0, 12),
                    (0, 22),
                    (0, 32),
                    (0, 42),
                    (0, 52),
                    (0, 62),
                    (0, 72),
                ],
                false,
            ),
        ),
        Object::new(99, 43, unityrs_testkit::ngui::st("Mesh", vec![])),
        a::bound_clip(
            98,
            Layout::Unity2022,
            &[
                a::Binding {
                    class: 137,
                    custom: 20,
                    path: 3_583_462_788,
                    attribute: SMILE_NAME_HASH,
                    ..Default::default()
                },
                a::Binding {
                    class: 114,
                    integer: 1,
                    path: 1,
                    ..Default::default()
                },
            ],
            &[],
        ),
    ];
    for (id, name) in [
        (11, b"\x9d\x0a\xd9\x6d".as_slice()),
        (21, b"repeat"),
        (31, b"repeat"),
        (41, "骨".as_bytes()),
        (51, b"other"),
        (61, b"two"),
        (71, b"literal/slash"),
    ] {
        let mut components = vec![(0, id + 1)];
        if (41..=61).contains(&id) {
            components.push((0, id + 2));
            objects.push(Object::new(
                id + 2,
                137,
                s::skinned_mesh_renderer((0, id), (0, 99), &[], &[]),
            ));
        }
        if id == 61 {
            components.push((0, id + 3));
            objects.push(Object::new(
                id + 3,
                137,
                s::skinned_mesh_renderer((0, id), (0, 99), &[], &[]),
            ));
        }
        objects.extend([
            Object::new(id, 1, s::game_object(name, &components)),
            Object::new(id + 1, 4, s::transform((0, id), (0, 2), &[], false)),
        ]);
    }
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"paths"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], &objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("serialized fixture");
    };
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        meter,
    )
    .unwrap();
    (graph, file)
}

#[test]
fn raw_relative_paths_keep_crc_collisions_and_duplicates() {
    let meter = Meter::unlimited();
    let (graph, _) = fixture(&meter);
    let baseline = meter.used();
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    assert_eq!(paths.resolve(PathQuery::Name(b""), &meter).unwrap(), 0);
    assert!(matches!(
        paths.resolve(PathQuery::Name(b"literal/slash"), &meter),
        Err(AnimError::MissingPath)
    ));
    assert_eq!(
        paths
            .resolve(PathQuery::Hash(4_008_179_475), &meter)
            .unwrap(),
        7
    );
    assert_eq!(
        paths.hash_candidates(0, &meter).unwrap().as_slice(),
        &[0, 1]
    );
    assert!(matches!(
        paths.resolve(PathQuery::Hash(0), &meter),
        Err(AnimError::CrcCollision(0))
    ));
    assert!(matches!(
        paths.resolve(PathQuery::Name(b"repeat"), &meter),
        Err(AnimError::AmbiguousPath)
    ));
    assert_eq!(
        paths
            .resolve(
                PathQuery::HashWithEvidence(3_583_462_788, "骨".as_bytes()),
                &meter
            )
            .unwrap(),
        4
    );
    assert!(matches!(
        paths.resolve(PathQuery::HashWithEvidence(3_583_462_788, b"other"), &meter),
        Err(AnimError::PathEvidenceMismatch)
    ));
    assert!(matches!(
        paths.resolve(PathQuery::Name(b"Other"), &meter),
        Err(AnimError::MissingPath)
    ));
    assert!(matches!(
        paths.resolve(PathQuery::Name("owner/骨".as_bytes()), &meter),
        Err(AnimError::MissingPath)
    ));
    let subrig = RigPaths::new(&graph, 4, &meter).unwrap();
    assert_eq!(subrig.resolve(PathQuery::Hash(0), &meter).unwrap(), 4);
    drop((paths, subrig));
    assert_eq!(meter.used(), baseline);
}

#[test]
fn morph_channels_are_bound_to_the_renderer_instance() {
    let meter = Meter::unlimited();
    let (graph, file) = fixture(&meter);
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    let renderer = file.object(43).unwrap();
    let other = file.object(53).unwrap();
    let channel = RendererChannel {
        name: b"Smile",
        hash: SMILE_NAME_HASH,
        target_slot: 3,
        frame: 7,
        full_weight: 80.0,
    };
    let channels = [channel];
    let tables = [
        RendererChannels {
            renderer: &renderer,
            attachment: Attachment::Present(&channels),
        },
        RendererChannels {
            renderer: &other,
            attachment: Attachment::Filtered,
        },
    ];
    let path = PathQuery::Name("骨".as_bytes());
    for property in [
        MorphProperty::Attribute(b"blendShape.Smile"),
        MorphProperty::Hash(SMILE_NAME_HASH),
        MorphProperty::Hash(SMILE_PREFIXED_HASH),
    ] {
        let target = paths
            .bind_morph(path, property, &[], &tables, &meter)
            .unwrap();
        assert!(
            matches!(target, BindingTarget::Morph { node: 4, renderer, channel }
            if renderer.path_id() == 43 && channel.target_slot == 3 && channel.frame == 7)
        );
    }
    assert!(matches!(
        paths.bind_morph(
            path,
            MorphProperty::Hash(FROWN_NAME_HASH),
            &[],
            &tables,
            &meter
        ),
        Err(AnimError::MissingChannel(4))
    ));
    assert!(matches!(
        paths.bind_morph(
            PathQuery::Name(b"other"),
            MorphProperty::Hash(SMILE_NAME_HASH),
            &[],
            &tables,
            &meter
        ),
        Err(AnimError::FilteredTarget(5))
    ));
    assert!(matches!(
        paths.bind_morph(path, MorphProperty::Hash(SMILE_NAME_HASH), &[], &[], &meter),
        Err(AnimError::MissingAttachment(4))
    ));
    assert!(matches!(
        paths.bind_morph(
            PathQuery::Name(b"two"),
            MorphProperty::Hash(SMILE_NAME_HASH),
            &[],
            &tables,
            &meter
        ),
        Err(AnimError::AmbiguousRenderer(6))
    ));
    let duplicate = [
        channel,
        RendererChannel {
            target_slot: 4,
            ..channel
        },
    ];
    let tables = [RendererChannels {
        renderer: &renderer,
        attachment: Attachment::Present(&duplicate),
    }];
    assert!(matches!(
        paths.bind_morph(
            path,
            MorphProperty::Hash(SMILE_NAME_HASH),
            &[],
            &tables,
            &meter
        ),
        Err(AnimError::AmbiguousChannel(4))
    ));
    assert!(matches!(
        paths.bind_transform(path, TransformProperty::Euler, &[4], &meter),
        Err(AnimError::FilteredTarget(4))
    ));
}

#[test]
fn compiled_morph_hashes_accept_both_conventions_unless_ambiguous() {
    let meter = Meter::unlimited();
    let (graph, file) = fixture(&meter);
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    let renderer = file.object(43).unwrap();
    let channel = RendererChannel {
        name: b"Smile",
        hash: SMILE_NAME_HASH,
        target_slot: 3,
        frame: 7,
        full_weight: 80.0,
    };
    let path = PathQuery::Name("骨".as_bytes());
    // `blendShape.Smile` hashes, bare, to the prefixed hash of `Smile`.
    let conventions = [
        channel,
        RendererChannel {
            name: b"blendShape.Smile",
            hash: SMILE_PREFIXED_HASH,
            target_slot: 4,
            ..channel
        },
    ];
    let tables = [RendererChannels {
        renderer: &renderer,
        attachment: Attachment::Present(&conventions),
    }];
    assert!(matches!(
        paths.bind_morph(
            path,
            MorphProperty::Hash(SMILE_PREFIXED_HASH),
            &[],
            &tables,
            &meter
        ),
        Err(AnimError::AmbiguousChannel(4))
    ));
    assert!(matches!(
        paths.bind_morph(path, MorphProperty::Hash(SMILE_NAME_HASH), &[], &tables, &meter),
        Ok(BindingTarget::Morph { channel, .. }) if channel.target_slot == 3
    ));
}

#[test]
fn compiled_slots_bind_playable_targets_and_retain_metadata() {
    let meter = Meter::unlimited();
    let (graph, file) = fixture(&meter);
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    let table = read_bindings(&file.object(98).unwrap(), &meter).unwrap();
    let slots = table
        .slots(
            SlotCounts {
                constant: 2,
                ..Default::default()
            },
            &meter,
        )
        .unwrap();
    let slots = slots.as_slice();
    let renderer = file.object(43).unwrap();
    let channel = RendererChannel {
        name: b"Smile",
        hash: SMILE_NAME_HASH,
        target_slot: 3,
        frame: 7,
        full_weight: 80.0,
    };
    let table = [RendererChannels {
        renderer: &renderer,
        attachment: Attachment::Present(std::slice::from_ref(&channel)),
    }];
    assert!(matches!(
        paths.bind_generic(&slots[0], &[], &table, &meter).unwrap(),
        BindingTarget::Morph { node: 4, channel, .. } if channel.target_slot == 3
    ));
    assert!(matches!(
        paths.bind_generic(&slots[1], &[], &[], &meter).unwrap(),
        BindingTarget::Metadata
    ));
    let path = PathQuery::Name("骨".as_bytes());
    assert!(matches!(
        paths.bind_morph(
            path,
            MorphProperty::Hash(FROWN_NAME_HASH),
            &[],
            &table,
            &meter
        ),
        Err(AnimError::MissingChannel(4))
    ));
    assert!(matches!(
        paths.bind_morph(
            path,
            MorphProperty::Attribute(b"blendShape.Smile"),
            &[4],
            &table,
            &meter
        ),
        Err(AnimError::FilteredTarget(4))
    ));
}
