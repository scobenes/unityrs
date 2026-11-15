//! Whole runs through the library entry, with an independently signed fake peer.
mod common;
mod controller_support;
#[path = "store_run_support/faults.rs"]
mod faults;
#[cfg(feature = "test-hooks")]
#[path = "store_run_support/finisher.rs"]
mod finisher;
#[path = "store_run_support/large.rs"]
mod large;
mod publication_support;
mod stage_support;
mod store_support;
mod url_run_support;
use serde_json::{Value, json};
use std::{fs, sync::Arc};
use store_support::fake::Fake;
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    reason::Code,
    run,
};
use unityrs_testkit::scratch::Scratch;

struct Environment;
impl unityrs_batch::stage::env::Environment for Environment {
    fn var(&self, name: &str) -> Option<std::ffi::OsString> {
        match name {
            "AWS_ACCESS_KEY_ID" => Some("AKIDEXAMPLE".into()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret".into()),
            _ => None,
        }
    }
}
struct Higher(Arc<Fake>, Vec<u8>);
impl run::Observer for Higher {
    fn seam(&self, seam: run::Seam) {
        if seam == run::Seam::Verified {
            self.0.seed("bucket", "p/_unityrs/r/manifest.json", &self.1);
        }
    }
}
struct Observe;
impl run::Observer for Observe {
    fn handed_over(&self, h: &unityrs_batch::finish::HandOver) {
        if let Some(unityrs_batch::finish::Selected::Owned(owned)) = &h.selected {
            let snapshot = owned.account.snapshot();
            assert!(snapshot.input.peak <= owned.account.limit());
            if h.cause == unityrs_batch::finish::FinishCause::Normal {
                let mut failures = 0;
                for i in 0..owned.handoff.counts.winners {
                    failures += u64::from(owned.outcomes.loose(i).unwrap().reason().is_some());
                }
                assert_eq!(
                    owned
                        .progress
                        .as_ref()
                        .unwrap()
                        .snapshot()
                        .unwrap()
                        .outputs
                        .failed,
                    failures
                );
            }
        }
    }
    fn finished(&self, f: &unityrs_batch::finish::Finished) {
        assert!(f.settled.closed, "the finisher closes the store journal");
    }
}
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
fn make_spec(scratch: &Scratch, fake: &Fake) -> Value {
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::write(scratch.join("input"), controller_support::clear(1)).unwrap();
    let mut spec = common::base();
    spec["run_id"] = "r".into();
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"] = json!({"kind":"object_store", "bucket":"bucket", "region":"us-east-1", "prefix":"p/", "endpoint":fake.endpoint.as_str(), "path_style":true});
    spec["deadline"] = json!("2099-01-01T00:00:00Z");
    spec["inputs"] = json!([{"id":"a", "path":scratch.join("input")}]);
    spec["dependencies"] = json!([]);
    spec
}
fn drive(
    spec: &Value,
    scratch: &std::path::Path,
) -> Result<unityrs_batch::finish::ResultEnd, run::Error> {
    drive_options(
        spec,
        scratch,
        run::Options {
            observer: Arc::new(Observe),
            ..Default::default()
        },
    )
}
fn drive_options(
    spec: &Value,
    scratch: &std::path::Path,
    mut options: run::Options,
) -> Result<unityrs_batch::finish::ResultEnd, run::Error> {
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let mut network = options.network.take().unwrap_or_else(run::Network::system);
    network.environment = Arc::new(Environment);
    options.network = Some(network);
    run::drive_with(
        prepared,
        123,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &options,
    )
}
#[test]
fn x7_empty_run_writes_heads_and_rows_on_the_finisher_runtime() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-run-empty");
        let spec = make_spec(&scratch, &fake);
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(drive(&spec, &scratch));
        });
        let result = receive.await.unwrap().unwrap();
        assert_eq!(result.code, Code::Complete);
        let head: Value = serde_json::from_slice(
            &fake
                .object("bucket", "p/_unityrs/r/manifest.json")
                .unwrap()
                .bytes,
        )
        .unwrap();
        assert_eq!(head["write_seq"], 1);
        assert_eq!(head["verified"], Value::Null);
        assert_eq!(head["unsettled_tries"], 0);
        assert_eq!(head["rows"]["inputs"], 1);
        assert!(
            fake.object("bucket", "p/_unityrs/r/1/inputs.0.ndjson")
                .is_some()
        );
        assert!(fake.logs().iter().all(|log| log.signature_valid));
        assert_eq!(fake.open_uploads(), 0);
    });
}

