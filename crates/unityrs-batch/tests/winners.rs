mod artifact_support;
mod attempt_support;
mod common;
#[path = "winners_support/fixtures.rs"]
mod fixtures;
#[path = "winners_support/harness.rs"]
mod harness;
use harness::{Case, candidate};

#[test]
fn unit_reservations_are_not_reoffered_after_whole_unit_conflict() {
    let case = Case::new(vec![
        candidate("a", "u", Some("u"), 1),
        candidate("a", "shared", Some("u"), 1),
        candidate("b", "v", Some("v"), 2),
        candidate("b", "shared", Some("v"), 2),
        candidate("c", "v", None, 3),
    ]);
    case.compare();
}

#[test]
fn exhaustive_small_plain_cases_match_all_decision_fields() {
    use unityrs_export::ngui::Prefer;
    for n in 1_u32..=3 {
        for encoded in 0..4_u32.pow(n) {
            for preference in 0..7 {
                let mut case = Case::new(Vec::new());
                let mut bits = encoded;
                for i in 0..n {
                    let id = format!("i{i}");
                    case.inputs.push(id.clone());
                    match bits % 4 {
                        1 => case.candidates.push(candidate(&id, "key", None, 1)),
                        2 => case.candidates.push(candidate(&id, "key", None, 2)),
                        3 => {
                            case.failed.insert(id);
                        }
                        _ => {}
                    }
                    bits /= 4;
                }
                if preference > 0 {
                    case.prefer.insert(
                        "key".into(),
                        Prefer {
                            input: if preference <= 3 {
                                Some(format!("i{}", (preference - 1) % n))
                            } else {
                                None
                            },
                            md5: if preference >= 3 {
                                Some([u8::try_from(preference - 2).unwrap(); 16])
                            } else {
                                None
                            },
                        },
                    );
                }
                case.compare();
            }
        }
    }
}

#[test]
fn two_thousand_seeded_unit_plain_cases_match_model() {
    use unityrs_export::ngui::Prefer;
    let mut seed = 0x5368_b091_4903_9123_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..2_000 {
        let mut case = Case::new(Vec::new());
        let inputs = next() % 8 + 1;
        for input in 0..inputs {
            let id = format!("i{input}");
            case.inputs.push(id.clone());
            let mut rows = std::collections::BTreeMap::new();
            for u in 0..4 {
                let unit = format!("u{u}");
                if next() % 3 == 0 {
                    continue;
                }
                rows.insert(
                    unit.clone(),
                    candidate(&id, &unit, Some(&unit), u8::try_from(next() % 3).unwrap()),
                );
                for member in 0..12 {
                    if next() % 3 == 0 {
                        let key = format!("k{member:02}");
                        rows.entry(key.clone()).or_insert_with(|| {
                            candidate(&id, &key, Some(&unit), u8::try_from(next() % 3).unwrap())
                        });
                    }
                }
            }
            for key in ["u0", "u1", "u2", "u3", "plain"] {
                if next() % 2 == 0 {
                    rows.entry(key.into()).or_insert_with(|| {
                        candidate(&id, key, None, u8::try_from(next() % 3).unwrap())
                    });
                }
            }
            case.candidates.extend(rows.into_values());
        }
        case.failed.insert("z".into());
        for key in ["u0", "u1", "u2", "k00", "k11", "plain"] {
            if next() % 2 == 0 {
                case.prefer.insert(
                    key.into(),
                    Prefer {
                        input: Some(if next() % 3 == 0 {
                            "z".into()
                        } else {
                            format!("i{}", next() % inputs)
                        }),
                        md5: Some([u8::try_from(next() % 3).unwrap(); 16]),
                    },
                );
            }
        }
        case.compare();
    }
}

