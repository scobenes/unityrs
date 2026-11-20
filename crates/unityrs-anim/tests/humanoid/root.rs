//! Freeze v8's root projection (plan section 2.4, B4a): the admitted E4
//! configurations, the reference's `project` and `animator_body` on known
//! vectors, E4's sampling and its error bounds, and the refusals.
use super::{
    body_model::{refused, verdict},
    file,
};
use unityrs_anim::{
    AnimError, BodyModel, Clip,
    humanoid::root::{self, Cell, RootRules, U32},
    read_clip,
};
use unityrs_core::limits::Meter;
use unityrs_testkit::{
    ngui::{Object, V, st, vector},
    scene3d::{
        Layout,
        anim::{
            humanoid::{self as h, CLIP, Rig},
            playback as p,
        },
        xyz,
    },
};

const BODY: [u32; 7] = [7, 8, 9, 10, 11, 12, 13];
const MOTION: [u32; 7] = [0, 1, 2, 3, 4, 5, 6];
/// `RootT`, then the raw `RootQ` (normalized as it samples).
const POSE: [f32; 7] = [0.25, 1., -0.5, 0.1, 0.3, -0.05, 0.9];
const START_T: [f32; 3] = [0.5, 0.875, 0.25];
const START_Q: [f32; 4] = [0., 0.2, 0., 0.98];
const SCALE: f64 = 1.25;
/// `MotionT`, then the raw `MotionQ`.
const MOTION_POSE: [f32; 7] = [1., 0.5, 2., 0., 0.6, 0., 0.8];

fn start() -> (&'static str, V) {
    start_at(START_T, START_Q)
}

fn start_at(t: [f32; 3], q: [f32; 4]) -> (&'static str, V) {
    (
        "m_StartX",
        st(
            "xform",
            vec![
                ("t", xyz("Vector3f", &t)),
                ("q", xyz("Quaternionf", &q)),
                ("s", xyz("Vector3f", &[1.; 3])),
            ],
        ),
    )
}

fn settings(flags: &[&'static str], extra: &[(&'static str, V)]) -> Vec<(&'static str, V)> {
    let mut all: Vec<_> = flags.iter().map(|&f| (f, V::Bool(true))).collect();
    all.extend(extra.iter().cloned());
    all.push(start());
    all
}