#[test]
fn x7_loose_and_small_archive_are_put_verified_and_resumed() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-run-publication");
        let mut spec = make_spec(&scratch, &fake);
        let file = unityrs_testkit::ngui::file(
            &[],
            &[unityrs_testkit::ngui::texture(
                1,
                b"pixel",
                4,
                (1, 1),
                vec![1, 2, 3, 255],
            )],
            true,
        )
        .unwrap();
        fs::write(
            scratch.join("input"),
            unityrs_testkit::env::bundle(&[("CAB-synthetic", file)]),
        )
        .unwrap();
        spec["archives"] =
            json!([{"key":"all.zip","members":[{"kind":"texture","path":"{name}.png"}]}]);
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let first = drive(&spec, &scratch).unwrap();
            assert_eq!(first.code, Code::Complete);
            spec["attempt"] = 2.into();
            spec["resume"] = true.into();
            let second = drive(&spec, &scratch).unwrap();
            assert_eq!(second.code, Code::Complete);
            send.send(()).unwrap();
        });
        receive.await.unwrap();
        let head: Value = serde_json::from_slice(
            &fake
                .object("bucket", "p/_unityrs/r/manifest.json")
                .unwrap()
                .bytes,
        )
        .unwrap();
        assert_eq!(head["verified"], true);
        assert_eq!(head["counts"]["outputs_present"], 1);
        assert_eq!(head["counts"]["archives_present"], 1);
        let archive = fake.object("bucket", "p/all.zip").unwrap();
        assert_eq!(archive.content_type.as_deref(), Some("application/zip"));
        assert!(archive.bytes.starts_with(b"PK"));
        for key in ["p/a/pixel.png", "p/all.zip"] {
            assert_eq!(
                fake.logs()
                    .iter()
                    .filter(|l| l.operation == store_support::fake::Operation::Put
                        && l.raw_path == format!("/bucket/{key}"))
                    .count(),
                1
            );
        }
        assert_eq!(fake.open_uploads(), 0);
    });
}