#[test]
fn prefix_matches_independent_quadratic_original_set_model() {
    use unityrs_batch::sort::Options;
    use unityrs_export::ngui;
    let keys = ["a", "a/b", "a/b/c", "a!", "a-", "a.", "a0", "é", "é/x", "z"];
    for bits in 1..1_u32 << keys.len() {
        // Every second key is in one valid unit rooted at `a` when present.
        let rows = keys
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(i, key)| {
                candidate(
                    "i",
                    key,
                    if bits & 1 != 0 && i % 2 == 0 {
                        Some("a")
                    } else {
                        None
                    },
                    1,
                )
            })
            .collect();
        let mut case = Case::new(rows);
        case.archives = match bits % 3 {
            0 => vec!["z/archive".into(), "b".into()],
            1 => vec!["a/b".into(), "z".into()],
            _ => vec!["é/x".into(), "a!".into()],
        };
        let mut expected = ngui::decide(&case.candidates, &case.prefer, &case.failed, true);
        let original = expected
            .iter()
            .filter(|d| d.publish)
            .map(|d| d.key.clone())
            .collect::<Vec<_>>();
        let mut affected = std::collections::BTreeSet::new();
        for d in &mut expected {
            if !d.publish {
                continue;
            }
            let prefix = format!("{}/", d.key);
            let ancestor = original
                .iter()
                .chain(&case.archives)
                .any(|k| k.starts_with(&prefix));
            let archive = case
                .archives
                .iter()
                .any(|k| d.key == *k || d.key.starts_with(&format!("{k}/")));
            if ancestor || archive {
                d.publish = false;
                d.reason = Some(if ancestor {
                    "unwritable_key(prefix_conflict)"
                } else {
                    "unwritable_key(archive_conflict)"
                });
                if let Some(unit) = case
                    .candidates
                    .iter()
                    .find(|c| c.key == d.key && c.input == d.winner)
                    .unwrap()
                    .unit
                    .clone()
                {
                    affected.insert(unit);
                }
            }
        }
        for d in &mut expected {
            let winner = case
                .candidates
                .iter()
                .find(|c| c.key == d.key && c.input == d.winner)
                .unwrap();
            if winner.unit.as_ref().is_some_and(|u| affected.contains(u)) && d.publish {
                d.publish = false;
                d.reason = Some("unit_conflict");
            }
            for c in &mut d.candidates {
                if c.unit.as_ref().is_some_and(|u| affected.contains(u)) {
                    c.rule = ngui::Rule::UnitConflict;
                }
            }
        }
        assert_eq!(case.select(Options::default(), true), expected);
    }
}

#[test]
fn production_selection_never_calls_the_in_memory_model() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/winners");
    for entry in std::fs::read_dir(root).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for forbidden in [
            "ngui::decide",
            "winners::decide",
            "BTreeMap",
            "BTreeSet",
            "HashMap",
            "HashSet",
            "format!",
            ".join(",
            ".collect(",
        ] {
            assert!(
                !text.contains(forbidden),
                "production selection contains {forbidden}"
            );
        }
    }
}

#[test]
fn all_seventeen_ngui_winner_fixtures_from_real_b4_attempts() {
    fixtures::replay("ngui", 17);
}
#[test]
fn all_three_sprite_winner_fixtures_from_real_b4_attempts() {
    fixtures::replay("sprites", 3);
}

#[test]
fn prefix_propagation_keeps_losing_instance_rules_and_never_promotes_plain_candidates() {
    use unityrs_export::ngui;
    let case = Case::new(vec![
        candidate("a", "u", Some("u"), 1),
        candidate("a", "x", Some("u"), 1),
        candidate("a", "p", Some("u"), 1),
        candidate("b", "u", Some("u"), 2),
        candidate("b", "x", Some("u"), 2),
        candidate("c", "p", None, 3),
        candidate("a", "x/child", None, 3),
    ]);
    let mut expected = ngui::decide(&case.candidates, &case.prefer, &case.failed, true);
    for d in &mut expected {
        if ["u", "p", "x"].contains(&d.key.as_str()) {
            d.publish = false;
            d.reason = Some(if d.key == "x" {
                "unwritable_key(prefix_conflict)"
            } else {
                "unit_conflict"
            });
            for c in &mut d.candidates {
                if c.unit.as_deref() == Some("u") && c.input == "a" {
                    c.rule = ngui::Rule::UnitConflict;
                }
            }
        }
    }
    assert_eq!(
        case.select(unityrs_batch::sort::Options::default(), true),
        expected
    );
}
