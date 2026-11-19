//! The body model's Avatar inputs on synthetic rigs: the reads round-trip,
//! each E0 fault and scope refusal fires alone, and the recorded topology
//! patterns agree with the f64 reference's.
use super::file;
use std::collections::BTreeSet;
use unityrs_anim::{
    AnimError, BodyAvatarFault, BodyModel,
    humanoid::{
        Avatar, BODY_SLOTS, RECORDED_ABSENT, body_avatar, mass_sum_bound, read_avatar, slot_bone,
    },
};
use unityrs_core::limits::Meter;
use unityrs_testkit::{
    ngui::{Object, V, pair, vector},
    scene3d::{
        Layout,
        anim::humanoid::{self as h, AVATAR, OPTIONAL_BONES, Rig},
        set,
    },
};

/// A rig with every optional body bone and a rest body at `m_Scale`.
fn full() -> Rig {
    Rig {
        optional: OPTIONAL_BONES.iter().map(|&(bone, _)| bone).collect(),
        body_offset: [0., 1., 0.],
        ..Rig::default()
    }
}

/// `full` without the named slots.
fn without(names: &[&str]) -> Rig {
    let mut rig = full();
    rig.optional
        .retain(|&bone| !names.iter().any(|name| slot_named(name) == slot_of(bone)));
    rig
}

fn slot_named(name: &str) -> usize {
    BODY_SLOTS.iter().position(|slot| *slot == name).unwrap()
}

fn slot_of(bone: usize) -> usize {
    (0..BODY_SLOTS.len())
        .find(|&s| slot_bone(s) == bone)
        .unwrap()
}

/// Set `name` in the struct `path` names below `value`.
fn edit(value: &mut V, path: &[&str], name: &str, replacement: Option<V>) {
    match path.split_first() {
        None => match replacement {
            Some(replacement) => set(value, name, replacement),
            None => {
                if let V::Struct(_, fields) = value {
                    fields.retain(|(field, _)| field != name);
                }
            }
        },
        Some((first, rest)) => {
            if let V::Struct(_, fields) = value
                && let Some((_, inner)) = fields.iter_mut().find(|(field, _)| field == first)
            {
                edit(inner, rest, name, replacement);
            }
        }
    }
}

const HUMAN: [&str; 3] = ["m_Avatar", "m_Human", "data"];

fn read(avatar: Object, layout: Layout) -> Result<Avatar, AnimError> {
    let (_, file) = file(layout, &[avatar]);
    read_avatar(&file.object(AVATAR).unwrap(), &Meter::unlimited())
}

/// The body-model verdict on a rig's Avatar with one `m_Human` field edited.
fn verdict_with(rig: &Rig, edits: &[(&[&str], &str, Option<V>)]) -> Result<(), String> {
    let mut avatar = h::avatar(rig);
    for (path, name, value) in edits {
        edit(&mut avatar.value, path, name, value.clone());
    }
    let avatar = read(avatar, Layout::Unity2022).map_err(|e| e.to_string())?;
    body_avatar(&avatar, &Meter::unlimited())
        .map(drop)
        .map_err(|e| e.to_string())
}

fn verdict(rig: &Rig) -> Result<(), String> {
    verdict_with(rig, &[])
}

/// Dyadic masses summing to one exactly: 1/64 per slot but Hips, 3/128 on
/// the left side's, and the rest (71/128) on Hips.
fn masses() -> Vec<f32> {
    let mut out: Vec<f32> = (0..25)
        .map(|slot| match slot {
            0 => 0.,
            s if BODY_SLOTS[s].starts_with("Left") => 3. / 128.,
            _ => 1. / 64.,
        })
        .collect();
    out[0] = 1. - out.iter().sum::<f32>();
    out
}

