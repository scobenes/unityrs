mod common;
mod publication_support;

use publication_support::{Portable, Scene};
use std::sync::Arc;
use unityrs_batch::{
    finish, manifest, outcomes, phase::Account, publish::Gate, reason::Code,
    sink::directory::Directory, spool::Host, winners,
};

fn finish_scene(scene: &Scene, archive_codes: [outcomes::Code; 2]) -> serde_json::Value {
    let sink = scene.scratch.join("sink");
    std::fs::create_dir(&sink).unwrap();
    let directory = Directory::preflight(
        &sink,
        "r",
        1,
        Arc::new(Portable),
        &mut vec![0; manifest::head::HEAD_LIMIT + 1],
    )
    .unwrap();
    let out = scene.outcomes("out", &Host);
    for index in 0..scene.handoff.counts.winners {
        out.set(index, outcomes::Code::Written).unwrap();
    }
    for (index, code) in archive_codes.into_iter().enumerate() {
        let value = match code {
            outcomes::Code::NotWritten => continue,
            outcomes::Code::Written | outcomes::Code::Present => {
                outcomes::Archive::success(code, 10, [0; 16], None).unwrap()
            }
            _ => outcomes::Archive::failed(code).unwrap(),
        };
        out.set_archive(u64::try_from(index).unwrap(), &value)
            .unwrap();
    }
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
    let cx = manifest::rows::Context {
        prepared: &scene.prepared,
        inputs: &inputs,
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        frozen: &frozen,
        account: &account,
        stopped: &|| false,
        taken_at: std::time::Instant::now(),
    };
    let identity = finish::Identity {
        started_at: "2026-01-01T00:00:00Z",
        memory_bound: 0,
        spool_peak: 0,
        vm_hwm: 0,
    };
    finish::running(&scene.prepared, &directory, &account, &identity).unwrap();
    let result = finish::normal(&cx, &directory, &identity, "2026-01-01T00:00:01Z").unwrap();
    let head: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sink.join("_unityrs/r/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(head["exit_code"], result.code.value());
    assert_eq!(result.reason, None);
    assert_eq!(account.snapshot().input.used, 0);
    head
}

#[test]
fn archive_publication_failures_set_four_when_all_loose_outputs_succeeded() {
    for code in [
        outcomes::Code::NotWritten,
        outcomes::Code::PublishFailed,
        outcomes::Code::PublishMismatch,
        outcomes::Code::CredentialsExpired,
        outcomes::Code::ArchiveTooLarge,
    ] {
        let scene = Scene::new(&["key"], true, 4096);
        let head = finish_scene(&scene, [code, outcomes::Code::Written]);
        assert_eq!(head["exit_code"], 4, "{code:?}");
        assert_eq!(head["counts"]["outputs_written"], 1);
        assert_eq!(head["counts"]["archives_written"], 1);
    }
}

#[test]
fn empty_archives_have_no_publication_failure() {
    let scene = Scene::new(&[], true, 4096);
    let head = finish_scene(&scene, [outcomes::Code::NotWritten; 2]);
    assert_eq!(head["exit_code"], 0);
}

#[test]
fn written_and_present_archives_do_not_raise_the_code() {
    let scene = Scene::new(&["key"], true, 4096);
    let head = finish_scene(&scene, [outcomes::Code::Written, outcomes::Code::Present]);
    assert_eq!(head["exit_code"], 0);
    assert_eq!(head["counts"]["archives_present"], 1);
}

#[test]
fn member_refusals_set_three_even_when_every_transfer_succeeded() {
    for reason in [
        "duplicate_member",
        "prefix_conflict",
        "unwritable_path(dot_segment)",
    ] {
        let mut scene = Scene::new(&["key"], true, 4096);
        // A synthetic phase-6 public row isolates member refusal reduction
        // from loose key refusals and transfer outcomes.
        let row = serde_json::json!({
            "archive":"a.zip", "path":"key", "key":"key", "input":"a",
            "md5":"00000000000000000000000000000000", "included":false,
            "reason":reason, "cut":false
        });
        let mut bytes = serde_json::to_vec(&row).unwrap();
        bytes.push(b'\n');
        let mut file = scene.workspace.create_file(&|| false).unwrap();
        file.append(&bytes, &|| false).unwrap();
        scene.handoff.archive_members = Some(file.seal(&|| false).unwrap());
        let head = finish_scene(&scene, [outcomes::Code::Written; 2]);
        assert_eq!(
            head["exit_code"],
            Code::CompleteWithSkips.value(),
            "{reason}"
        );
        assert_eq!(head["counts"]["archives_written"], 2);
    }
}
