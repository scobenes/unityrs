mod common;
mod publication_support;
use publication_support::Scene;
use std::collections::BTreeMap;
use unityrs_batch::{
    manifest::{self, Family, WriteError, chunks::Destination},
    outcomes::{Archive, Code},
    phase::Account,
    publish::Gate,
    spool::Host,
    winners,
};
#[derive(Default)]
struct Rows {
    rows: BTreeMap<String, Vec<serde_json::Value>>,
    fail: Option<Family>,
}
impl Destination for Rows {
    fn publish(&mut self, family: Family, _: u64, bytes: &[u8]) -> Result<(), WriteError> {
        if self.fail == Some(family) {
            return Err(WriteError::Publish);
        }
        assert!(bytes.len() <= 16 * 1024 * 1024);
        assert_eq!(bytes.last(), Some(&b'\n'));
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            assert!(line.len() <= 32 * 1024);
            self.rows
                .entry(family.name().into())
                .or_default()
                .push(serde_json::from_slice(line).unwrap());
        }
        Ok(())
    }
}
fn assemble(scene: &Scene, failed: Option<Family>) -> (manifest::Report, Rows) {
    let out = scene.outcomes(&format!("out-{}", failed.map_or(8, |f| f as usize)), &Host);
    for (index, code) in [Code::Written, Code::Present, Code::PublishFailed]
        .into_iter()
        .enumerate()
    {
        out.set(index as u64, code).unwrap();
    }
    out.set_archive(
        0,
        &Archive::success(Code::Written, 1234, [1; 16], None).unwrap(),
    )
    .unwrap();
    out.set_archive(
        1,
        &Archive::success(Code::Present, 5678, [2; 16], None).unwrap(),
    )
    .unwrap();
    assemble_outcomes(scene, failed, &out)
}
fn assemble_outcomes(
    scene: &Scene,
    failed: Option<Family>,
    out: &unityrs_batch::outcomes::Outcomes,
) -> (manifest::Report, Rows) {
    let gate = Gate::default();
    let frozen = gate.freeze(out).unwrap();
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let inputs = winners::committed::Committed::received(
        &scene.done,
        &scene.prepared.inputs,
        &Host,
        &|| false,
    )
    .unwrap();
    let mut rows = Rows {
        fail: failed,
        ..Rows::default()
    };
    let report = manifest::assemble(
        &manifest::rows::Context {
            facts: &scene.prepared,
            inputs: &inputs,
            selected: Some(manifest::rows::Selected {
                handoff: &scene.handoff,
                workspace: &scene.workspace,
                outcomes: frozen.outcomes,
            }),
            stop: None,
            found: None,
            account: &account,
            stopped: &|| false,
            taken_at: std::time::Instant::now(),
        },
        &mut rows,
    )
    .unwrap();
    assert_eq!(account.snapshot().input.used, 0);
    assert!(account.snapshot().input.peak <= unityrs_batch::publish::WORKING_SET);
    (report, rows)
}
#[test]
fn all_eight_families_overlay_by_key_and_original_archive_index() {
    let scene = Scene::new(&["z", "a", "m"], true, 4096);
    let (report, rows) = assemble(&scene, None);
    assert_eq!(report.rows, [Some(1); 8]);
    assert_eq!(report.counts.bundles, 2);
    assert_eq!(report.counts.inputs, 2);
    assert_eq!(report.counts.inputs_with_skips, 2);
    assert_eq!(report.counts.collisions, 1);
    assert_eq!(report.counts.flips, 0);
    assert_eq!(rows.rows["collisions"].len(), 2);
    assert_eq!(rows.rows["collisions"][0]["input"], "a");
    assert_eq!(rows.rows["collisions"][0]["winner"], true);
    assert_eq!(rows.rows["collisions"][1]["winner"], false);
    assert_eq!(report.counts.outputs_written, 1);
    assert_eq!(report.counts.outputs_present, 1);
    assert_eq!(report.counts.outputs_failed, 1);
    assert_eq!(report.counts.skips, 2);
    assert_eq!(report.counts.skips_expected, 2);
    assert_eq!(report.counts.archives_written, 1);
    assert_eq!(report.counts.archives_present, 1);
    for (row, (key, status)) in
        rows.rows["outputs"]
            .iter()
            .zip([("a", "written"), ("m", "present"), ("z", "failed")])
    {
        assert_eq!(row["key"], key);
        assert_eq!(row["status"], status);
    }
    assert_eq!(rows.rows["archives"][0]["key"], "a.zip");
    assert_eq!(rows.rows["archives"][0]["size"], 5678);
    assert_eq!(rows.rows["archives"][1]["key"], "z.zip");
    assert_eq!(rows.rows["archives"][1]["size"], 1234);
    assert_eq!(rows.rows["inputs"][0]["id"], "a");
    assert_eq!(rows.rows["inputs"][1]["id"], "z");
    for row in &rows.rows["diagnostics"] {
        assert_eq!(row["name"].as_str().unwrap().len(), 3072);
        assert_eq!(row["cut"], true);
    }
    for row in &rows.rows["skips"] {
        assert_eq!(row["serialized_file"].as_str().unwrap().len(), 3072);
    }
    assert_eq!(rows.rows["archive_members"].len(), 6);
    assert!(
        rows.rows["archive_members"]
            .iter()
            .filter(|r| r["key"] == "z")
            .all(|r| r["included"] == true)
    );
}
#[test]
fn each_unacknowledged_family_zeros_its_own_counts_and_sync_zeros_all() {
    let scene = Scene::new(&["z", "a", "m"], true, 4096);
    for family in Family::ALL {
        let (mut report, _) = assemble(&scene, Some(family));
        assert_eq!(report.rows[family as usize], None);
        assert!(report.publication_failed);
        assert_eq!(report.run.undeclared_dependencies, 2);
        let mut expected = manifest::counts::Counts {
            bundles: 2,
            inputs: 2,
            inputs_with_skips: 2,
            outputs_written: 1,
            outputs_present: 1,
            outputs_failed: 1,
            skips: 2,
            skips_expected: 2,
            collisions: 1,
            archives_written: 1,
            archives_present: 1,
            ..manifest::counts::Counts::default()
        };
        match family {
            Family::Bundles => expected.bundles = 0,
            Family::Inputs => {
                expected.inputs = 0;
                expected.inputs_with_skips = 0;
            }
            Family::Skips => {
                expected.skips = 0;
                expected.skips_expected = 0;
            }
            Family::Outputs => {
                expected.outputs_written = 0;
                expected.outputs_present = 0;
                expected.outputs_failed = 0;
            }
            Family::Collisions => expected.collisions = 0,
            Family::Archives => {
                expected.archives_written = 0;
                expected.archives_present = 0;
            }
            Family::Diagnostics | Family::ArchiveMembers => {}
        }
        assert_eq!(report.counts, expected, "{family:?}");
        let run = report.run;
        report.sync_failed();
        assert_eq!(report.run, run);
        assert_eq!(report.rows, [None; 8]);
        assert_eq!(report.counts, manifest::counts::Counts::default());
    }
}
#[test]
fn determinism_across_block_sizes_and_preexisting_output_results() {
    use std::sync::Arc;
    use unityrs_batch::{
        publish::{self, Context, payload::Committed},
        sink::directory::Directory,
    };
    let mut expected = BTreeMap::new();
    for block in [512, 3072, 4096, 5003, 65536] {
        let scene = Scene::new(&["z", "a", "m"], true, block);
        for (mode, preexisting, resume) in [(0, false, false), (1, true, false), (2, true, true)] {
            let root = scene.scratch.join(format!("sink-{mode}"));
            std::fs::create_dir(&root).unwrap();
            let directory = Directory::preflight(
                &root,
                "r",
                1,
                Arc::new(publication_support::Portable),
                &mut vec![0; 1_048_577],
            )
            .unwrap();
            if preexisting {
                std::fs::write(root.join("a"), b"payload").unwrap();
                std::fs::write(root.join("m"), b"payloae").unwrap();
            }
            let out = scene.outcomes(&format!("determinism-{mode}"), &Host);
            let gate = Gate::default();
            let account = Account::new(64 << 20);
            let result = publish::drive(&Context {
                handoff: &scene.handoff,
                workspace: &scene.workspace,
                payloads: &Committed {
                    done: &scene.done,
                    states: &scene.prepared.inputs,
                    fs: &Host,
                },
                directory: &directory,
                outcomes: &out,
                gate: &gate,
                account: &account,
                run_id: "r",
                resume,
                stopped: &|| false,
                hooks: &publish::NoHooks,
            })
            .unwrap();
            assert_eq!(
                result,
                publish::Admission::Drained(publish::Report {
                    written: if resume { 2 } else { 3 },
                    present: u64::from(resume),
                    failed: 0,
                })
            );
            for key in ["a", "m", "z"] {
                assert_eq!(std::fs::read(root.join(key)).unwrap(), b"payload");
            }
            let (report, rows) = assemble_outcomes(&scene, None, &out);
            assert_eq!(
                rows.rows["outputs"][0]["status"],
                if resume { "present" } else { "written" }
            );
            if let Some((counts, previous)) = expected.get(&mode) {
                assert_eq!(&report.counts, counts);
                assert_eq!(&rows.rows, previous);
            } else {
                expected.insert(mode, (report.counts, rows.rows));
            }
        }
    }
}