/// The reads against the testkit's synthetic Avatar schema under both
/// version headers: the same hand-built type tree and payload, so this
/// checks no real Editor layout. The real layouts are the gated
/// `recorded_humanoid_refusals` test's (`unityrs-export`), which runs
/// `body_avatar` on every Editor-built Avatar of a completed run.
#[test]
fn the_body_inputs_round_trip_on_both_layouts() {
    for layout in [Layout::Unity2022, Layout::Unity6000] {
        let rig = Rig {
            bone_mass: Some(masses()),
            scale: 1.25,
            body_offset: [0.5, 1.25, -0.25],
            ..full()
        };
        let avatar = read(h::avatar(&rig), layout).unwrap();
        let meter = Meter::unlimited();
        let body = body_avatar(&avatar, &meter).unwrap();
        assert_eq!(body.masses.as_slice(), masses().as_slice());
        assert_eq!(body.scale(), 1.25);
        assert_eq!(body.root().t, [0.5, 1.25, -0.25]);
        for (slot, name) in BODY_SLOTS.iter().enumerate() {
            let node = avatar.human.bone_node(slot_bone(slot));
            assert_eq!(body.nodes[slot], node, "{name}");
            assert!(node.is_some());
            assert_eq!(body.rest(slot).map(|x| x.q), Some([0., 0., 0., 1.]));
            let path = body.slot_path(slot, &meter).unwrap().unwrap();
            let want = match slot {
                0 => "Hips".to_owned(),
                _ => format!("Hips/{name}"),
            };
            assert_eq!(path.as_bytes(), want.as_bytes());
        }
        assert_eq!(body.human().pose.len(), 26);
    }
}

#[test]
fn every_recorded_topology_is_in_scope() {
    for pattern in RECORDED_ABSENT {
        let names: Vec<_> = (0..25)
            .filter(|slot| pattern & (1 << slot) != 0)
            .map(|slot| BODY_SLOTS[slot])
            .collect();
        let rig = without(&names);
        assert_eq!(verdict(&rig), Ok(()), "absent {names:?}");
        let avatar = read(h::avatar(&rig), Layout::Unity2022).unwrap();
        let body = body_avatar(&avatar, &Meter::unlimited()).unwrap();
        for name in &names {
            let slot = slot_named(name);
            assert_eq!((body.nodes[slot], body.masses[slot]), (None, 0.));
        }
    }
}

#[test]
fn the_frozen_scope_refuses_its_topologies_in_order() {
    let refused =
        |cause: BodyModel| Err(AnimError::UnsupportedHumanoidBodyModel(cause).to_string());
    let cases: [(&[&str], BodyModel); 9] = [
        (&["Chest", "UpperChest"], BodyModel::ChestAbsent),
        // Chest absent comes first, whatever else is absent.
        (&["Chest", "Neck", "LeftShoulder"], BodyModel::ChestAbsent),
        (&["Neck"], BodyModel::NeckAbsent),
        (
            &["Neck", "UpperChest", "LeftShoulder"],
            BodyModel::NeckAbsent,
        ),
        (&["LeftShoulder"], BodyModel::OneShoulder),
        (&["RightShoulder", "Jaw"], BodyModel::OneShoulder),
        // One mapped eye, and both toes with the head's extras mapped.
        (&["LeftEye"], BodyModel::UnrecordedTopology),
        (&["LeftToes", "RightToes"], BodyModel::UnrecordedTopology),
        (
            &["Jaw", "LeftEye", "RightEye", "UpperChest", "LeftToes"],
            BodyModel::UnrecordedTopology,
        ),
    ];
    for (names, cause) in cases {
        assert_eq!(verdict(&without(names)), refused(cause), "absent {names:?}");
    }
    assert_eq!(
        refused(BodyModel::NeckAbsent),
        Err("unsupported_humanoid_body_model(neck_absent)".to_owned())
    );
    // The P2b fixture rig maps the required bones alone: no Chest.
    assert_eq!(
        verdict(&Rig {
            body_offset: [0., 1., 0.],
            ..Rig::default()
        }),
        refused(BodyModel::ChestAbsent)
    );
}

