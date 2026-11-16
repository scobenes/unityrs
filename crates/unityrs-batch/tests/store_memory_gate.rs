//! X12 store processes: real signed requests to deterministic local peers,
//! coherent held charges and frozen owners through process exit (§5.1–5.3).
#![cfg(all(target_os = "linux", feature = "test-hooks"))]
mod common;
mod memory_support;
mod store_support;
#[path = "http_support/tls.rs"]
#[allow(dead_code)]
mod tls;
mod url_run_support;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::Instant,
};
use store_support::fake::{Fake, Fault, Operation};
use unityrs_batch::{
    alloc::{self, Counting},
    bound::{self, SELECTED as PROVISIONAL},
    phase1,
    progress::{Phase, clock::Clock},
    run::{
        hooks::{Tally, memory},
        process,
    },
    spec,
};
use unityrs_testkit::{child, guard::DEADLOCK_GUARD};
#[global_allocator]
static ALLOC: Counting<memory_support::Bridge> = Counting(memory_support::Bridge);
include!("memory_support/engine.rs");

/// The peer and all of its tasks stay on an uncounted runtime/thread. The
/// driver owns the child scene; an outside kill retries fixtures and peer.
struct Peer {
    fake: Arc<Fake>,
    end: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Peer {
    fn start() -> Self {
        let (send, receive) = std::sync::mpsc::channel();
        let (end, done) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            store_support::run(async {
                let fake = Arc::new(
                    Fake::start_with(store_support::fake::Config {
                        credentials: store_support::sign::Credentials {
                            access_key: "K".repeat(128),
                            secret: "S".repeat(128),
                            token: Some("T".repeat(16384)),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .await,
                );
                send.send(fake.clone()).unwrap();
                let _ = done.await;
            });
        });
        Self {
            fake: unityrs_testkit::guard::recv(&receive, "store peer"),
            end: Some(end),
            thread: Some(thread),
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.end.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

fn fixture(root: &Path, large: bool, count: usize) {
    let side = if large { 2048 } else { 1 };
    let mut bytes = vec![0; side * side * 4];
    let mut rng = unityrs_testkit::Rng::new(42);
    for part in bytes.as_chunks_mut::<8>().0 {
        part.copy_from_slice(&rng.next_u64().to_le_bytes());
    }
    let file = unityrs_testkit::ngui::file(
        &[],
        &(0..count)
            .map(|i| {
                unityrs_testkit::ngui::texture(
                    i64::try_from(i + 1).unwrap(),
                    if count == 1 {
                        b"pixel".to_vec()
                    } else {
                        format!("t{i:06}").into_bytes()
                    }
                    .as_slice(),
                    4,
                    (i32::try_from(side).unwrap(), i32::try_from(side).unwrap()),
                    bytes.clone(),
                )
            })
            .collect::<Vec<_>>(),
        true,
    )
    .unwrap();
    fs::write(
        root.join("input"),
        unityrs_testkit::env::bundle(&[("CAB-synthetic", file)]),
    )
    .unwrap();
}
fn command(job: &Path) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "memory_child", "--nocapture", "--test-threads=1"])
        .env("X12_JOB", job)
        .env("AWS_ACCESS_KEY_ID", "K".repeat(128))
        .env("AWS_SECRET_ACCESS_KEY", "S".repeat(128))
        .env("AWS_SESSION_TOKEN", "T".repeat(16384))
        .env_remove("AWS_CREDENTIAL_EXPIRATION")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}
#[derive(Debug)]
struct Report(Vec<i64>);
impl Report {
    fn read(path: &Path) -> Self {
        let text = fs::read_to_string(path).unwrap();
        let fields: Vec<i64> = text
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(fields.len(), 19, "terminal observations: {text}");
        let r = Self(fields);
        assert!(r.0[0] <= r.0[1], "live residual: {r:?}");
        assert!(r.0[2] <= r.0[3], "process bound: {r:?}");
        assert!(r.0[5] > 100, "empty measurement: {r:?}");
        r
    }
}

/// `operation` holds a real request after its body was read and signed.
/// Phase holds use the official phase observer; neither uses time as order.
#[allow(clippy::too_many_lines)] // One supervised scene owns setup, seed run, stopped run and peer lifetime.
fn run(phase: Option<u64>, operation: Option<Operation>, mode: &str) -> Report {
    child::scene(|scene| {
        let peer = Peer::start();
        let fake = &peer.fake;
        let root = scene.scratch.path().to_owned();
        fs::create_dir(root.join("spool"))?;
        fixture(&root, mode == "multipart", mode.strip_prefix("scale-").map_or(1, |n| n.parse().unwrap()));
        let mut spec = common::base();
        spec["run_id"] = json!("r");
        spec["threads"] = json!(1);
        spec["download_concurrency"] = json!(1);
        spec["max_inflight_uncompressed_bytes"] = json!(512 << 20);
        spec["max_decoded_bytes"] = json!(128 << 20);
        spec["dep_cache_bytes"] = json!(128 << 20);
        spec["spool"]["dir"] = json!(root.join("spool"));
        spec["spool"]["size_bytes"] = json!(1u64 << 34);
        spec["inputs"] = json!([{"id":"a", "path":root.join("input")}]);
        spec["dependencies"] = json!([]);
        spec["deadline"] = json!("2099-01-01T00:00:00Z");
        spec["sink"] = json!({"kind":"object_store", "bucket":"bucket", "region":"us-east-1", "prefix":"p/", "endpoint":fake.endpoint.as_str(), "path_style":true, "attempts":2});
        spec["http"] = json!({"backoff_initial_ms":0, "backoff_max_ms":0});
        spec["archives"] = json!([{"key":"all.zip","members":[{"kind":"texture","path":"{name}.png"}]}]);
        let origin = url_run_support::Origin::start();
        let base = mode == "base";
        let late = matches!(mode, "finish-row" | "finish-head");
        let response_gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if base {
            spec["inputs"].as_array_mut().unwrap().push(json!({"id":"failed", "path":root.join("missing")}));
            spec["dependencies"] = json!([{"id":"lookup", "url":"http://peer.invalid/unused"}]);
            spec["http"]["attempts"] = json!(1);
            spec["http"]["connect_timeout_ms"] = json!(1000);
        }
        if mode == "multipart" {
            spec["spec_version"] = json!(2);
            spec["outputs"]["image"] = json!("png_fast");
        }
        if mode == "read-back" {
            for ordinal in [1, 3] {
                fake.schedule(Operation::Put, "p/_unityrs/r/manifest.json", ordinal,
                    Fault { lost_response:true, ..Default::default() });
            }
        }
        if mode == "error" {
            fake.schedule(Operation::Put, "p/a/pixel.png", 1, Fault::error(503, "SlowDown"));
        }
        // Force several LIST pages; no completed page may remain retained.
        for n in 0..5 { fake.seed("bucket", &format!("p/unused{n}"), b"x"); }
        for ordinal in 1..8 {
            fake.schedule_list("p/", None, ordinal, Fault { list_page_size:Some(2), ..Default::default() });
        }
        if mode == "error" {
            fake.schedule_list("p/", None, 2, Fault::error(403, "AccessDenied"));
        }
        let stopped = phase.is_some() || operation.is_some() || base || late;
        let lifecycle = if stopped { Some(child::Fifo::new(scene, "trace.lifecycle")?) } else { None };
        if let Some(op) = operation {
            if op == Operation::List {
                fake.schedule_list("p/", None, 2, Fault { before_head:Some(DEADLOCK_GUARD), ..Default::default() });
            } else {
                let key = if op == Operation::Abort { format!("p/{}", "x".repeat(1022)) }
                    else if op == Operation::Put { "p/_unityrs/r/manifest.json".to_owned() }
                    else { "p/a/pixel.png".to_owned() };
                let fault = if op == Operation::Abort {
                    Fault { head_gate:Some(response_gate.clone()), ..Default::default() }
                } else { Fault { before_head:Some(DEADLOCK_GUARD), ..Default::default() } };
                fake.schedule(op, &key, 1, fault);
            }
        }
        if late {
            fake.schedule(Operation::Put,
                if mode == "finish-row" { "p/_unityrs/r/1/bundles.0.ndjson" } else { "p/_unityrs/r/manifest.json" },
                if mode == "finish-row" { 1 } else { 2 },
                Fault { head_gate:Some(response_gate.clone()), ..Default::default() });
        }
        let job = root.join("job.json");
        let mut completed_peak = 0;
        fs::write(&job, serde_json::to_vec(&spec)?)?;
        for file in ["ready", "report"] { fs::write(root.join(file), b"")?; }
        if mode == "resume" || base {
            let mut seed = command(&job);
            if base {
                seed.env("X12_RESOLVE", format!("127.0.0.1={},peer.invalid={}", fake.address(), origin.address));
            }
            let first = child::live(scene, &mut seed, false)?;
            let (_, out) = child::drain(scene.take_stdout(first).unwrap());
            let (_, err) = child::drain(scene.take_stderr(first).unwrap());
            assert_eq!(scene.wait(first)?.code(), Some(if base {4} else {0}));
            out.join().unwrap(); err.join().unwrap();
            completed_peak = Report::read(&root.join("report")).0[2];
            let key = format!("p/{}", "x".repeat(1022));
            let id = "u".repeat(1024);
            fake.seed_upload(&key, &id);
            let journal = unityrs_batch::spool::journal::Journal::new(&root.join("spool/runs/r")).unwrap();
            journal.append(unityrs_batch::spool::journal::Entry {
                endpoint: Some(fake.endpoint.as_str()), bucket:"bucket", key:&key, upload_id:&id,
            }).unwrap();
            spec["attempt"] = json!(2);
            spec["resume"] = json!(true);
            fs::write(&job, serde_json::to_vec(&spec)?)?;
            fs::write(root.join("report"), b"")?;
        }
        let mut cmd = command(&job);
        if base {
            cmd.env("X12_RESOLVE", format!("127.0.0.1={},peer.invalid={}+hold:{}", fake.address(), origin.address, root.join("lookup-release").display()));
        }
        if stopped { cmd.env("UNITYRS_TEST_TRACE", root.join("trace")); }
        if let Some(phase) = phase { cmd.env("X12_HOLD", phase.to_string()); }
        let engine = child::live(scene, &mut cmd, false)?;
        let (out, stdout) = child::drain(scene.take_stdout(engine).unwrap());
        let (err, stderr) = child::drain(scene.take_stderr(engine).unwrap());
        if stopped {
            let until = Instant::now() + DEADLOCK_GUARD;
            loop {
                let ready = if late {
                    fake.logs().iter().any(|l| l.operation == Operation::Put && l.signature_valid &&
                        if mode == "finish-row" { l.raw_path.ends_with("/bundles.0.ndjson") }
                        else { l.raw_path.ends_with("/manifest.json") && l.try_ordinal == 2 })
                } else if base {
                    fs::read_to_string(root.join("trace")).unwrap_or_default().contains("resolve peer.invalid held")
                } else if let Some(op) = operation {
                    fake.logs().iter().any(|l| l.operation == op && l.signature_valid && (op != Operation::List || l.try_ordinal == 2))
                } else { fs::metadata(root.join("ready"))?.len() > 0 };
                if ready { break; }
                assert!(scene.running(engine)?, "store child ended before acknowledgment: {:?}", err.try_iter().collect::<Vec<_>>());
                assert!(Instant::now() < until, "store acknowledgment");
                std::thread::yield_now();
            }
            if phase != Some(0) {
                scene.signal(engine, libc::SIGTERM)?;
                while lifecycle.as_ref().unwrap().line_while(scene, engine)? != "lifecycle_stop_woken reached" {}
            }
            if phase.is_some() { fs::write(root.join("release"), b"")?; }
            if late || operation == Some(Operation::Abort) { response_gate.store(true, std::sync::atomic::Ordering::SeqCst); }
        }
        let status = scene.wait(engine)?;
        stdout.join().unwrap(); stderr.join().unwrap();
        let out = String::from_utf8(out.try_iter().flatten().collect()).unwrap();
        let err = String::from_utf8(err.try_iter().flatten().collect()).unwrap();
        assert_eq!(status.code(), Some(if late {0} else if stopped {6} else if base {4} else {0}), "{status}: {out}\n{err}");
        let mut report = Report::read(&root.join("report"));
        // Calibration covers both complete archive/verification/head work
        // and the stopped scene with a lookup retained through finishing.
        report.0[2] = report.0[2].max(completed_peak);
        if !stopped || late {
            assert_eq!(report.0[4], 0, "normal completion releases every charge");
            assert_eq!(report.0[6] & !64, 1 | 2 | 4 | 8 | 16 | 128 | 256 | 512, "all store engine roles: {report:?}");
            assert_eq!(report.0[7], 1);
            assert_eq!(report.0[8], 2);
            assert_eq!(report.0[9], 2);
            assert_eq!(report.0[11], 255, "all store phases: {report:?}");
            assert!(fake.logs().iter().all(|l| l.signature_valid));
            let head: Value = serde_json::from_slice(&fake.object("bucket", "p/_unityrs/r/manifest.json").unwrap().bytes)?;
            assert_eq!(head["write_seq"], 1);
            assert_eq!(head["run"]["memory_bound"], report.0[3]);
            if late {
                assert_eq!(head["status"], "complete");
                assert_eq!(head["stop_reason"], Value::Null);
                assert_eq!(head["exit_code"], 0);
            }
            if mode == "multipart" {
                for op in [Operation::Create, Operation::Part, Operation::Complete] {
                    assert!(fake.logs().iter().any(|l| l.operation == op));
                }
            }
            if mode == "read-back" {
                assert!(fake.logs().iter().filter(|l| l.operation == Operation::Get).count() >= 3);
            }
            if mode == "error" {
                assert_eq!(head["verified"], false);
                assert_eq!(head["warnings"]["verify_list_failed"], 1);
            }
            if mode == "resume" || base {
                assert!(fake.logs().iter().any(|l| l.operation == Operation::Abort));
                let journal = unityrs_batch::spool::journal::Journal::new(&root.join("spool/runs/r")).unwrap();
                assert!(journal.reader().unwrap().next().unwrap().is_none());
            }
        }
        if operation == Some(Operation::Part) {
            assert!(report.0[4] >= 16 << 20, "frozen multipart payload lost its charge: {report:?}");
            assert!(report.0[15] > 0, "frozen request lost its network reservation: {report:?}");
        }
        Ok(report)
    }).unwrap()
}
#[test]
fn store_base_resume_recovers_maximum_journal_ids_with_failed_input_and_held_lookup() {
    let r = run(None, None, "base");
    // Deliberately deduct nothing: the complete measured scene, including
    // charged/native/network/finisher overlap, conservatively remains visible.
    println!("\nB12_COEFFICIENT c_base {}", r.0[2]);
    assert!(
        u64::try_from(r.0[2]).unwrap() <= bound::SELECTED.c_base,
        "{r:?}"
    );
}
#[test]
fn store_resume_and_journal_rewrite_return_all_ownership() {
    run(None, None, "resume");
}
#[test]
fn store_error_bodies_and_retry_verification_failure_remain_bounded() {
    run(None, None, "error");
}
#[test]
fn store_stop_during_old_journal_abort_retains_its_counted_identity() {
    run(None, Some(Operation::Abort), "resume");
}
#[test]
fn store_stop_inside_finisher_rows_and_final_head_keeps_its_normal_snapshot() {
    for mode in ["finish-row", "finish-head"] {
        run(None, None, mode);
    }
}
#[test]
fn store_process_counts_signed_pages_verification_and_final_head() {
    run(None, None, "normal");
}
#[test]
fn store_process_read_back_and_rewrite_stays_bounded() {
    run(None, None, "read-back");
}
#[test]
fn store_stops_in_every_phase_retain_counted_owners() {
    for phase in 0..7 {
        run(Some(phase), None, "normal");
    }
}
#[test]
fn store_stops_with_signed_running_and_verification_requests_frozen() {
    for op in [Operation::Put, Operation::List] {
        run(None, Some(op), "normal");
    }
}
#[test]
#[cfg_attr(debug_assertions, ignore = "scale: whole-process multipart")]
fn store_multipart_normal_and_stopped_transfers_remain_counted() {
    run(None, None, "multipart");
    for op in [Operation::Create, Operation::Part, Operation::Complete] {
        run(None, Some(op), "multipart");
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "scale: store output and LIST history")]
fn store_output_and_page_counts_leave_no_retained_history() {
    let mut residuals = Vec::new();
    for count in [1, 128, 2048] {
        let r = run(None, None, &format!("scale-{count}"));
        residuals.push((r.0[17] - r.0[14], r.0[12] - r.0[4] - r.0[14]));
    }
    let span = |column: usize| {
        let values: Vec<_> = residuals
            .iter()
            .map(|r| if column == 0 { r.0 } else { r.1 })
            .collect();
        values.iter().max().unwrap() - values.iter().min().unwrap()
    };
    assert!(span(0) <= 128 << 10, "store peak slope: {residuals:?}");
    assert!(span(1) <= 16 << 10, "store retained slope: {residuals:?}");
}