#[test]
fn retained_handoff_and_outcomes_share_the_six_s_plus_two_a_ledger() {
    use unityrs_batch::{
        records::{Text, text::escaped},
        work::rounded,
    };
    for block in [512_u64, 3072, 4096, 5003, 65536] {
        for keys in [&[][..], &["a"][..], &["z", "a", "m"][..]] {
            let scene = Scene::new(keys, true, block);
            let baseline: u64 = scene
                .prepared
                .inputs
                .iter()
                .map(|s| s.committed.unwrap().claimed_bytes)
                .sum();
            let k = if block < 4096 && 4096 % block != 0 {
                2
            } else {
                block.div_ceil(4096).max(1)
            };
            let size = scene.spooled * k;
            let mut archives = 0;
            for archive in &scene.prepared.spec.archives {
                let mut bytes = 0_u64;
                escaped(Text::Prepared(&archive.key, false), |part| {
                    bytes += part.len() as u64;
                    Ok::<_, ()>(())
                })
                .unwrap();
                assert_eq!(
                    bytes,
                    serde_json::to_string(&archive.key).unwrap().len() as u64 - 2
                );
                archives += rounded(bytes + 512, block).unwrap();
            }
            let _outcomes = scene.outcomes("ledger", &Host);
            let additional = scene.counter.peak() - baseline;
            assert!(
                additional <= 6 * size + 2 * archives,
                "block={block} peak={additional} S={size} A={archives}"
            );
        }
    }
}
#[test]
fn frozen_not_run_rows_distinguish_queued_from_dispatched() {
    use unityrs_batch::phase1::{InputPhase, JobKind};
    let mut scene = Scene::new(&[], false, 4096);
    for state in &mut scene.prepared.inputs {
        state.received = None;
        state.committed = None;
    }
    scene.prepared.inputs[0].phase = InputPhase::Ready;
    scene.prepared.inputs[1].phase = InputPhase::Running;
    // Input 1 was dispatched: its bit is set until a commit clears it.
    scene.prepared.uncommitted.set(1, true).unwrap();
    let taken_at = std::time::Instant::now();
    scene.prepared.slots[0] = Some((
        JobKind::Attempt,
        1,
        taken_at
            .checked_sub(std::time::Duration::from_millis(1234))
            .unwrap(),
    ));
    let out = scene.outcomes("snapshot", &Host);
    let gate = Gate::default();
    let frozen = gate.freeze(&out).unwrap();
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let inputs = winners::committed::Committed::received(
        &scene.done,
        &scene.prepared.inputs,
        &Host,
        &|| false,
    )
    .unwrap();
    assert!(winners::Inputs::open(&inputs, 0).is_err());
    assert!(winners::Inputs::failed(&inputs, 1).is_err());
    let mut rows = Rows::default();
    manifest::assemble(
        &manifest::rows::Context {
            facts: &scene.prepared,
            inputs: &inputs,
            selected: Some(manifest::rows::Selected {
                handoff: &scene.handoff,
                workspace: &scene.workspace,
                outcomes: frozen.outcomes,
            }),
            stop: None,
            found: None,
            account: &account,
            stopped: &|| false,
            taken_at,
        },
        &mut rows,
    )
    .unwrap();
    let dispatched = &rows.rows["inputs"][0];
    let queued = &rows.rows["inputs"][1];
    assert_eq!(dispatched["id"], "a");
    assert_eq!(dispatched["reason"], "stopped");
    assert_eq!(dispatched["ms"], 1234);
    assert_eq!(queued["id"], "z");
    assert_eq!(queued["reason"], serde_json::Value::Null);
    assert_eq!(queued["ms"], serde_json::Value::Null);
}
#[test]
fn finisher_storage_is_independent_of_retained_phase_reservations() {
    let scene = Scene::new(&["z", "a", "m"], true, 4096);
    let phase = Account::new(64 << 20);
    let retained = unityrs_batch::phase::Buffer::<u8>::new(phase.meter(), 48 << 20).unwrap();
    let (report, _) = assemble(&scene, None);
    assert_eq!(report.counts.outputs_written, 1);
    assert!(phase.snapshot().input.used > 0);
    drop(retained);
    assert_eq!(phase.snapshot().input.used, 0);
}
#[test]
fn predispatched_failure_record_flush_is_included_in_run_work() {
    use std::sync::Arc;
    use unityrs_batch::{
        records::{self, Reason, Status, Summary},
        spool::Directory,
    };
    let mut scene = Scene::new(&[], false, 4096);
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let mut failure = Directory::create(
        scene.scratch.join("failure"),
        scene.counter.clone(),
        Arc::new(Host),
    )
    .unwrap();
    let summary = records::write_failure_record(
        &failure,
        account.meter(),
        Summary {
            input: "a",
            status: Status::Failed,
            reason: Some(Reason::Text("source_missing".into())),
            detail: None,
            figures: None,
            spooled_part: 0,
            claimed_bytes: 0,
            cut: false,
        },
    )
    .unwrap();
    std::fs::rename(scene.done.join("1"), scene.scratch.join("saved")).unwrap();
    failure.rename(scene.done.join("1")).unwrap();
    scene.prepared.inputs[1].received = None;
    scene.prepared.inputs[1].failure_bundle = Some(1);
    scene.prepared.inputs[1].committed = Some(failure.into_claim().unwrap());
    scene.handoff.publication_blocked = true;
    let expected = scene.prepared.inputs[0].received.unwrap().spooled_part + summary.spooled_part;
    let out = scene.outcomes("failure-out", &Host);
    let gate = Gate::default();
    let frozen = gate.freeze(&out).unwrap();
    let inputs = winners::committed::Committed::received(
        &scene.done,
        &scene.prepared.inputs,
        &Host,
        &|| false,
    )
    .unwrap();
    let report = manifest::assemble(
        &manifest::rows::Context {
            facts: &scene.prepared,
            inputs: &inputs,
            selected: Some(manifest::rows::Selected {
                handoff: &scene.handoff,
                workspace: &scene.workspace,
                outcomes: frozen.outcomes,
            }),
            stop: None,
            found: None,
            account: &account,
            stopped: &|| false,
            taken_at: std::time::Instant::now(),
        },
        &mut Rows::default(),
    )
    .unwrap();
    assert_eq!(report.run.spooled_parts, expected);
    assert_eq!(report.counts.inputs_failed, 1);
}