#[test]
fn malformed_masses_refuse_with_their_faults() {
    let fault = |fault: BodyAvatarFault| Err(AnimError::InvalidBodyAvatar(fault).to_string());
    let with = |values: Vec<f32>| Rig {
        bone_mass: Some(values),
        ..full()
    };
    let mut nan = masses();
    nan[7] = f32::NAN;
    let mut infinite = masses();
    infinite[3] = f32::INFINITY;
    let mut negative = masses();
    negative[11] = -1. / 64.;
    negative[0] += 2. / 64.;
    let heavy: Vec<f32> = masses().iter().map(|m| m * 1.0001).collect();
    let cases = [
        (
            with(masses()[..24].to_vec()),
            BodyAvatarFault::MassLength(24),
        ),
        (
            with([masses(), vec![0.]].concat()),
            BodyAvatarFault::MassLength(26),
        ),
        (with(nan), BodyAvatarFault::MassNonFinite(7)),
        (with(infinite), BodyAvatarFault::MassNonFinite(3)),
        (with(negative), BodyAvatarFault::MassNegative(11)),
        (
            // masses() gives Jaw a share.
            Rig {
                bone_mass: Some(masses()),
                ..without(&["Jaw"])
            },
            BodyAvatarFault::MassAbsentSlot(24),
        ),
        (with(heavy), BodyAvatarFault::MassSum),
        (with(vec![0.; 25]), BodyAvatarFault::MassSum),
    ];
    for (rig, want) in cases {
        assert_eq!(verdict(&rig), fault(want), "{want}");
    }
    assert_eq!(
        verdict_with(&full(), &[(&HUMAN, "m_HumanBoneMass", None)]),
        fault(BodyAvatarFault::MassMissing)
    );
    // A mass array of another element type is malformed on read.
    let ints = vector(V::Int(0), vec![V::Int(0); 25]);
    assert_eq!(
        verdict_with(&full(), &[(&HUMAN, "m_HumanBoneMass", Some(ints))]),
        Err("malformed animation field: m_HumanBoneMass".to_owned())
    );
}

#[test]
fn the_mass_sum_holds_within_its_derived_bound() {
    let bound = mass_sum_bound(1.);
    let gamma = |n: f64, u: f64| n * u / (1. - n * u);
    assert_eq!(
        bound,
        gamma(50., 2f64.powi(-24)) + gamma(25., 2f64.powi(-53))
    );
    // Hips (71/128, ulp 2^-24) one f32 step either way stays inside, and
    // so do 48 steps: gamma(50) at f32 is just over 50 of them.
    for step in [-1_i32, 1, 48] {
        let mut values = masses();
        let bits = values[0].to_bits().wrapping_add_signed(step);
        values[0] = f32::from_bits(bits);
        let rig = Rig {
            bone_mass: Some(values),
            ..full()
        };
        assert_eq!(verdict(&rig), Ok(()), "{step}");
    }
    // A sum off by twice the bound refuses.
    let mut values = masses();
    #[allow(clippy::cast_possible_truncation, reason = "a test perturbation")]
    let off = (2. * bound) as f32;
    values[0] += off;
    let rig = Rig {
        bone_mass: Some(values),
        ..full()
    };
    assert_eq!(
        verdict(&rig),
        Err(AnimError::InvalidBodyAvatar(BodyAvatarFault::MassSum).to_string())
    );
}

#[test]
fn the_slot_table_and_rest_body_refuse_with_their_faults() {
    let fault = |fault: BodyAvatarFault| Err(AnimError::InvalidBodyAvatar(fault).to_string());
    // Rig bone 3 is LeftLowerLeg, slot 3.
    let no_tos = Rig {
        no_tos: Some(3),
        ..full()
    };
    assert_eq!(verdict(&no_tos), fault(BodyAvatarFault::SlotPath(3)));
    // Human node 6 is LeftFoot (slot 5) and node 4 LeftLowerLeg (slot 3).
    let ids: Vec<u32> = (0..26).map(|i| i * 7 + 1).collect();
    let names: Vec<String> = (0..26).map(|i| format!("n{i}")).collect();
    assert_eq!(relabelled(&ids, &names), Ok(()));
    // Node 6 takes node 4's ID, which keeps its m_TOS path.
    let mut shared = ids.clone();
    shared[6] = shared[4];
    assert_eq!(
        relabelled(&shared, &names),
        fault(BodyAvatarFault::SlotAlias(5))
    );
    // Distinct IDs whose m_TOS entries name one path.
    let mut same = names.clone();
    same[6] = same[4].clone();
    assert_eq!(
        relabelled(&ids, &same),
        fault(BodyAvatarFault::SlotAlias(5))
    );
    for scale in [0., -1.] {
        let rig = Rig {
            scale,
            body_offset: [0., scale, 0.],
            ..full()
        };
        assert_eq!(verdict(&rig), fault(BodyAvatarFault::Scale), "{scale}");
    }
    // m_RootX.t.y one f32 step off m_Scale passes; 1% off refuses.
    let near = Rig {
        body_offset: [0., f32::from_bits(1f32.to_bits() + 1), 0.],
        ..full()
    };
    assert_eq!(verdict(&near), Ok(()));
    let far = Rig {
        body_offset: [0., 1.01, 0.],
        ..full()
    };
    assert_eq!(verdict(&far), fault(BodyAvatarFault::RootHeight));
}

