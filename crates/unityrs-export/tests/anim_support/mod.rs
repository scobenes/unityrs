//! Embedded-typetree animation scenes shared by integration tests.
use unityrs_testkit::{
    env as e,
    ngui::{Object, V},
    scene3d::{
        self as s, Layout,
        anim::{
            self, Binding,
            playback::{self as p, Key, Storage},
        },
        mesh as m,
    },
};

pub const PREFAB: &str = "assets/synthetic.prefab";
pub const CLIP: &str = "assets/synthetic.anim";
pub const CAB: &str = "CAB-synthetic";

pub fn legacy_clip() -> Object {
    let mut left = Key::new(2., [12.5]);
    left.outgoing = [f32::INFINITY];
    let mut middle = Key::new(2.25, [17.5]);
    middle.outgoing = [f32::INFINITY];
    let right = Key::new(3., [17.5]);
    p::explicit(
        9,
        4.,
        &[
            (
                "m_FloatCurves",
                vec![
                    p::curve(
                        "",
                        &[left, middle, right],
                        [2, 2],
                        None,
                        true,
                        Some(("blendShape.bend", 137)),
                    ),
                    p::curve(
                        "",
                        &[Key::new(2., [0.5])],
                        [2, 2],
                        None,
                        true,
                        Some(("material.opacity", 23)),
                    ),
                ],
            ),
            (
                "m_PositionCurves",
                vec![p::curve(
                    "",
                    &[Key::new(2., [0.; 3]), Key::new(3., [1., -2., 0.])],
                    [2, 2],
                    None,
                    true,
                    None,
                )],
            ),
            (
                "m_RotationCurves",
                vec![p::curve(
                    "",
                    &[
                        Key::new(2., [0., 0., 0., 1.]),
                        Key::new(3., [0., 0., 0.6, 0.8]),
                    ],
                    [2, 2],
                    None,
                    true,
                    None,
                )],
            ),
        ],
    )
}

#[derive(Clone, Copy)]
pub enum GenericClip {
    ConstantTrs,
    ClampedTranslation,
    HeldQuaternionFlip,
}

pub fn generic_clip(kind: GenericClip) -> Object {
    let clamped_translation = matches!(kind, GenericClip::ClampedTranslation);
    let held_rotation = matches!(kind, GenericClip::HeldQuaternionFlip);
    let mut object = p::explicit(9, 4., &[]);
    let mut words = Vec::new();
    p::frame(&mut words, f32::MIN, &[]);
    for (time, weight) in [(2., 12.5_f32), (2.25, 17.5), (3., 17.5)] {
        if clamped_translation && time == 3. {
            p::frame(&mut words, 2.75, &[(1, [0, 0, 0, 1000_f32.to_bits()])]);
        }
        if held_rotation && time == 3. {
            p::frame(&mut words, 2.75, &[(4, [0, 0, 0, (-1_f32).to_bits()])]);
        }
        let mut keys = vec![(0, [0, 0, 0, weight.to_bits()])];
        if clamped_translation && time == 2. {
            keys.extend([(1, [0, 0, 1_f32.to_bits(), 0]), (2, [0; 4]), (3, [0; 4])]);
        }
        if held_rotation {
            let w = if time < 2.75 { 1_f32 } else { -1. };
            keys.extend([
                (1, [0; 4]),
                (2, [0; 4]),
                (3, [0; 4]),
                (4, [0, 0, 0, w.to_bits()]),
            ]);
        }
        p::frame(&mut words, time, &keys);
    }
    p::frame(&mut words, f32::INFINITY, &[]);
    let (transform_attributes, streamed_count, constants): (_, _, &[f32]) = match kind {
        GenericClip::ConstantTrs => ([1, 2], 1, &[1., -2., 0., 0., 0., 0., 1.]),
        GenericClip::ClampedTranslation => ([1, 2], 4, &[0., 0., 0., 1.]),
        GenericClip::HeldQuaternionFlip => ([2, 1], 5, &[1., -2., 0.]),
    };
    let [first, second] = transform_attributes;
    let constants = constants.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    p::add_compiled(
        &mut object,
        Layout::Unity2022,
        &[
            Binding {
                class: 137,
                custom: 20,
                attribute: 2_753_195_566,
                ..Binding::default()
            },
            Binding {
                class: 4,
                attribute: first,
                ..Binding::default()
            },
            Binding {
                class: 4,
                attribute: second,
                ..Binding::default()
            },
        ],
        &[],
        Storage {
            interval: [2., 3.],
            streamed_count,
            words: &words,
            constants: &constants,
            ..Storage::default()
        },
    );
    if let V::Struct(_, fields) = &mut object.value
        && let Some((_, muscle)) = fields.iter_mut().find(|(name, _)| name == "m_MuscleClip")
    {
        s::set(muscle, "m_LoopTime", V::Bool(false));
    }
    object
}

pub fn bundle(legacy: bool) -> Vec<u8> {
    bundle_with_clip(
        legacy,
        if legacy {
            legacy_clip()
        } else {
            generic_clip(GenericClip::ConstantTrs)
        },
    )
}

pub fn bundle_with_clip(legacy: bool, clip: Object) -> Vec<u8> {
    let mut mesh = m::mesh(
        7,
        b"mesh",
        &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
        &[0, 1, 2],
        m::MeshVersion::V2022,
    );
    s::set(
        &mut mesh.value,
        "m_Shapes",
        m::blend_shapes(
            vec![
                m::blend_vertex(0, [0.5, 0., 0.], [0.; 3], [0.; 3]),
                m::blend_vertex(1, [1., 0., 0.], [0.; 3], [0.; 3]),
            ],
            vec![
                m::blend_frame(0, 1, false, false),
                m::blend_frame(1, 1, false, false),
            ],
            vec![m::blend_channel(b"bend", 0, 0, 2)],
            vec![25., 80.],
        ),
    );
    let mut objects = vec![
        Object::new(1, 1, s::game_object(b"root", &[(0, 2), (0, 3), (0, 6)])),
        Object::new(2, 4, s::transform((0, 1), (0, 0), &[(0, 5)], false)),
        if legacy {
            anim::legacy(3, (0, 9), &[(0, 9)])
        } else {
            anim::animator(3, (0, 10))
        },
        Object::new(4, 1, s::game_object(b"instance", &[(0, 5), (0, 8)])),
        Object::new(5, 4, s::transform((0, 4), (0, 2), &[], false)),
        Object::new(
            6,
            137,
            s::skinned_mesh_renderer((0, 1), (0, 7), &[], &[10.]),
        ),
        mesh,
        Object::new(
            8,
            137,
            s::skinned_mesh_renderer((0, 4), (0, 7), &[], &[30.]),
        ),
        clip,
    ];
    if !legacy {
        objects.push(anim::controller(10, &[(0, 9)], &[0]));
    }
    let index = e::serialized(
        &[CAB],
        &[e::Obj::asset_bundle(
            100,
            e::Bundle {
                rows: vec![e::row(PREFAB, (0, 0), (1, 1)), e::row(CLIP, (0, 0), (1, 9))],
                ..e::Bundle::default()
            },
        )],
    );
    e::bundle(&[
        ("CAB-index", index),
        (CAB, s::file(Layout::Unity2022, &[], &objects).unwrap()),
    ])
}
