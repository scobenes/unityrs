//! B5a's muscle pose against the Python reference, bit for bit
//! (`tools/parity/anim_muscle_pose.py`): the committed goldens on synthetic
//! axes, and, given `UNITYRS_MUSCLE_POSE_CASES`, the private real-Avatar
//! cases of both Editor versions with their agreement with Unity's
//! recordings.
//!
//! Each row pins the angles bit for bit, the tangents where `tan` is
//! glibc's (where the goldens were made: CI), and the local rotation bit
//! for bit given the row's tangents, so another C library's last-bit
//! difference in `tan` alone is counted, never hidden.
use serde_json::Value;
use std::collections::BTreeMap;
use unityrs_anim::humanoid::{
    Axes, MUSCLE_COUNT,
    muscle_pose::{
        MUSCLE_AXES, bone_muscles, bone_rotation, joint_rotation, local_rotation, muscle_angles,
        muscle_axis, tangents,
    },
};

const GOLDENS: &str = include_str!("muscle_pose.json");
/// Whether `tan` here is glibc's, the goldens' (the CI job's target).
const GLIBC: bool = cfg!(all(target_os = "linux", target_env = "gnu"));

fn f32s<const N: usize>(row: &Value) -> [f32; N] {
    let values: Vec<f32> = row
        .as_array()
        .unwrap()
        .iter()
        .map(|v| f32::from_bits(u32::from_str_radix(v.as_str().unwrap(), 16).unwrap()))
        .collect();
    values.try_into().unwrap()
}

fn bits<const N: usize>(row: &Value) -> [u64; N] {
    let values: Vec<u64> = row
        .as_array()
        .unwrap()
        .iter()
        .map(|v| u64::from_str_radix(v.as_str().unwrap(), 16).unwrap())
        .collect();
    values.try_into().unwrap()
}

fn axes(row: &Value) -> Axes {
    Axes {
        pre: f32s(&row["pre"]),
        post: f32s(&row["post"]),
        sign: f32s(&row["sign"]),
        min: f32s(&row["min"]),
        max: f32s(&row["max"]),
        length: 0.,
        kind: 0,
    }
}

fn values(row: &Value) -> Vec<(f32, usize)> {
    row.as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            let value = u32::from_str_radix(pair[0].as_str().unwrap(), 16).unwrap();
            (
                f32::from_bits(value),
                usize::try_from(pair[1].as_u64().unwrap()).unwrap(),
            )
        })
        .collect()
}