/// The verdict on `full`'s Avatar with human node `i` given skeleton ID
/// `ids[i]` and the `m_TOS` path `paths[i]` (one entry per distinct ID, the
/// first path written for it).
fn relabelled(ids: &[u32], paths: &[String]) -> Result<(), String> {
    let mut avatar = h::avatar(&full());
    let skeleton = ["m_Avatar", "m_Human", "data", "m_Skeleton", "data"];
    let id_list = vector(V::UInt(0), ids.iter().map(|&id| V::UInt(id)).collect());
    edit(&mut avatar.value, &skeleton, "m_ID", Some(id_list));
    let mut seen = BTreeSet::new();
    let pairs = ids
        .iter()
        .zip(paths)
        .filter(|&(id, _)| seen.insert(*id))
        .map(|(&id, path)| pair(V::UInt(id), V::Str(path.clone().into_bytes())))
        .collect();
    let empty = pair(V::UInt(0), V::Str(Vec::new()));
    edit(&mut avatar.value, &[], "m_TOS", Some(vector(empty, pairs)));
    let avatar = read(avatar, Layout::Unity2022).map_err(|e| e.to_string())?;
    body_avatar(&avatar, &Meter::unlimited())
        .map(drop)
        .map_err(|e| e.to_string())
}

#[test]
fn a_caller_edited_non_finite_scale_or_root_height_refuses() {
    let fault = |fault: BodyAvatarFault| Err(AnimError::InvalidBodyAvatar(fault).to_string());
    let edited = |edit: fn(&mut Avatar)| {
        let mut avatar = read(h::avatar(&full()), Layout::Unity2022).unwrap();
        edit(&mut avatar);
        body_avatar(&avatar, &Meter::unlimited())
            .map(drop)
            .map_err(|e| e.to_string())
    };
    assert_eq!(edited(|_| {}), Ok(()));
    assert_eq!(
        edited(|a| a.human.scale = f32::NAN),
        fault(BodyAvatarFault::Scale)
    );
    assert_eq!(
        edited(|a| a.human.scale = f32::INFINITY),
        fault(BodyAvatarFault::Scale)
    );
    assert_eq!(
        edited(|a| a.human.root.t[1] = f32::NAN),
        fault(BodyAvatarFault::RootHeight)
    );
    assert_eq!(
        edited(|a| a.human.root.t[1] = f32::INFINITY),
        fault(BodyAvatarFault::RootHeight)
    );
}

/// The reference's `RECORDED_ABSENT` patterns, read from its source.
fn reference_patterns() -> BTreeSet<BTreeSet<String>> {
    let source = include_str!("../../../../tools/parity/anim_body_model.py");
    let start = source.find("RECORDED_ABSENT = ").unwrap();
    let body = &source[start..];
    let body = &body[..body.find("))\n").unwrap()];
    let body = &body[body.find("(absent) for absent in (").unwrap() + 24..];
    let mut out = BTreeSet::new();
    let mut rest = body;
    while let Some(open) = rest.find('(') {
        let close = open + rest[open..].find(')').unwrap();
        let names = rest[open + 1..close]
            .split(',')
            .map(|name| name.trim().trim_matches('"').to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        out.insert(names);
        rest = &rest[close + 1..];
    }
    out
}

#[test]
fn the_recorded_patterns_are_the_references() {
    let ours: BTreeSet<BTreeSet<String>> = RECORDED_ABSENT
        .iter()
        .map(|pattern| {
            (0..25)
                .filter(|slot| pattern & (1 << slot) != 0)
                .map(|slot| BODY_SLOTS[slot].to_owned())
                .collect()
        })
        .collect();
    assert_eq!(ours.len(), RECORDED_ABSENT.len());
    assert_eq!(ours, reference_patterns());
}
