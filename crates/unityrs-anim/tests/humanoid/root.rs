//! Freeze v8's root projection (plan section 2.4, B4a): the admitted E4
//! configurations, the reference's `project` and `animator_body` on known
//! vectors, E4's sampling and its error bounds, and the refusals.
use super::{
    body_model::{refused, verdict},
    file,
};
use unityrs_anim::{
    AnimError, BodyModel, Clip,
    humanoid::root::{self, Cell, RootRules},
    read_clip,
};
use unityrs_core::limits::Meter;
use unityrs_testkit::{
    ngui::{Object, V, st},
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
const U32: f64 = 1. / 16_777_216.;

fn start() -> (&'static str, V) {
    (
        "m_StartX",
        st(
            "xform",
            vec![
                ("t", xyz("Vector3f", &START_T)),
                ("q", xyz("Quaternionf", &START_Q)),
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
    let meter = Meter::unlimited();
    let settings = root::root_settings(clip, &meter)?;
    root::target_body_pose(clip, &settings, time, SCALE, &meter)
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

/// A case's flags, other settings and expected `(T_A, B_A)` bits.
type Case<'a> = (&'a [&'static str], &'a [(&'static str, V)], [u64; 7]);

/// Expected `(T_A, B_A)` bits from the f64 reference (`anim_body_model`'s
/// `cells`, `project` and `animator_body` under Python 3.15.2) on `POSE`,
/// `m_StartX` and `SCALE`.
#[test]
fn the_projection_matches_the_reference_vectors() {
    let level = [("m_Level", V::Float(0.125))];
    let offset = [("m_OrientationOffsetY", V::Float(30.))];
    let cases: [Case<'_>; 4] = [
        (
            &[],
            &level,
            [
                0,
                0xbfc4_0000_0000_0000,
                0,
                0x3fbd_a951_420d_219d,
                0,
                0xbf90_f309_b43e_bebb,
                0x3fef_c7b2_6993_1e1a,
            ],
        ),
        (
            &[],
            &offset,
            [
                0,
                0,
                0,
                0x3fbd_bf56_7aad_7bcb,
                0xbfd0_7358_bbf4_016b,
                0x3f8c_abfb_d6e5_f75c,
                0x3fee_b27a_d26a_d2e4,
            ],
        ),
        (
            &[BAKE_YAW, BAKE_Y],
            &[],
            [
                0,
                0x3fc4_0000_0000_0000,
                0,
                0x3fbc_effd_9831_c92d,
                0x3fbe_8b8b_f518_2bae,
                0xbf9f_14bb_0da0_0863,
                0x3fef_8cc4_6970_36de,
            ],
        ),
        (
            &[BAKE_XZ],
            &[],
            [
                0x3fd4_0000_2800_000a,
                0,
                0xbfed_ffff_f955_554c,
                0x3fbd_a951_420d_219d,
                0,
                0xbf90_f309_b43e_bebb,
                0x3fef_c7b2_6993_1e1a,
            ],
        ),
    ];
    for (flags, extra, want) in cases {
        let clip = constant_clip(&BODY, &POSE, &settings(flags, extra));
        assert_close(pose(&clip, 0.5).unwrap(), want);
    }
    let mut dofs = BODY.to_vec();
    dofs.extend(MOTION);
    let mut values = POSE.to_vec();
    values.extend([1., 0.5, 2., 0., 0.6, 0., 0.8]);
    let clip = constant_clip(&dofs, &values, &settings(&[KEEP_YAW, KEEP_Y, KEEP_XZ], &[]));
    assert_close(
        pose(&clip, 0.5).unwrap(),
        [
            0x4005_e666_6c14_7ade,
            0x3fe4_0000_0000_0000,
            0xbffc_6666_54e1_47b1,
            0x3fbd_7aaf_3a5f_3286,
            0xbfd4_1977_6638_4e64,
            0x3f95_707f_88a2_29e6,
            0x3fee_2633_26ba_c547,
        ],
    );
}

/// E4's eleven passing variants are admitted, with their cells.
#[test]
fn admits_the_e4_passing_configurations() {
    use Cell::{Bake, Body, Original};
    let meter = Meter::unlimited();
    let bake = |o: bool, y: bool, xz: bool| {
        let flags: Vec<&str> = [(o, BAKE_YAW), (y, BAKE_Y), (xz, BAKE_XZ)]
            .into_iter()
            .filter_map(|(on, f)| on.then_some(f))
            .collect();
        let cell = |on| if on { Bake } else { Body };
        (
            flags,
            RootRules {
                yaw: cell(o),
                height: cell(y),
                xz: cell(xz),
            },
        )
    };
    let mut cases: Vec<_> = (0..8)
        .map(|bits| bake(bits & 4 != 0, bits & 2 != 0, bits & 1 != 0))
        .collect();
    cases.push((
        vec![KEEP_YAW, KEEP_Y, KEEP_XZ],
        RootRules {
            yaw: Original,
            height: Original,
            xz: Original,
        },
    ));
    for (flags, rules) in cases {
        let clip = constant_clip(&BODY, &POSE, &settings(&flags, &[]));
        let admitted = root::root_settings(&clip, &meter).unwrap();
        assert_eq!(admitted.rules, rules);
        assert_eq!(admitted.start.0, START_T.map(f64::from));
    }
    for extra in [
        ("m_Level", V::Float(0.125)),
        ("m_OrientationOffsetY", V::Float(30.)),
    ] {
        let clip = constant_clip(&BODY, &POSE, &settings(&[], &[extra]));
        assert!(root::root_settings(&clip, &meter).is_ok());
    }
}

/// The unverified areas refuse: mixed Original cells, Based Upon Feet,
/// start-at-origin off, and the settings E4 never paired.
#[test]
fn unverified_settings_refuse() {
    let meter = Meter::unlimited();
    let refusal = |flags: &[&'static str], extra: &[(&'static str, V)]| {
        let clip = constant_clip(&BODY, &POSE, &settings(flags, extra));
        root::root_settings(&clip, &meter).unwrap_err().to_string()
    };
    let basis = |f| AnimError::UnsupportedHumanoidRootBasis(f).to_string();
    let clip_settings = |f| AnimError::UnsupportedHumanoidClipSettings(f).to_string();
    assert_eq!(refusal(&[KEEP_Y], &[]), basis(KEEP_Y));
    assert_eq!(refusal(&[KEEP_YAW, KEEP_XZ], &[]), basis(KEEP_YAW));
    // A bake flag shadows its keep flag; the other Original cells still mix.
    assert_eq!(refusal(&[BAKE_YAW, KEEP_YAW, KEEP_Y], &[]), basis(KEEP_Y));
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
        clip_settings("m_Level")
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

/// Missing, partial or degenerate root slots refuse.
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