/// A clip whose `dofs` take the constant `values`, read on the default rig.
fn constant_clip(dofs: &[u32], values: &[f32], flags: &[(&'static str, V)]) -> Clip {
    let constants: Vec<u32> = values.iter().map(|v| v.to_bits()).collect();
    read(h::clip_stored(
        dofs,
        &[],
        flags,
        p::Storage {
            interval: [0., 1.],
            constants: &constants,
            ..p::Storage::default()
        },
    ))
}

fn read(clip: Object) -> Clip {
    let mut objects = h::prefab(&Rig::default());
    objects.push(clip);
    let (_, file) = file(Layout::Unity2022, &objects);
    read_clip(&file.object(CLIP).unwrap(), &Meter::unlimited()).unwrap()
}

fn pose(clip: &Clip, time: f64) -> Result<([f64; 3], [f64; 4]), AnimError> {
    root::target_body_pose(clip, time, SCALE, &Meter::unlimited())
        .map(|pose| (pose.translation, pose.rotation))
}

fn assert_close(got: ([f64; 3], [f64; 4]), want: [u64; 7]) {
    let want = want.map(f64::from_bits);
    for (g, w) in got.0.iter().chain(&got.1).zip(&want) {
        assert!((g - w).abs() <= 4e-16, "{got:?} vs {want:?}");
    }
}

const BAKE_YAW: &str = "m_LoopBlendOrientation";
const BAKE_Y: &str = "m_LoopBlendPositionY";
const BAKE_XZ: &str = "m_LoopBlendPositionXZ";
const KEEP_YAW: &str = "m_KeepOriginalOrientation";
const KEEP_Y: &str = "m_KeepOriginalPositionY";
const KEEP_XZ: &str = "m_KeepOriginalPositionXZ";

/// The fixture's number after `key` in `tools/parity/anim_fixture_body.py`.
fn fixture_number(key: &str) -> f32 {
    let source = include_str!("../../../../tools/parity/anim_fixture_body.py");
    let at = source.find(key).unwrap() + key.len();
    let end = at + source[at..].find([',', ')']).unwrap();
    source[at..end].trim().parse().unwrap()
}

/// A variant's settings, from its name as E4's fixture spells it
/// (`_flags`: `-o<bit>-y<bit>-xz<bit>`) and the fixture's level and
/// orientation offset.
pub(super) fn variant_settings(name: &str) -> Vec<(&'static str, V)> {
    let mut extra = Vec::new();
    let mut flags = Vec::new();
    for (prefix, fields) in [
        ("bake", [BAKE_YAW, BAKE_Y, BAKE_XZ]),
        ("keep", [KEEP_YAW, KEEP_Y, KEEP_XZ]),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            let bits = rest.trim_end_matches("-motion");
            for (part, field) in bits.trim_start_matches('-').split('-').zip(fields) {
                if part.ends_with('1') {
                    flags.push(field);
                }
            }
        }
    }
    match name {
        "level" => extra.push(("m_Level", V::Float(fixture_number("dict(level=")))),
        "orientation-offset" => extra.push((
            "m_OrientationOffsetY",
            V::Float(fixture_number("dict(orientation=")),
        )),
        _ => {}
    }
    settings(&flags, &extra)
}

/// A variant's clip as E4 played it: the body slots at `POSE`, and the
/// motion slots at `MOTION_POSE` where the variant has them.
pub(super) fn variant_clip(name: &str) -> Clip {
    let settings = variant_settings(name);
    if name.ends_with("-motion") {
        full_clip(&settings)
    } else {
        constant_clip(&BODY, &POSE, &settings)
    }
}

/// A clip with the body and motion slots at `POSE` and `MOTION_POSE`.
pub(super) fn full_clip(settings: &[(&'static str, V)]) -> Clip {
    let mut dofs = BODY.to_vec();
    dofs.extend(MOTION);
    let mut values = POSE.to_vec();
    values.extend(MOTION_POSE);
    constant_clip(&dofs, &values, settings)
}

/// Expected `(T_A, B_A)` bits from the f64 reference (`anim_body_model`'s
/// `cells`, `project` and `animator_body` under Python 3.15.2, on each
/// variant's `CLIP_VARIANTS` settings) on `POSE`, `MOTION_POSE`,
/// `m_StartX` and `SCALE`.
const VECTORS: [(&str, [u64; 7]); 11] = [
    (
        "bake-o0-y0-xz1",
        [
            0x3fd4_0000_2800_000a,
            0x0000_0000_0000_0000,
            0xbfed_ffff_f955_554c,
            0x3fbd_a951_420d_219d,
            0x0000_0000_0000_0000,
            0xbf90_f309_b43e_bebb,
            0x3fef_c7b2_6993_1e1a,
        ],
    ),
    (
        "bake-o0-y1-xz0",
        [
            0x0000_0000_0000_0000,
            0x3fc4_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x3fbd_a951_420d_219d,
            0x0000_0000_0000_0000,
            0xbf90_f309_b43e_bebb,
            0x3fef_c7b2_6993_1e1a,
        ],
    ),
    (
        "bake-o0-y1-xz1",
        [
            0x3fd4_0000_2800_000a,
            0x3fc4_0000_0000_0000,
            0xbfed_ffff_f955_554c,
            0x3fbd_a951_420d_219d,
            0x0000_0000_0000_0000,
            0xbf90_f309_b43e_bebb,
            0x3fef_c7b2_6993_1e1a,
        ],
    ),
    (
        "bake-o1-y0-xz0",
        [
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x3fbc_effd_9831_c92d,
            0x3fbe_8b8b_f518_2bae,
            0xbf9f_14bb_0da0_0863,
            0x3fef_8cc4_6970_36de,
        ],
    ),
    (
        "bake-o1-y0-xz1",
        [
            0x3fb4_7098_47de_76ae,
            0x0000_0000_0000_0000,
            0xbfef_84f6_e3ed_896e,
            0x3fbc_effd_9831_c92d,
            0x3fbe_8b8b_f518_2bae,
            0xbf9f_14bb_0da0_0863,
            0x3fef_8cc4_6970_36de,
        ],
    ),
    (
        "bake-o1-y1-xz0",
        [
            0x0000_0000_0000_0000,
            0x3fc4_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x3fbc_effd_9831_c92d,
            0x3fbe_8b8b_f518_2bae,
            0xbf9f_14bb_0da0_0863,
            0x3fef_8cc4_6970_36de,
        ],
    ),
    (
        "bake-o1-y1-xz1",
        [
            0x3fb4_7098_47de_76ae,
            0x3fc4_0000_0000_0000,
            0xbfef_84f6_e3ed_896e,
            0x3fbc_effd_9831_c92d,
            0x3fbe_8b8b_f518_2bae,
            0xbf9f_14bb_0da0_0863,
            0x3fef_8cc4_6970_36de,
        ],
    ),
    (
        "keep-o0-y0-xz0",
        [
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x3fbd_a951_420d_219d,
            0x0000_0000_0000_0000,
            0xbf90_f309_b43e_bebb,
            0x3fef_c7b2_6993_1e1a,
        ],
    ),
    (
        "keep-o1-y1-xz1-motion",
        [
            0x4005_e666_6c14_7ade,
            0x3fe4_0000_0000_0000,
            0xbffc_6666_54e1_47b1,
            0x3fbd_7aaf_3a5f_3286,
            0xbfd4_1977_6638_4e64,
            0x3f95_707f_88a2_29e6,
            0x3fee_2633_26ba_c547,
        ],
    ),
    (
        "level",
        [
            0x0000_0000_0000_0000,
            0xbfc4_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x3fbd_a951_420d_219d,
            0x0000_0000_0000_0000,
            0xbf90_f309_b43e_bebb,
            0x3fef_c7b2_6993_1e1a,
        ],
    ),
    (
        "orientation-offset",
        [
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x3fbd_bf56_7aad_7bcb,
            0xbfd0_7358_bbf4_016b,
            0x3f8c_abfb_d6e5_f75c,
            0x3fee_b27a_d26a_d2e4,
        ],
    ),
];

/// Each admitted variant, with its motion slots, projects to the
/// reference's pose and reads back as its own `ADMITTED` row and cells.
#[test]
fn every_admitted_variant_matches_the_reference_vectors() {
    use Cell::{Bake, Body, Original};
    let meter = Meter::unlimited();
    for (name, want) in VECTORS {
        let clip = variant_clip(name);
        let settings = root::root_settings(&clip, &meter).unwrap();
        assert_eq!(settings.admitted().variant, name);
        let row = settings.admitted();
        let cell = |k: usize| match (row.bake[k], row.keep[k]) {
            (true, _) => Bake,
            (false, true) => Original,
            (false, false) => Body,
        };
        let rules = RootRules {
            yaw: cell(0),
            height: cell(1),
            xz: cell(2),
        };
        assert_eq!(settings.rules(), rules, "{name}");
        assert_eq!(settings.start().0, START_T.map(f64::from));
        assert_close(pose(&clip, 0.5).unwrap(), want);
    }
}

/// The unverified areas refuse: mixed or masked Original cells, Based
/// Upon Feet, start-at-origin off, and every level or orientation offset
/// but the recorded ones.
#[test]
fn unverified_settings_refuse() {
    let meter = Meter::unlimited();
    let refusal = |flags: &[&'static str], extra: &[(&'static str, V)]| {
        let clip = full_clip(&settings(flags, extra));
        root::root_settings(&clip, &meter).unwrap_err().to_string()
    };
    let basis = |f| AnimError::UnsupportedHumanoidRootBasis(f).to_string();
    let clip_settings = |f| AnimError::UnsupportedHumanoidClipSettings(f).to_string();
    assert_eq!(refusal(&[KEEP_Y], &[]), basis(KEEP_Y));
    assert_eq!(refusal(&[KEEP_YAW, KEEP_XZ], &[]), basis(KEEP_YAW));
    // A bake flag never masks a keep-original flag.
    assert_eq!(refusal(&[BAKE_YAW, KEEP_YAW], &[]), basis(KEEP_YAW));
    assert_eq!(
        refusal(&[BAKE_YAW, BAKE_Y, BAKE_XZ, KEEP_XZ], &[]),
        basis(KEEP_XZ)
    );
    assert_eq!(
        refusal(&["m_HeightFromFeet"], &[]),
        basis("m_HeightFromFeet")
    );
    let off = [("m_StartAtOrigin", V::Bool(false))];
    assert_eq!(refusal(&[], &off), basis("m_StartAtOrigin"));
    let level = ("m_Level", V::Float(0.125));
    let offset = ("m_OrientationOffsetY", V::Float(30.));
    assert_eq!(
        refusal(&[BAKE_Y], std::slice::from_ref(&level)),
        clip_settings("m_Level")
    );
    assert_eq!(
        refusal(&[], &[level.clone(), offset.clone()]),
        clip_settings("m_OrientationOffsetY")
    );
    // Only the recorded values: 0.125 and 30, never another.
    assert_eq!(
        refusal(&[], &[("m_Level", V::Float(0.25))]),
        clip_settings("m_Level")
    );
    assert_eq!(
        refusal(&[], &[("m_Level", V::Float(-0.))]),
        clip_settings("m_Level")
    );
    assert_eq!(
        refusal(&[], &[("m_OrientationOffsetY", V::Float(45.))]),
        clip_settings("m_OrientationOffsetY")
    );
    assert_eq!(
        refusal(&[BAKE_YAW], std::slice::from_ref(&offset)),
        clip_settings("m_OrientationOffsetY")
    );
    assert_eq!(
        refusal(&[KEEP_YAW, KEEP_Y, KEEP_XZ], &[offset]),
        clip_settings("m_OrientationOffsetY")
    );
    for flag in ["m_Mirror", "m_LoopTime", "m_LoopBlend"] {
        assert_eq!(refusal(&[flag], &[]), clip_settings(flag));
    }
    let cycle = [("m_CycleOffset", V::Float(0.5))];
    assert_eq!(refusal(&[], &cycle), clip_settings("m_CycleOffset"));
}

/// Missing, partial, degenerate or non-finite root slots and starts refuse.
#[test]
fn unusable_root_slots_refuse() {
    let basis = Err(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray").to_string());
    let keep = settings(&[KEEP_YAW, KEEP_Y, KEEP_XZ], &[]);
    // All Original without motion slots, no body slots, partial slots.
    assert_eq!(
        verdict(pose(&constant_clip(&BODY, &POSE, &keep), 0.5)),
        basis.clone()
    );
    let none = constant_clip(&[42], &[0.], &settings(&[], &[]));
    assert_eq!(verdict(pose(&none, 0.5)), basis.clone());
    let partial = constant_clip(&BODY[..6], &POSE[..6], &settings(&[], &[]));
    assert_eq!(verdict(pose(&partial, 0.5)), basis);
    let zero = [0.25, 1., -0.5, 0., 0., 0., 0.];
    let clip = constant_clip(&BODY, &zero, &settings(&[], &[]));
    assert_eq!(
        verdict(pose(&clip, 0.5)),
        Err(refused(BodyModel::DegenerateRootRotation))
    );
    // A half turn about X has no twist about up.
    let flipped = [0.25, 1., -0.5, 1., 0., 0., 0.];
    let clip = constant_clip(&BODY, &flipped, &settings(&[], &[]));
    assert_eq!(
        verdict(pose(&clip, 0.5)),
        Err(refused(BodyModel::UndefinedYaw))
    );
    // Outside the clip's interval.
    let clip = constant_clip(&BODY, &POSE, &settings(&[], &[]));
    assert_eq!(
        verdict(pose(&clip, 2.)),
        Err(AnimError::InvalidSchedule.to_string())
    );
    // A non-finite motion translation.
    let mut dofs = BODY.to_vec();
    dofs.extend(MOTION);
    let mut values = POSE.to_vec();
    values.extend([f32::NAN, 0.5, 2., 0., 0.6, 0., 0.8]);
    let clip = constant_clip(&dofs, &values, &keep);
    assert_eq!(
        verdict(pose(&clip, 0.5)),
        Err(refused(BodyModel::NonFiniteBody))
    );
    // A non-finite or zero `m_StartX` rotation.
    let baked = |q: [f32; 4]| {
        let mut all = settings(&[BAKE_YAW], &[]);
        all.push(start_at(START_T, q));
        verdict(pose(&constant_clip(&BODY, &POSE, &all), 0.5))
    };
    assert_eq!(
        baked([0., f32::NAN, 0., 1.]),
        Err(refused(BodyModel::NonFiniteBody))
    );
    assert_eq!(
        baked([0.; 4]),
        Err(refused(BodyModel::DegenerateRootRotation))
    );
}

/// Motion slots as the variant has them, all or none, and no additive
/// reference pose: E4 played no other clip.
#[test]
fn motion_slots_and_additive_poses_follow_the_variant() {
    let meter = Meter::unlimited();
    let refusal = |clip: &Clip| verdict(root::root_settings(clip, &meter).map(|_| ()));
    let basis = Err(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray").to_string());
    // Body and Bake variants with motion slots.
    for name in ["keep-o0-y0-xz0", "bake-o1-y1-xz1", "level"] {
        assert_eq!(
            refusal(&full_clip(&variant_settings(name))),
            basis,
            "{name}"
        );
        assert_eq!(refusal(&variant_clip(name)), Ok(()), "{name}");
    }
    // The all-Original variant without them, or with only `MotionT`.
    let original = variant_settings("keep-o1-y1-xz1-motion");
    assert_eq!(refusal(&constant_clip(&BODY, &POSE, &original)), basis);
    let mut dofs = BODY.to_vec();
    dofs.extend(&MOTION[..3]);
    let mut values = POSE.to_vec();
    values.extend(&MOTION_POSE[..3]);
    assert_eq!(refusal(&constant_clip(&dofs, &values, &original)), basis);
    // An additive reference pose.
    let pose = (
        "m_ValueArrayReferencePose",
        vector(V::Float(0.), vec![V::Float(1.)]),
    );
    let clip = constant_clip(&BODY, &POSE, &settings(&[], &[pose]));
    assert_eq!(
        refusal(&clip),
        Err(AnimError::UnsupportedAdditivePose.to_string())
    );
}

/// A root slot bound by any key but E4's (path 0, class 95, its DOF,
/// `customType` 8, `isPPtrCurve` present and clear, no discrete flag)
/// refuses, whatever the flag's encoding.
#[test]
fn only_e4s_binding_key_samples() {
    // The bad value on the first binding; the others clear, in the same
    // encoding, so the type tree declares the encoding under test.
    let cases = [
        ("path", Some((V::UInt(5), V::UInt(0)))),
        ("isPPtrCurve", Some((V::UInt8(1), V::UInt8(0)))),
        ("isPPtrCurve", Some((V::Bool(true), V::Bool(false)))),
        ("isPPtrCurve", Some((V::Int(-1), V::Int(0)))),
        ("isPPtrCurve", None),
        ("isIntCurve", Some((V::Bool(true), V::Bool(false)))),
    ];
    for (field, values) in cases {
        let mut object = h::clip_stored(
            &BODY,
            &[],
            &settings(&[], &[]),
            p::Storage {
                interval: [0., 1.],
                constants: &POSE.map(f32::to_bits),
                ..p::Storage::default()
            },
        );
        let V::Struct(_, root) = &mut object.value else {
            panic!("clip")
        };
        let table = &mut root
            .iter_mut()
            .find(|(name, _)| name == "m_ClipBindingConstant")
            .unwrap()
            .1;
        let V::Struct(_, fields) = table else {
            panic!("binding table")
        };
        let bindings = &mut fields
            .iter_mut()
            .find(|(name, _)| name == "genericBindings")
            .unwrap()
            .1;
        let V::Vector(_, shape, items) = bindings else {
            panic!("bindings")
        };
        let rows = std::iter::once(&mut **shape).chain(items.iter_mut());
        for (k, row) in rows.enumerate() {
            let V::Struct(_, fields) = row else {
                panic!("binding")
            };
            match &values {
                None => fields.retain(|(name, _)| name != field),
                Some((bad, clear)) => {
                    let value = if k == 1 { bad.clone() } else { clear.clone() };
                    match fields.iter_mut().find(|(name, _)| name == field) {
                        Some(at) => at.1 = value,
                        None => fields.push((field.into(), value)),
                    }
                }
            }
        }
        let clip = read(object);
        assert_eq!(
            verdict(pose(&clip, 0.5)),
            Err(AnimError::UnsupportedHumanoidRootBasis("m_ClipBindingConstant").to_string()),
            "{field}"
        );
    }
}

fn gamma(n: f64) -> f64 {
    n * U32 / (1. - n * U32)
}

/// A streamed `RootT.x` samples its Hermite key, with E4's bound: Horner's
/// rounding at its operands plus the clock's.
#[test]
fn streamed_root_slots_sample_with_e4_bounds() {
    let constant = |v: f32| [0, 0, 0, v.to_bits()];
    let mut words = Vec::new();
    let mut first: Vec<_> = (0..7_u32)
        .map(|slot| (slot, constant(POSE[slot as usize])))
        .collect();
    first[0].1 = [0, 0, 1_f32.to_bits(), 0.25_f32.to_bits()];
    p::frame(&mut words, 0., &first);
    let last: Vec<_> = (0..7_u32)
        .map(|slot| (slot, constant(POSE[slot as usize])))
        .collect();
    p::frame(&mut words, 1., &last);
    p::frame(&mut words, f32::INFINITY, &[]);
    let clip = read(h::clip_stored(
        &BODY,
        &[],
        &settings(&[], &[]),
        p::Storage {
            interval: [0., 1.],
            streamed_count: 7,
            words: &words,
            ..p::Storage::default()
        },
    ));
    let sample = root::root_sample(&clip, 0.5, &Meter::unlimited()).unwrap();
    let body = sample.body.unwrap();
    assert!(sample.motion.is_none());
    assert_eq!(body.position, [0.75, 1., -0.5]);
    // `RootT.x` adds the clock's rounding; the held Y and Z keys only
    // Horner's, at their values. `pose_at` takes the norm.
    let errors = [gamma(6.) * 0.75 + U32 * 0.5, gamma(6.), gamma(6.) * 0.5];
    let norm = errors.iter().map(|e| e * e).sum::<f64>().sqrt();
    assert!((body.position_error - norm).abs() <= norm * 1e-15);
}

/// A dense `RootT.y` interpolates its frames, with E4's bound.
#[test]
fn dense_root_slots_sample_with_e4_bounds() {
    let mut bits = Vec::new();
    for frame in [0., 1.] {
        let mut row = POSE;
        row[1] = 1. + frame;
        bits.extend(row.map(f32::to_bits));
    }
    let clip = read(h::clip_stored(
        &BODY,
        &[],
        &settings(&[], &[]),
        p::Storage {
            interval: [0., 1.],
            dense: p::Dense {
                frames: 2,
                curves: 7,
                rate: 1.,
                begin: 0.,
                bits: &bits,
            },
            ..p::Storage::default()
        },
    ));
    let body = root::root_sample(&clip, 0.25, &Meter::unlimited())
        .unwrap()
        .body
        .unwrap();
    assert_eq!(body.position[1], 1.25);
    // Each slot's error, combined by the norm, as E4's `pose_at` does.
    let slot = |a: f64, b: f64| gamma(4.) * (a.abs() + b.abs()) + (b - a).abs() * gamma(4.) * 1.25;
    let errors: Vec<f64> = (0..3)
        .map(|k| {
            let a = f64::from(POSE[k]);
            if k == 1 { slot(1., 2.) } else { slot(a, a) }
        })
        .collect();
    let norm = errors.iter().map(|e| e * e).sum::<f64>().sqrt();
    assert!((body.position_error - norm).abs() <= norm * 1e-15);
}

/// A clip whose `RootT.x` streams the key `[c3, c2, c1, c0]` from `at`
/// (after a constant key at zero when `at` is later), every other root
/// slot constant at `POSE`.
fn streamed_key(at: f32, coefficients: [f32; 4]) -> Clip {
    let constant = |v: f32| [0, 0, 0, v.to_bits()];
    let all = || -> Vec<_> {
        (0..7_u32)
            .map(|slot| (slot, constant(POSE[slot as usize])))
            .collect()
    };
    let mut words = Vec::new();
    let key = coefficients.map(f32::to_bits);
    if at > 0. {
        p::frame(&mut words, 0., &all());
        p::frame(&mut words, at, &[(0, key)]);
    } else {
        let mut first = all();
        first[0].1 = key;
        p::frame(&mut words, 0., &first);
    }
    p::frame(&mut words, 1., &all());
    p::frame(&mut words, f32::INFINITY, &[]);
    read(h::clip_stored(
        &BODY,
        &[],
        &settings(&[BAKE_XZ], &[]),
        p::Storage {
            interval: [0., 1.],
            streamed_count: 7,
            words: &words,
            ..p::Storage::default()
        },
    ))
}

/// A streamed key whose f32 Horner evaluation overflows refuses, though
/// its f64 value is finite: `c1 = c0 = f32::MAX` at `dt = 0.5`; and, in
/// E4's order (the clock rounded to f32, then the key's time taken off),
/// `c1 = 2^105` and `c0 = f32::MAX` from a key at 0.25 at the time
/// `0.5 - 3 * 2^-28`, whose f32 clock is 0.5: `dt = 0.25`, and the sum
/// rounds up past `f32::MAX`. Subtracting first would round `dt` down and
/// keep it finite.
#[test]
fn an_overflowing_f32_key_refuses() {
    let max = f32::MAX;
    let cases = [
        (streamed_key(0., [0., 0., max, max]), 0.5),
        (
            streamed_key(0.25, [0., 0., 2_f32.powi(105), max]),
            0.5 - 3. * 2_f64.powi(-28),
        ),
    ];
    for (clip, time) in cases {
        assert_eq!(
            verdict(root::root_sample(&clip, time, &Meter::unlimited())),
            Err(refused(BodyModel::NonFiniteBody)),
            "{time}"
        );
        assert_eq!(
            verdict(pose(&clip, time)),
            Err(refused(BodyModel::NonFiniteBody))
        );
    }
}

/// A yaw defined exactly is undefined within its sampling error: the same
/// rotation, nearly a half turn about X, projects from constant slots
/// (error 0) and refuses from dense ones (`undefined_yaw`).
#[test]
fn a_yaw_undefined_within_its_sampling_error_refuses() {
    let q = [0.25, 1., -0.5, 1., 1e-7, 0., 1e-7];
    let clip = constant_clip(&BODY, &q, &settings(&[], &[]));
    assert!(pose(&clip, 0.5).is_ok());
    let bits: Vec<u32> = q.iter().chain(&q).map(|v| v.to_bits()).collect();
    let clip = read(h::clip_stored(
        &BODY,
        &[],
        &settings(&[], &[]),
        p::Storage {
            interval: [0., 1.],
            dense: p::Dense {
                frames: 2,
                curves: 7,
                rate: 1.,
                begin: 0.,
                bits: &bits,
            },
            ..p::Storage::default()
        },
    ));
    assert_eq!(
        verdict(pose(&clip, 0.5)),
        Err(refused(BodyModel::UndefinedYaw))
    );
}
