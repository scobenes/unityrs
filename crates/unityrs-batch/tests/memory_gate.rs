//! Integrated directory/HTTP X12 (`extract.md` X12; `batch.md` 5.1–5.3,
//! 5.8). Real process termination, both runtimes, all engine threads. These
//! scenes verify the compiled selection. Designated observations also
//! supply raw evidence to the calibration pipeline.
#![cfg(all(target_os = "linux", feature = "test-hooks"))]
mod memory_support;
mod run_support;
#[path = "http_support/stress.rs"]
mod stress;
#[path = "http_support/tls.rs"]
#[allow(dead_code)]
mod tls;
mod url_run_support;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};
use unityrs_batch::{
    alloc::{self, Counting},
    bound::{self, SELECTED},
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

#[derive(Debug)]
struct Report(Vec<i64>);
impl Report {
    fn read(path: &Path) -> Self {
        let text = fs::read_to_string(path).unwrap();
        let fields = text
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(fields.len(), 19, "missing terminal observation: {text}");
        Self(fields)
    }
    fn check(&self, workers: u64, dc: u64, stopped: bool) {
        assert!(
            self.0[0] <= self.0[1],
            "live held-charge residual: {self:?}"
        );
        assert!(self.0[2] <= self.0[3], "whole process bound: {self:?}");
        assert!(self.0[5] > 100, "no allocation events: {self:?}");
        if !stopped {
            assert_eq!(
                self.0[6] & !64,
                1 | 2 | 4 | 8 | 16 | 32 | 128 | 256 | 512,
                "all roles: {self:?}"
            );
            assert_eq!(self.0[7], i64::try_from(workers).unwrap());
            assert_eq!(self.0[8], i64::try_from(dc).unwrap() + 1);
            assert_eq!(self.0[9], i64::try_from(dc).unwrap() + 1);
            assert_eq!(self.0[10], 2);
            assert_eq!(self.0[11], 255, "all phases: {self:?}");
            assert_eq!(self.0[4], 0, "normal completion releases charges: {self:?}");
        }
    }
}

fn command(job: &Path) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "memory_child", "--nocapture", "--test-threads=1"])
        .env("X12_JOB", job)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Whole-scene retries include fixtures and the local origin; no external DNS