fn render_unwritten(
    scene: &Scene,
) -> Result<(manifest::Report, Rows), unityrs_batch::phase::Error> {
    let out = scene.outcomes("unwritten", &Host);
    let gate = Gate::default();
    let frozen = gate.freeze(&out)?;
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let inputs = winners::committed::Committed::received(
        &scene.done,
        &scene.prepared.inputs,
        &Host,
        &|| false,
    )?;
    let mut rows = Rows::default();
    let report = manifest::assemble(
        &manifest::rows::Context {
            facts: &scene.prepared,
            inputs: &inputs,
            selected: Some(manifest::rows::Selected {
                handoff: &scene.handoff,
                workspace: &scene.workspace,
                outcomes: frozen.outcomes,
            }),
            stop: None,
            found: None,
            account: &account,
            stopped: &|| false,
            taken_at: std::time::Instant::now(),
        },
        &mut rows,
    )?;
    Ok((report, rows))
}
#[test]
fn unsigned_format_max_is_an_unexpected_skip() {
    let scene = Scene::new(&[], false, 4096);
    for index in 0..2 {
        let path = scene.done.join(format!("{index}/records.ndjson"));
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("empty_image", &format!("unsupported_format({})", u64::MAX));
        std::fs::write(path, text).unwrap();
    }
    let (report, _) = render_unwritten(&scene).unwrap();
    assert_eq!(report.counts.skips, 2);
    assert_eq!(report.counts.skips_expected, 0);
}
fn rewrite_outputs(scene: &mut Scene, change: impl FnOnce(&mut Vec<serde_json::Value>)) {
    use std::io::Read;
    let old = scene.handoff.outputs.take().unwrap();
    let mut bytes = String::new();
    scene
        .workspace
        .open(&old, &|| false)
        .unwrap()
        .read_to_string(&mut bytes)
        .unwrap();
    let mut rows = bytes
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    change(&mut rows);
    let mut file = scene.workspace.create_file(&|| false).unwrap();
    for row in rows {
        let mut bytes = serde_json::to_vec(&row).unwrap();
        bytes.push(b'\n');
        file.append(&bytes, &|| false).unwrap();
    }
    scene.handoff.outputs = Some(file.seal(&|| false).unwrap());
    scene.workspace.retire(old, &|| false).unwrap();
}
#[test]
fn malformed_output_identity_is_internal() {
    for (field, value) in [
        ("input", serde_json::json!("z")),
        ("md5", serde_json::json!("00000000000000000000000000000000")),
        ("size", serde_json::json!(8)),
        ("serialized_file", serde_json::json!("OTHER")),
        ("path_id", serde_json::json!(8)),
        ("unit", serde_json::json!("other")),
        ("cut", serde_json::json!(true)),
        ("picture", serde_json::Value::Null),
    ] {
        let mut scene = Scene::new(&["a"], false, 4096);
        rewrite_outputs(&mut scene, |rows| {
            if field == "picture" {
                rows[0]["kind"] = serde_json::json!("texture");
                rows[0]["mode"] = serde_json::json!("RGBA");
                rows[0]["visible_hash"] = serde_json::json!("00".repeat(32));
            } else {
                rows[0][field] = value;
            }
        });
        assert!(
            matches!(render_unwritten(&scene), Err(e) if e == unityrs_batch::phase::Error::internal()),
            "{field}"
        );
    }
}
#[test]
fn malformed_output_membership_and_missing_refusal_are_internal() {
    for defect in [
        "refused_winner",
        "orphan_publishable",
        "missing_refusal",
        "wrong_count",
    ] {
        let mut scene = Scene::new(&["a", "a/b", "z"], false, 4096);
        match defect {
            "refused_winner" => rewrite_outputs(&mut scene, |rows| {
                let row = rows.iter_mut().find(|r| r["key"] == "z").unwrap();
                row["status"] = serde_json::json!("withheld");
                row["reason"] = serde_json::json!("unit_lost");
            }),
            "orphan_publishable" => rewrite_outputs(&mut scene, |rows| {
                rows[0]["status"] = serde_json::json!("not_written");
                rows[0]["reason"] = serde_json::Value::Null;
            }),
            "missing_refusal" => rewrite_outputs(&mut scene, |rows| {
                rows.remove(0);
            }),
            _ => scene.handoff.counts.keys += 1,
        }
        assert!(
            matches!(render_unwritten(&scene), Err(e) if e == unityrs_batch::phase::Error::internal()),
            "{defect}"
        );
    }
}