#[test]
fn x8_stop_during_running_write_keeps_its_precondition_for_the_finisher() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-running-stop");
        let spec = make_spec(&scratch, &fake);
        fake.schedule(
            store_support::fake::Operation::Put,
            "p/_unityrs/r/manifest.json",
            1,
            store_support::fake::Fault {
                before_head: Some(unityrs_testkit::guard::DEADLOCK_GUARD),
                ..Default::default()
            },
        );
        let lifetime = Arc::new(run::Lifetime::default());
        let control = lifetime.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(drive_options(
                &spec,
                &scratch,
                run::Options {
                    lifetime: control,
                    ..Default::default()
                },
            ));
        });
        while !fake
            .logs()
            .iter()
            .any(|l| l.operation == store_support::fake::Operation::Put && l.body_len > 0)
        {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        lifetime.request(unityrs_batch::lifetime::Stop::external(
            unityrs_batch::reason::StopReason::Sigterm,
        ));
        let result = receive.await.unwrap().unwrap();
        assert_eq!(result.code, Code::Stopped);
        let head: Value = serde_json::from_slice(
            &fake
                .object("bucket", "p/_unityrs/r/manifest.json")
                .unwrap()
                .bytes,
        )
        .unwrap();
        assert_eq!(head["write_seq"], 1);
        assert_eq!(head["stop_reason"], "sigterm");
        assert_eq!(head["rows"]["outputs"], Value::Null);
    });
}
fn pixel(scratch: &Scratch) {
    let file = unityrs_testkit::ngui::file(
        &[],
        &[unityrs_testkit::ngui::texture(
            1,
            b"pixel",
            4,
            (1, 1),
            vec![1, 2, 3, 255],
        )],
        true,
    )
    .unwrap();
    fs::write(
        scratch.join("input"),
        unityrs_testkit::env::bundle(&[("CAB-synthetic", file)]),
    )
    .unwrap();
}
async fn execute(spec: Value, scratch: Scratch) -> (unityrs_batch::finish::ResultEnd, Scratch) {
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = drive(&spec, &scratch).unwrap();
        let _ = send.send((result, scratch));
    });
    receive.await.unwrap()
}
fn head(fake: &Fake) -> Value {
    serde_json::from_slice(
        &fake
            .object("bucket", "p/_unityrs/r/manifest.json")
            .unwrap()
            .bytes,
    )
    .unwrap()
}
#[test]
fn x7_opaque_write_tags_are_kept_and_verified_for_loose_and_archive_outputs() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-opaque");
        let mut spec = make_spec(&scratch, &fake);
        pixel(&scratch);
        spec["archives"] =
            json!([{"key":"all.zip","members":[{"kind":"texture","path":"{name}.png"}]}]);
        for key in ["p/a/pixel.png", "p/all.zip"] {
            fake.schedule(
                store_support::fake::Operation::Put,
                key,
                1,
                store_support::fake::Fault {
                    etag: Some("\"opaque-V1\"".into()),
                    ..Default::default()
                },
            );
        }
        let (result, scratch) = execute(spec.clone(), scratch).await;
        assert_eq!(result.code, Code::Complete);
        let head = head(&fake);
        assert_eq!(head["verified"], true);
        assert_eq!(head["verified_by_size"], 2);
        assert_eq!(head["warnings"]["etag_not_md5"], 2);
        assert_eq!(
            fs::read(scratch.join("spool/runs/r/outcomes")).unwrap(),
            [7]
        );
        let row = fake
            .object("bucket", "p/_unityrs/r/1/archives.0.ndjson")
            .unwrap();
        let row: Value = serde_json::from_slice(&row.bytes).unwrap();
        assert_eq!(row["etag"], "opaque-V1");
        spec["attempt"] = 2.into();
        spec["resume"] = true.into();
        let (result, _) = execute(spec, scratch).await;
        assert_eq!(result.code, Code::Complete);
        assert_eq!(
            fake.logs()
                .iter()
                .filter(|l| l.operation == store_support::fake::Operation::Put
                    && l.raw_path == "/bucket/p/a/pixel.png")
                .count(),
            2
        );
    });
}
#[test]
fn x7_failed_resume_and_verification_lists_degrade_with_their_own_warnings() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-list-failure");
        let mut spec = make_spec(&scratch, &fake);
        pixel(&scratch);
        spec["resume"] = true.into();
        for ordinal in [2, 3] {
            fake.schedule_list(
                "p/",
                None,
                ordinal,
                store_support::fake::Fault::error(403, "AccessDenied"),
            );
        }
        let (result, _) = execute(spec, scratch).await;
        assert_eq!(result.code, Code::Complete);
        let head = head(&fake);
        assert_eq!(head["verified"], false);
        assert_eq!(head["warnings"]["resume_list_failed"], 1);
        assert_eq!(head["warnings"]["verify_list_failed"], 1);
        assert_eq!(head["counts"]["outputs_written"], 1);
    });
}
#[test]
fn x7_a_write_that_is_not_visible_at_verification_is_publish_mismatch() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-late-put");
        let spec = make_spec(&scratch, &fake);
        pixel(&scratch);
        fake.schedule(
            store_support::fake::Operation::Put,
            "p/a/pixel.png",
            1,
            store_support::fake::Fault {
                commit_delay: Some(unityrs_testkit::guard::DEADLOCK_GUARD),
                ..Default::default()
            },
        );
        let (result, _) = execute(spec, scratch).await;
        assert_eq!(result.code, Code::Incomplete);
        let head = head(&fake);
        assert_eq!(head["verified"], false);
        assert_eq!(head["warnings"]["verify_list_failed"], 0);
        assert_eq!(head["counts"]["outputs_failed"], 1);
        let row = fake
            .object("bucket", "p/_unityrs/r/1/outputs.0.ndjson")
            .unwrap();
        let row: Value = serde_json::from_slice(&row.bytes).unwrap();
        assert_eq!(row["reason"], "publish_mismatch");
    });
}
#[test]
fn x8_preflight_supersession_writes_nothing_and_final_supersession_keeps_the_newer_head() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-superseded");
        let spec = make_spec(&scratch, &fake);
        let newer = json!({"manifest_version":1,"run_id":"r","attempt":2,"write_seq":0,"binary_version":"x","spec_version":1,"spec_sha256":"00".repeat(32)});
        fake.seed(
            "bucket",
            "p/_unityrs/r/manifest.json",
            &serde_json::to_vec(&newer).unwrap(),
        );
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            send.send(drive(&spec, &scratch)).unwrap();
        });
        let result = receive.await.unwrap().unwrap_err();
        assert_eq!(result.code, Code::Superseded);
        assert!(
            fake.logs()
                .iter()
                .all(|l| l.operation != store_support::fake::Operation::Put)
        );
        let fake = Arc::new(Fake::start().await);
        let scratch = Scratch::new("store-final-superseded");
        let spec = make_spec(&scratch, &fake);
        let observer = Arc::new(Higher(fake.clone(), serde_json::to_vec(&newer).unwrap()));
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            send.send(drive_options(
                &spec,
                &scratch,
                run::Options {
                    observer,
                    ..Default::default()
                },
            ))
            .unwrap();
        });
        let result = receive.await.unwrap().unwrap();
        assert_eq!(result.code, Code::Superseded);
        assert_eq!(result.status, None);
        assert_eq!(head(&fake), newer);
    });
}