/// The angle between two rotations, in radians (`anim_reference.angle`).
fn angle(q: [f64; 4], r: [f64; 4]) -> f64 {
    let unit = |q: [f64; 4]| {
        let norm = q.iter().map(|x| x * x).sum::<f64>().sqrt();
        q.map(|x| x / norm)
    };
    let (q, mut r) = (unit(q), unit(r));
    if q.iter().zip(&r).map(|(a, b)| a * b).sum::<f64>() < 0. {
        r = r.map(|x| -x);
    }
    let length = |sign: f64| {
        q.iter()
            .zip(&r)
            .map(|(a, b)| (a + sign * b).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    4. * length(-1.).atan2(length(1.))
}

/// The tally of one file's rows.
#[derive(Default)]
struct Checked {
    rows: usize,
    /// Rows whose `tan` here differs from the row's in the last bits.
    libm: usize,
    /// (table, editor, bone name): worst angle to Unity's recording, its
    /// bound, samples, samples outside the bound.
    agreement: BTreeMap<(Table, String, String), Tally>,
}

/// Worst angle to Unity's recording, its bound, samples, samples outside.
type Tally = (f64, f64, usize, usize);

/// Which agreement table an entry's rows go to, as `anim_muscle_pose.py
/// agree` sorts them: a refusal case (`humanoid-twist-refusal`, playback
/// B5a must refuse) is reported apart, never in acceptance or confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Table {
    Acceptance,
    Confirmation,
    Refusal,
}

impl Table {
    fn of(entry: &Value) -> Self {
        if entry["refusal"].as_bool().unwrap_or(false) {
            Self::Refusal
        } else if entry["confirmation"].as_bool().unwrap_or(false) {
            Self::Confirmation
        } else {
            Self::Acceptance
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Acceptance => "Acceptance (rounds 1 to 10)",
            Self::Confirmation => "Confirmation (round 11)",
            Self::Refusal => "Refusal cases (not acceptance)",
        }
    }
}

/// Every row of a reference file, bit for bit.
fn check(document: &Value) -> Checked {
    let mut checked = Checked::default();
    for entry in document["entries"].as_array().unwrap() {
        let axes = axes(&entry["axes"]);
        let rest: [f32; 4] = f32s(&entry["rest"]);
        for row in entry["vectors"].as_array().unwrap() {
            let values = values(&row["values"]);
            let angles = muscle_angles(&axes, &values).unwrap();
            assert_eq!(
                angles.map(f64::to_bits),
                bits::<3>(&row["angles"]),
                "{entry:.80}"
            );
            let want = bits::<3>(&row["tangents"]).map(f64::from_bits);
            let ours = tangents(angles);
            if ours.map(f64::to_bits) != want.map(f64::to_bits) {
                checked.libm += 1;
                assert!(
                    std::hint::black_box(!GLIBC),
                    "glibc's tan differs from the reference's: {row}"
                );
            }
            let local = local_rotation(rest, joint_rotation(&axes, want).unwrap()).unwrap();
            assert_eq!(local.map(f64::to_bits), bits::<4>(&row["local"]), "{row}");
            if ours.map(f64::to_bits) == want.map(f64::to_bits) {
                assert_eq!(bone_rotation(rest, &axes, &values), Some(local));
            }
            if let Some(recorded) = row.get("recorded") {
                let recorded: [f32; 4] = f32s(recorded);
                let turn = angle(local, recorded.map(f64::from));
                let bound = row["bound"].as_f64().unwrap();
                let key = (
                    Table::of(entry),
                    entry["editor"].as_str().unwrap().to_owned(),
                    entry["name"].as_str().unwrap().to_owned(),
                );
                let tally = checked.agreement.entry(key).or_default();
                if turn > tally.0 {
                    (tally.0, tally.1) = (turn, bound);
                }
                tally.2 += 1;
                tally.3 += usize::from(turn > bound);
            }
            checked.rows += 1;
        }
    }
    checked
}

#[test]
fn the_axes_table_is_the_references() {
    let goldens: Value = serde_json::from_str(GOLDENS).unwrap();
    let axes: Vec<u8> = goldens["muscle_axes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| u8::try_from(a.as_u64().unwrap()).unwrap())
        .collect();
    assert_eq!(axes, MUSCLE_AXES);
    assert_eq!(muscle_axis(0), Some((7, 2)));
    assert_eq!(muscle_axis(23), Some((1, 0)));
    assert_eq!(muscle_axis(94), Some((53, 2)));
    assert_eq!(muscle_axis(MUSCLE_COUNT), None);
    // Each bone's muscles in muscle order: the Spine's z, y and x, a
    // finger's proximal Stretched and Spread.
    let mut vector = [0.; MUSCLE_COUNT];
    for (muscle, value) in vector.iter_mut().enumerate() {
        *value = f32::from(u8::try_from(muscle).unwrap());
    }
    assert_eq!(bone_muscles(&vector, 7), [(0., 2), (1., 1), (2., 0)]);
    assert_eq!(bone_muscles(&vector, 24), [(55., 2), (56., 1)]);
    assert_eq!(bone_muscles(&vector, 0), Vec::new());
}

#[test]
fn every_golden_is_the_references_bit_for_bit() {
    let goldens: Value = serde_json::from_str(GOLDENS).unwrap();
    assert_eq!(goldens["kind"], "goldens");
    assert_eq!(goldens["reference"], 2);
    let checked = check(&goldens);
    // Every bone a muscle turns, each muscle at all fifteen values, each
    // bone's muscles jointly.
    assert_eq!(goldens["entries"].as_array().unwrap().len(), 54);
    assert_eq!(checked.rows, 15 * MUSCLE_COUNT + 4 * 54);
    if checked.libm > 0 {
        eprintln!(
            "{} of {} rows: this C library's tan differs from glibc's in the last bits",
            checked.libm, checked.rows
        );
    }
}

/// Reference 2's angle rule: the limit by the value's sign, then the axis
/// sign. On an `m_Sgn` -1 axis with limits -60 and +100 degrees (Unity's
/// `LeftUpperArm` z), values +1, -1, +0.5 and -0.5 turn -100, +60, -50 and
/// +30 degrees; the hypothesis it replaced turned -60, +100, -30 and +50.
/// With `m_Sgn` +1, or limits mirror images, the two rules are one.
#[test]
fn the_limit_follows_the_values_sign() {
    let degrees = |d: f32| d.to_radians();
    let mut axes = Axes {
        pre: [0., 0., 0., 1.],
        post: [0., 0., 0., 1.],
        sign: [1., 1., -1.],
        min: [degrees(-40.), degrees(-30.), degrees(-60.)],
        max: [degrees(40.), degrees(30.), degrees(100.)],
        length: 0.,
        kind: 0,
    };
    let z = |axes: &Axes, v: f32| muscle_angles(axes, &[(v, 2)]).unwrap()[2];
    for (v, want) in [(1., -100.), (-1., 60.), (0.5, -50.), (-0.5, 30.)] {
        let limit = if v >= 0. { axes.max[2] } else { -axes.min[2] };
        assert_eq!(z(&axes, v), -(f64::from(v) * f64::from(limit)));
        assert!(
            (z(&axes, v).to_degrees() - want).abs() < 1e-4,
            "{v}: {}",
            z(&axes, v).to_degrees()
        );
    }
    // A zero of either sign takes the max limit, its sign through m_Sgn.
    assert_eq!(z(&axes, 0.).to_bits(), (-0_f64).to_bits());
    assert_eq!(z(&axes, -0.).to_bits(), 0_f64.to_bits());
    // m_Sgn +1: the value times its own side's limit.
    axes.sign[2] = 1.;
    assert_eq!(z(&axes, -1.), -f64::from(-axes.min[2]));
}

#[test]
fn a_bad_axis_or_zero_quaternion_is_none() {
    let axes = Axes {
        pre: [0.; 4],
        post: [0., 0., 0., 1.],
        sign: [1.; 3],
        min: [-1.; 3],
        max: [1.; 3],
        length: 0.,
        kind: 0,
    };
    assert_eq!(muscle_angles(&axes, &[(1., 3)]), None);
    assert_eq!(bone_rotation([0., 0., 0., 1.], &axes, &[(1., 0)]), None);
}

/// The private real-Avatar cases (`anim_muscle_pose.py cases`), when
/// `UNITYRS_MUSCLE_POSE_CASES` names them: bit for bit, then each bone's
/// agreement with Unity's recordings, reported. Disagreeing bones are
/// B5a's findings, never tuned here.
#[test]
fn real_avatar_cases_when_given() {
    let Some(path) = std::env::var_os("UNITYRS_MUSCLE_POSE_CASES") else {
        return;
    };
    let document: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(document["kind"], "cases");
    let checked = check(&document);
    let editors: std::collections::BTreeSet<&str> = document["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["editor"].as_str().unwrap())
        .collect();
    assert_eq!(editors.into_iter().collect::<Vec<_>>(), ["2022", "6000"]);
    eprintln!(
        "{} rows bit for bit, {} with another C library's tan",
        checked.rows, checked.libm
    );
    let mut rows: Vec<_> = checked.agreement.iter().collect();
    rows.sort_by_key(|((table, editor, name), _)| {
        (*table, name != "Spine", name.clone(), editor.clone())
    });
    let mut title = None;
    for ((table, editor, name), (worst, bound, count, outside)) in rows {
        if title != Some(*table) {
            title = Some(*table);
            eprintln!("{}:", table.title());
        }
        let verdict = if *outside == 0 {
            "agrees".to_owned()
        } else {
            format!("DISAGREES in {outside}")
        };
        eprintln!(
            "{editor} {name:16} worst {:.3e} deg  bound {:.3e} deg  {count} samples  {verdict}",
            worst.to_degrees(),
            bound.to_degrees()
        );
    }
}

/// A refusal case's rows go to their own table, never acceptance's or
/// confirmation's, as Python's `agree` and `confirm` sort them.
#[test]
fn refusal_rows_are_a_table_apart() {
    let entry = |refusal: bool, confirmation: bool| serde_json::json!({"refusal": refusal, "confirmation": confirmation});
    assert_eq!(Table::of(&entry(false, false)), Table::Acceptance);
    assert_eq!(Table::of(&entry(false, true)), Table::Confirmation);
    assert_eq!(Table::of(&entry(true, false)), Table::Refusal);
    assert_eq!(Table::of(&entry(true, true)), Table::Refusal);
    assert_eq!(Table::of(&serde_json::json!({})), Table::Acceptance);
}