/// or service. Stop holds acknowledge the phase before the signal is sent.
fn run(case: &Value, http: bool, threads: u64, dc: u64, stop: Option<u64>, inject: bool) -> Report {
    run_extra(case, http, threads, dc, stop, inject, "")
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One scene owns fixtures, peers, resume children and cleanup on outside kills.
fn run_extra(
    case: &Value,
    http: bool,
    threads: u64,
    dc: u64,
    stop: Option<u64>,
    inject: bool,
    extra: &str,
) -> Report {
    child::scene(|scene| {
        let root = scene.scratch.path().to_owned();
        fs::create_dir(root.join("spool"))?;
        fs::create_dir(root.join("sink"))?;
        let all_formats = extra == "formats";
        let fixture = if all_formats {
            memory_support::formats::scene()
        } else {
            run_support::scene(case)
        };
        let (mut spec, expected) = run_support::instantiate(case, &fixture, &scene.scratch);
        let scale = extra == "scale";
        if all_formats {
            spec["spec_version"] = json!(2);
            spec["outputs"]["formats"] = unityrs_export::ngui::Format::ALL
                .iter()
                .map(|f| json!(f.name()))
                .collect();
        }
        let requests = extra == "requests";
        if requests {
            let dependency = spec["dependencies"][0].clone();
            let deps = spec["dependencies"].as_array_mut().unwrap();
            for i in 0..case["request_copies"].as_u64().unwrap() {
                let mut copy = dependency.clone();
                copy["id"] = json!(format!("unused{i:06}"));
                deps.push(copy);
            }
        }
        spec["threads"] = json!(threads);
        spec["download_concurrency"] = json!(dc);
        // The negative control spends only a small part of this headroom
        // beyond its residual allowance, so it can still fit the total bound.
        spec["max_inflight_uncompressed_bytes"] = json!(256 << 20);
        spec["stop_on_stdin_eof"] = json!(true);
        let origin = http.then(url_run_support::Origin::start);
        if let Some(origin) = &origin {
            url_run_support::onto(origin, &mut spec, |_| true);
            if extra == "body" {
                origin.script("/b0", [url_run_support::Reply::Hold(32)]);
            }
        }
        let tls = if extra == "tls" {
            let input = &mut spec["inputs"][0];
            let body = fs::read(input["path"].as_str().unwrap())?;
            let response = tls_response(&body);
            let peer = tls::Peer::with_aligned_config(response, stress::config("maximal-combined"));
            input.as_object_mut().unwrap().remove("path");
            input["url"] = json!(format!("https://localhost:{}/b0", peer.address.port()));
            Some(peer)
        } else {
            None
        };
        if extra == "lookup" {
            let origin = origin.as_ref().unwrap();
            for list in ["inputs", "dependencies"] {
                for bundle in spec[list].as_array_mut().into_iter().flatten() {
                    let url = bundle["url"]
                        .as_str()
                        .unwrap()
                        .replace(&origin.address.to_string(), "peer.invalid");
                    bundle["url"] = json!(url);
                }
            }
        }
        let mut answers = Vec::new();
        if requests {
            let origin = origin.as_ref().unwrap();
            for list in ["inputs", "dependencies"] {
                for bundle in spec[list].as_array_mut().into_iter().flatten() {
                    let host = format!("h{}.invalid", answers.len());
                    bundle["url"] = json!(
                        bundle["url"]
                            .as_str()
                            .unwrap()
                            .replace(&origin.address.to_string(), &host)
                    );
                    answers.push(format!("{host}={}", origin.address));
                }
            }
            for i in 0..answers.len() {
                origin.script(
                    &format!("/b{i}"),
                    [url_run_support::Reply::Status(503, String::new())],
                );
            }
            spec["http"] = json!({"attempts":2,"backoff_initial_ms":0,"backoff_max_ms":0});
        }
        let job = root.join("job.json");
        fs::write(&job, serde_json::to_vec(&spec)?)?;
        for file in ["ready", "report"] {
            fs::write(root.join(file), b"")?;
        }
        let mut cmd = command(&job);
        if requests {
            cmd.env("X12_RESOLVE", answers.join(","));
        }
        let stopped = stop.is_some() || matches!(extra, "freeze" | "lookup" | "body" | "attempt");
        let lifecycle = if stopped {
            Some(child::Fifo::new(scene, "trace.lifecycle")?)
        } else {
            None
        };
        if stopped {
            cmd.env("UNITYRS_TEST_TRACE", root.join("trace"));
        }
        if let Some(stop) = stop {
            cmd.env("X12_HOLD", stop.to_string());
        }
        if inject {
            cmd.env("X12_INJECT", "1");
        }
        if let Some(peer) = &tls {
            cmd.env("X12_TLS", peer.address.to_string());
        }
        if extra == "freeze" {
            cmd.env("X12_FREEZE", "1");
        }
        if extra == "attempt" {
            cmd.env("X12_ATTEMPT", "1");
        }
        if extra == "lookup" {
            cmd.env("UNITYRS_TEST_TRACE", root.join("trace")).env(
                "UNITYRS_TEST_RESOLVE",
                format!(
                    "peer.invalid={}+hold:{}",
                    origin.as_ref().unwrap().address,
                    root.join("lookup-release").display()
                ),
            );
        }
        if let Some(env) = case["env"].as_object() {
            for (key, value) in env {
                cmd.env(key, value.as_str().unwrap());
            }
        }
        let engine = child::live(scene, &mut cmd, false)?;
        let (out, stdout) = child::drain(scene.take_stdout(engine).unwrap());
        let (err, stderr) = child::drain(scene.take_stderr(engine).unwrap());
        if stopped {
            let until = Instant::now() + DEADLOCK_GUARD;
            while if extra == "lookup" {
                !fs::read_to_string(root.join("trace"))
                    .unwrap_or_default()
                    .contains("resolve peer.invalid held")
            } else if extra == "body" {
                !fs::metadata(
                    root.join("spool/runs")
                        .join(spec["run_id"].as_str().unwrap())
                        .join("in/i0"),
                )
                .is_ok_and(|m| m.len() >= 32)
            } else {
                fs::metadata(root.join("ready"))?.len() == 0
            } {
                assert!(
                    scene.running(engine)?,
                    "engine ended before phase hold {stop:?}/{extra}: {} {}",
                    String::from_utf8_lossy(&out.try_iter().flatten().collect::<Vec<_>>()),
                    String::from_utf8_lossy(&err.try_iter().flatten().collect::<Vec<_>>())
                );
                assert!(Instant::now() < until, "phase acknowledgment");
                std::thread::yield_now();
            }
            if stop != Some(0) {
                let parsed = unityrs_batch::spec::parse(serde_json::to_vec(&spec)?).unwrap();
                let needs = unityrs_batch::start::host::Requirements::new(&parsed.spec, 0).unwrap();
                let pid = scene.id(engine);
                let threads = fs::read_dir(format!("/proc/{pid}/task"))?.count() as u64;
                let files = fs::read_dir(format!("/proc/{pid}/fd"))?.count() as u64;
                // The libtest runner and the two report/hold descriptors are
                // test infrastructure, explicitly outside the engine tally.
                assert!(
                    threads <= needs.threads + 1,
                    "thread floor: {threads} > {}",
                    needs.threads
                );
                assert!(
                    files <= needs.files + 2,
                    "descriptor floor: {files} > {}",
                    needs.files
                );
            }
            if stop != Some(0) {
                scene.signal(engine, libc::SIGTERM)?;
                while lifecycle.as_ref().unwrap().line_while(scene, engine)?
                    != "lifecycle_stop_woken reached"
                {}
            }
            if extra.is_empty() {
                fs::write(root.join("release"), b"")?;
            }
        }
        let status = scene.wait(engine)?;
        stdout.join().unwrap();
        stderr.join().unwrap();
        let out = String::from_utf8(out.try_iter().flatten().collect()).unwrap();
        let err = String::from_utf8(err.try_iter().flatten().collect()).unwrap();
        assert_eq!(
            status.code(),
            Some(if stopped {
                6
            } else {
                i32::try_from(case["exit_code"].as_i64().unwrap()).unwrap()
            }),
            "{status}: {out}\n{err}"
        );
        let mut report = Report::read(&root.join("report"));
        if !inject {
            report.check(threads, dc, stopped);
        }
        if !stopped
            && !requests
            && !all_formats
            && !scale
            && case["missing_sources"].as_array().is_none_or(Vec::is_empty)
        {
            run_support::check(
                &scene.scratch,
                &spec,
                case,
                expected,
                u64::try_from(report.0[3]).unwrap(),
            );
        }
        if scale {
            let manifest = root.join("sink/_unityrs/golden/manifest.json");
            let head: Value = serde_json::from_slice(&fs::read(manifest)?)?;
            assert_eq!(head["run"]["memory_bound"], report.0[3]);
            // Every generated texture has its own loose output and ZIP
            // member; the head counts chunk files rather than row records.
            assert!(head["rows"]["outputs"].as_u64().unwrap() > 0);
            assert!(root.join("sink/all.zip").is_file());
        }
        if all_formats {
            let files = fs::read_dir(root.join("sink"))?
                .map(|e| e.unwrap().path())
                .filter(|p| p.is_file())
                .count();
            assert_eq!(files, unityrs_export::ngui::Format::ALL.len());
        }
        if matches!(extra, "resume-hit" | "resume-miss") {
            if extra == "resume-miss" {
                let key = case["outputs"].as_object().unwrap().keys().next().unwrap();
                let path = root.join("sink").join(key);
                let mut bytes = fs::read(&path)?;
                bytes[0] ^= 1;
                fs::write(path, bytes)?;
            }
            spec["attempt"] = json!(2);
            spec["resume"] = json!(true);
            fs::write(&job, serde_json::to_vec(&spec)?)?;
            fs::write(root.join("report"), b"")?;
            let mut cmd = command(&job);
            let resumed = child::live(scene, &mut cmd, false)?;
            let (_, out) = child::drain(scene.take_stdout(resumed).unwrap());
            let (_, err) = child::drain(scene.take_stderr(resumed).unwrap());
            assert_eq!(scene.wait(resumed)?.code(), Some(0));
            out.join().unwrap();
            err.join().unwrap();
            report = Report::read(&root.join("report"));
            report.check(threads, dc, false);
            let (_, expected) = run_support::instantiate(case, &fixture, &scene.scratch);
            run_support::check(
                &scene.scratch,
                &spec,
                case,
                expected,
                u64::try_from(report.0[3]).unwrap(),
            );
        }
        if let Some(origin) = origin {
            if requests {
                assert_eq!(origin.total(), answers.len() * 2);
                // Peer observation of RST can lag the engine's close and
                // next request; it is not a live permit/socket census.
                assert!(
                    origin
                        .wait_closed()
                        .iter()
                        .all(|c| *c == url_run_support::Close::Reset)
                );
            } else {
                assert!(origin.most_open() <= usize::try_from(dc).unwrap());
            }
        }
        if let Some(peer) = tls {
            let observed = peer.finish()?;
            assert!(observed.reset && !observed.close_notify);
            assert_eq!(observed.server_name.as_deref(), Some("localhost"));
        }
        Ok(report)
    })
    .unwrap()
}

fn tls_response(body: &[u8]) -> Vec<u8> {
    fn head(prefix: &[u8]) -> Vec<u8> {
        let mut bytes = prefix.to_vec();
        for i in 0..98 {
            bytes.extend_from_slice(format!("x-head-{i}: a\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"x-last: ");
        bytes.resize(131_068, b'a');
        bytes.extend_from_slice(b"\r\n\r\n");
        bytes
    }
    let mut bytes = head(b"HTTP/1.1 103 Early Hints\r\n");
    bytes.extend_from_slice(&head(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n"));
    bytes.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(b"\r\n0\r\n");
    let start = bytes.len();
    for i in 0..99 {
        bytes.extend_from_slice(format!("x-trailer-{i}: a\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"x-last: ");
    bytes.resize(start + 131_067, b'y');
    bytes.extend_from_slice(b"\r\n\r\n");
    bytes
}

#[test]
fn stopped_finishing_overlaps_a_frozen_charged_publication_buffer() {
    for http in [false, true] {
        let report = run_extra(&sample(), http, 2, 1, None, false, "freeze");
        assert!(
            report.0[13] >= 16 << 20,
            "no charged buffer frozen: {report:?}"
        );
        assert!(
            report.0[4] >= 16 << 20,
            "frozen owner released before exit: {report:?}"
        );
    }
}

#[test]
fn stopped_finishing_overlaps_a_frozen_attempt_and_its_live_charges() {
    for http in [false, true] {
        let report = run_extra(&sample(), http, 2, 1, None, false, "attempt");
        assert!(
            report.0[13] > 0 && report.0[4] >= report.0[13],
            "frozen attempt charges: {report:?}"
        );
    }
}

#[test]
fn stopped_url_lookup_stays_on_its_counted_pool_beside_finishing() {
    let report = run_extra(&sample(), true, 2, 1, None, false, "lookup");
    assert_eq!((report.0[8], report.0[9]), (2, 2));
    assert!(report.0[11] & (1 << Phase::Finish as u32) != 0);
}

#[test]
fn stopped_url_body_keeps_its_stage_and_permit_beside_finishing() {
    let report = run_extra(&sample(), true, 2, 1, None, false, "body");
    assert!(
        report.0[15]
            >= i64::try_from(
                unityrs_batch::http::LIBRARY_ENVELOPE + unityrs_batch::stage::url::STAGE_ENVELOPE
            )
            .unwrap(),
        "no frozen network/stage charge: {report:?}"
    );
}

#[test]
fn tls12_near_cap_http_and_reframed_handshake_are_counted_in_process() {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "textures-five")
        .unwrap();
    run_extra(&case, false, 2, 1, None, false, "tls");
}

#[test]
fn process_resume_hit_and_hash_miss_keep_the_same_memory_gate() {
    for http in [false, true] {
        for mode in ["resume-hit", "resume-miss"] {
            run_extra(&sample(), http, 2, 1, None, false, mode);
        }
    }
}

#[test]
fn every_texture_format_runs_through_the_counted_process() {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "textures-five")
        .unwrap();
    for http in [false, true] {
        run_extra(&case, http, 2, 1, None, false, "formats");
    }
}

fn sample() -> Value {
    run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-material_in_dependency")
        .unwrap()
}

#[test]
fn directory_and_http_processes_count_all_threads_and_live_charges() {
    for http in [false, true] {
        run(&sample(), http, 2, 2, None, false);
    }
}

#[test]
fn every_phase_stop_keeps_the_counted_owners_through_exit() {
    for http in [false, true] {
        for phase in 0..7 {
            run(&sample(), http, 2, 1, Some(phase), false);
        }
    }
}

#[test]
fn an_uncharged_holder_fails_even_with_unused_admission_capacity() {
    let report = run(&sample(), false, 1, 1, None, true);
    assert!(
        report.0[0] > report.0[1],
        "injected holder escaped gate: {report:?}"
    );
    assert!(
        report.0[2] < report.0[3],
        "the global bound alone must not detect this: {report:?}"
    );
}

#[test]
#[cfg_attr(debug_assertions, ignore = "scale: complete process export matrix")]
fn process_export_matrix_directory_http_and_pool_counts() {
    for case in run_support::cases() {
        for http in [false, true] {
            for (threads, dc) in [(1, 1), (4, 3)] {
                run(&case, http, threads, dc, None, false);
            }
        }
    }
}

/// A fixed spec shape with increasing output/member counts. Source-sized
/// structures are charged; neither published objects nor completed requests
/// may accumulate in a process-wide collection behind the base estimate.
#[test]
#[cfg_attr(debug_assertions, ignore = "scale: output/member residual growth")]
fn output_and_zip_member_growth_leaves_no_count_proportional_residual() {
    let mut case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "textures-five")
        .unwrap();
    case["name"] = json!("memory-scale");
    case["spec"]["max_decoded_bytes"] = json!(128 << 20);
    case["spec"]["dep_cache_bytes"] = json!(128 << 20);
    case["spec"]["spool"]["size_bytes"] = json!(1u64 << 34);
    case["spec"]["archives"] =
        json!([{"key":"all.zip","members":[{"kind":"texture","path":"{name}.png"}]}]);
    let mut residuals = Vec::new();
    for n in [1, 128, 2048] {
        case["texture_inputs"] = (0..n)
            .map(|i| json!({"name":format!("t{i:06}"),"format":4,"size":[1,1],"data":{"zeros":4}}))
            .collect();
        for http in [false, true] {
            let r = run_extra(&case, http, 2, 2, None, false, "scale");
            if n == 2048 {
                assert!(r.0[18] > 16, "multiple sort generations not reached: {r:?}");
            }
            // Remove only the actually measured preparation, never a cap.
            residuals.push((r.0[17] - r.0[14], r.0[12] - r.0[4] - r.0[14]));
            println!("X12 growth n={n}: max residual in phase {}", r.0[16]);
        }
    }
    let peak_span =
        residuals.iter().map(|r| r.0).max().unwrap() - residuals.iter().map(|r| r.0).min().unwrap();
    let kept_span =
        residuals.iter().map(|r| r.1).max().unwrap() - residuals.iter().map(|r| r.1).min().unwrap();
    println!("X12 output/member residuals {residuals:?}");
    // One fixed 64 KiB cursor plus runtime scheduling/allocator granularity.
    // The spread gate is independent of the selected base.
    assert!(peak_span <= 128 << 10, "peak count slope: {residuals:?}");
    assert!(kept_span <= 16 << 10, "retained count slope: {residuals:?}");
}

#[test]
#[cfg_attr(debug_assertions, ignore = "scale: hostname/retry residual growth")]
fn sequential_hostnames_and_retries_leave_no_completed_request_history() {
    let mut case = sample();
    let mut kept = Vec::new();
    for n in [0, 32, 512] {
        case["request_copies"] = json!(n);
        let r = run_extra(&case, true, 1, 1, None, false, "requests");
        kept.push(r.0[12] - r.0[4] - r.0[14]);
    }
    println!("X12 completed-request retained residuals {kept:?}");
    assert!(
        kept.iter().max().unwrap() - kept.iter().min().unwrap() <= 16 << 10,
        "completed-request history grew: {kept:?}"
    );
}

#[test]
fn directory_base_measurement_keeps_all_conservative_overlap_visible() {
    let mut case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "archives-textures")
        .unwrap();
    case["spec"]["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"failed", "path":"/missing-synthetic-source"}));
    case["exit_code"] = json!(4);
    case["missing_sources"] = json!(["failed"]);
    let r = run(&case, false, 1, 1, None, false);
    // Zero deductions: no cap, unseen native charge, padded coefficient or
    // incompatible peak can manufacture headroom in this conservative base.
    println!("\nB12_COEFFICIENT c_base {}", r.0[2]);
}