struct ChangeTag(Arc<Fake>);
impl unityrs_batch::publish::Hooks for ChangeTag {
    fn point(&self, point: unityrs_batch::publish::Point) {
        if point == unityrs_batch::publish::Point::Drained {
            let object = self.0.object("bucket", "p/a/pixel.png").unwrap();
            self.0.seed("bucket", "p/a/pixel.png", &object.bytes);
        }
    }
}
#[test]
fn x7_verification_requires_the_returned_opaque_tag_even_at_the_same_size() {
    store_support::run(async {
        let fake = Arc::new(Fake::start().await);
        let scratch = Scratch::new("store-changed-tag");
        let spec = make_spec(&scratch, &fake);
        pixel(&scratch);
        fake.schedule(
            store_support::fake::Operation::Put,
            "p/a/pixel.png",
            1,
            store_support::fake::Fault {
                etag: Some("opaque".into()),
                ..Default::default()
            },
        );
        let hook = Arc::new(ChangeTag(fake.clone()));
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(drive_options(
                &spec,
                &scratch,
                run::Options {
                    publish_hooks: hook,
                    ..Default::default()
                },
            ));
        });
        assert_eq!(receive.await.unwrap().unwrap().code, Code::Incomplete);
        let head = head(&fake);
        assert_eq!(head["verified"], false);
        assert_eq!(head["verified_by_size"], 0);
        assert_eq!(head["warnings"]["etag_not_md5"], 1);
        assert_eq!(head["counts"]["outputs_failed"], 1);
    });
}
struct StopAtDrain(Arc<run::Lifetime>);
impl unityrs_batch::publish::Hooks for StopAtDrain {
    fn point(&self, point: unityrs_batch::publish::Point) {
        if point == unityrs_batch::publish::Point::Drained {
            self.0.request(unityrs_batch::lifetime::Stop::external(
                unityrs_batch::reason::StopReason::Sigterm,
            ));
        }
    }
}
#[test]
fn x7_a_stop_after_drain_keeps_written_outcomes_and_marks_verification_false() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-stop-after-drain");
        let spec = make_spec(&scratch, &fake);
        pixel(&scratch);
        let lifetime = Arc::new(run::Lifetime::default());
        let hook = Arc::new(StopAtDrain(lifetime.clone()));
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(drive_options(
                &spec,
                &scratch,
                run::Options {
                    lifetime,
                    publish_hooks: hook,
                    ..Default::default()
                },
            ));
        });
        assert_eq!(receive.await.unwrap().unwrap().code, Code::Stopped);
        let head = head(&fake);
        assert_eq!(head["verified"], false);
        assert_eq!(head["warnings"]["verify_list_failed"], 0);
        assert_eq!(head["counts"]["outputs_written"], 1);
        assert_eq!(head["stop_reason"], "sigterm");
    });
}

#[cfg(target_os = "linux")]
#[test]
fn x10_a_stop_armed_inside_a_finisher_row_preserves_the_live_head_reserve() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-live-finisher-budget");
        let mut spec = make_spec(&scratch, &fake);
        spec["sink"]["timeout_ms"] = 1000.into();
        spec["http"] = json!({"backoff_initial_ms":0,"backoff_max_ms":0});
        fake.schedule(
            store_support::fake::Operation::Put,
            "p/_unityrs/r/1/bundles.0.ndjson",
            1,
            store_support::fake::Fault {
                before_head: Some(unityrs_testkit::guard::DEADLOCK_GUARD),
                ..Default::default()
            },
        );
        let lifetime = Arc::new(run::Lifetime::default());
        let control = lifetime.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(drive_options(
                &spec,
                &scratch,
                run::Options {
                    lifetime: control,
                    ..Default::default()
                },
            ));
        });
        while !fake
            .logs()
            .iter()
            .any(|l| l.raw_path == "/bucket/p/_unityrs/r/1/bundles.0.ndjson" && l.body_len > 0)
        {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        let now = unityrs_batch::lifetime::Boot::now().unwrap();
        lifetime
            .state()
            .arm(unityrs_batch::lifetime::Boot(now.0 + 3100));
        lifetime.request(unityrs_batch::lifetime::Stop::external(
            unityrs_batch::reason::StopReason::Sigterm,
        ));
        let result = receive.await.unwrap().unwrap();
        assert_eq!(result.code, Code::Incomplete);
        assert_eq!(head(&fake)["write_seq"], 1);
        assert_eq!(head(&fake)["stop_reason"], Value::Null);
        assert_eq!(
            fake.logs()
                .iter()
                .filter(|l| l.raw_path == "/bucket/p/_unityrs/r/1/bundles.0.ndjson")
                .count(),
            1
        );
    });
}