#[test]
fn globally_withheld_base_rows_still_reconcile_with_winners() {
    let scene = Scene::with_failure(&["a"], false, 4096, true);
    assert!(scene.handoff.publication_blocked);
    assert_eq!(scene.handoff.counts.winners, 1);
    let (report, rows) = render_unwritten(&scene).unwrap();
    assert_eq!(report.counts.outputs_withheld, 1);
    assert_eq!(rows.rows["outputs"][0]["status"], "withheld");
    assert_eq!(rows.rows["outputs"][0]["reason"], "publish_incomplete");
}

#[test]
fn fixed_download_details_render_in_bundle_rows_without_peer_values() {
    use unityrs_batch::{
        http::Detail,
        stage::{DataFailure, Failure, StageEnd},
    };
    for (failure, reason, detail) in [
        (
            DataFailure::DownloadFailed(Detail::Connect),
            "download_failed",
            "connect",
        ),
        (
            DataFailure::DownloadFailed(Detail::Status(503)),
            "download_failed",
            "status 503",
        ),
        (
            DataFailure::RedirectRefused(307),
            "redirect_refused",
            "status 307",
        ),
    ] {
        let mut scene = Scene::new(&["z", "a", "m"], true, 4096);
        scene.prepared.bundles[0].stage = Some(StageEnd {
            bundle: 0,
            failure: Some(Failure::Data(failure)),
            integrity: None,
            facts: None,
            claimed_bytes: 0,
        });
        let (_, rows) = assemble(&scene, None);
        let row = rows.rows["bundles"]
            .iter()
            .find(|r| r["reason"] == reason)
            .unwrap();
        assert_eq!(row["detail"], detail);
        assert!(row["size"].is_null());
        assert!(row["sha256"].is_null());
    }
}
