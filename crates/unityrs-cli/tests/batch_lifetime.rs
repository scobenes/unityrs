//! The process host through the binary (`docs/plans/batch.md` 2.11): the
//! stops a signal, the deadline, stdin's EOF and the parent's death make;
//! step 1's, step 4's, step 6's and step 7's 69s; the panics; and the test
//! hooks the scenes drive it with. Built only with `test-hooks`.
#![cfg(target_os = "linux")]
mod common;
mod scenes;
use scenes::{deadline, ended, head, held, job, lifecycle, start, traced};
use serde_json::json;
use std::{
    fs,
    process::Stdio,
    time::{Duration, Instant},
};
use unityrs_testkit::child::{self, Fifo};

#[test]
fn a_first_signal_stops_the_run_with_6_and_its_final_head() {
    for (signal, name) in [
        (libc::SIGTERM, "sigterm"),
        (libc::SIGINT, "sigint"),
        (libc::SIGHUP, "sighup"),
    ] {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let (trace, go, point) = held(&dir);
            let _fifo = Fifo::new(scene, "trace.lifecycle")?;
            let handover = dir.join("handover");
            let job = job(&dir, |_| {});
            let s = start(
                scene,
                &job,
                &[
                    ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                    ("UNITYRS_TEST_POINT", &point),
                    ("UNITYRS_TEST_HANDOVER", handover.to_str().unwrap()),
                ],
                Stdio::null(),
            );
            traced(scene, s.live, &trace, "poll reached");
            scene.signal(s.live, signal)?;
            fs::write(&go, b"")?;
            let (code, line, err) = ended(scene, s);
            assert_eq!(code, 6, "{name} {line} {err}");
            assert_eq!(line["reason"], name);
            let head = head(&dir);
            assert_eq!(head["stop_reason"], name);
            assert_ne!(head["status"], "running");
            assert!(head["deadline"].is_null());
            let log = fs::read_to_string(&trace)?;
            assert!(log.lines().any(|l| l == "handover"), "{log}");
            for l in fs::read_to_string(&handover)?.lines() {
                assert!(l.starts_with("stopped "), "{l}");
            }
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn a_second_signal_exits_at_once_with_the_stop_code() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, _go, point) = held(&dir);
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir, |_| {});
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::null(),
        );
        traced(scene, s.live, &trace, "poll reached");
        scene.signal(s.live, libc::SIGTERM)?;
        scene.signal(s.live, libc::SIGINT)?;
        // The controller is still held: the second signal's own exit.
        assert_eq!(scene.wait(s.live)?.code(), Some(6));
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_deadline_is_69_too_close_and_6_when_it_comes() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir, |v| {
            v["deadline"] = json!(deadline(Duration::from_secs(5)));
        });
        let s = start(scene, &job, &[], Stdio::null());
        let (code, line, err) = ended(scene, s);
        assert_eq!((code, &line["reason"]), (69, &json!("deadline_too_close")));
        assert_eq!(line["seq"], 0);
        assert_eq!(err, "unityrs batch: deadline_too_close\n");
        assert!(!dir.join("sink/_unityrs").exists());
        Ok(())
    })
    .unwrap();
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, go, point) = held(&dir);
        let fifo = Fifo::new(scene, "trace.lifecycle")?;
        // A deadline far off, its cutoff (the deadline less the grace) read
        // 1.5 s after step 6 however long startup took, so the controller
        // reaches its wait first even under load.
        let at = deadline(Duration::from_secs(600));
        let job = job(&dir, |v| {
            v["deadline"] = json!(at);
            v["shutdown_grace_ms"] = json!(10_000);
        });
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
                ("UNITYRS_TEST_CUTOFF_IN", "1500"),
            ],
            Stdio::null(),
        );
        traced(scene, s.live, &trace, "poll reached");
        lifecycle(scene, s.live, &fifo, "lifecycle_stop_woken reached");
        fs::write(&go, b"")?;
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 6, "{line} {err}");
        assert_eq!(line["reason"], "deadline");
        let head = head(&dir);
        assert_eq!(head["stop_reason"], "deadline");
        assert_eq!(head["deadline"], json!(at));
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_held_finisher_exits_at_t_hard_with_the_stop_code() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, go, _) = held(&dir);
        let fifo = Fifo::new(scene, "trace.lifecycle")?;
        let grace = 10_000;
        let job = job(&dir, |v| {
            v["deadline"] = json!(deadline(Duration::from_secs(600)));
            v["shutdown_grace_ms"] = json!(grace);
        });
        // The controller held at its wait, the finisher held for good.
        let point = format!(
            "poll=wait:{},finisher=wait:{}",
            go.display(),
            dir.join("never").display()
        );
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::null(),
        );
        // The stop sent once the controller holds at its wait, so no
        // startup under load runs past it.
        traced(scene, s.live, &trace, "poll reached");
        scene.signal(s.live, libc::SIGTERM)?;
        lifecycle(scene, s.live, &fifo, "lifecycle_stop_woken reached");
        // On CLOCK_BOOTTIME, as the lifecycle thread reads it: with the
        // deadline far off, T_hard is the stop's time plus the grace less
        // 500 ms, `grace - 500` ms after the stop woke.
        let woken = unityrs_batch::sys::boottime().unwrap();
        fs::write(&go, b"")?;
        traced(scene, s.live, &trace, "finisher reached");
        let status = scene.wait(s.live)?;
        let exited = unityrs_batch::sys::boottime().unwrap();
        assert_eq!(status.code(), Some(6));
        let after = exited.checked_sub(woken).unwrap();
        let hard = Duration::from_millis(grace - 500);
        assert!(after + Duration::from_millis(50) >= hard, "{after:?}");
        assert!(after < hard + Duration::from_secs(2), "{after:?}");
        // The final head is the finisher's, and it never ran.
        assert_eq!(head(&dir)["status"], "running");
        Ok(())
    })
    .unwrap();
}

#[test]
fn stdin_eof_stops_the_run_and_an_unwatchable_stdin_is_69() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, go, point) = held(&dir);
        let fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir, |v| v["stop_on_stdin_eof"] = json!(true));
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::piped(),
        );
        traced(scene, s.live, &trace, "poll reached");
        drop(scene.take_stdin(s.live));
        lifecycle(scene, s.live, &fifo, "lifecycle_stop_woken reached");
        fs::write(&go, b"")?;
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 6, "{line} {err}");
        assert_eq!(line["reason"], "stdin_eof");
        assert_eq!(head(&dir)["stop_reason"], "stdin_eof");
        Ok(())
    })
    .unwrap();
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir, |v| v["stop_on_stdin_eof"] = json!(true));
        let file = fs::File::open(&job)?;
        let s = start(scene, &job, &[], Stdio::from(file));
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 69, "{line} {err}");
        assert_eq!(line["reason"], "startup_failed");
        assert_eq!(line["detail"], "stdin");
        assert!(!dir.join("sink/_unityrs").exists());
        Ok(())
    })
    .unwrap();
}

/// A stdin watch that ended (a UDP socket whose peer's port is closed, its
/// ICMP error `POLLERR` with no EOF) before the controller's start: the
/// `running` head, sampled as it is encoded, records it, as the final head
/// does.
#[test]
fn the_running_head_records_a_stdin_watch_and_masks_its_resolved_header_secret() {
    use std::{net::UdpSocket, os::fd::OwnedFd};
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, go, poll) = held(&dir);
        let start_controller = dir.join("main");
        let points = format!("main=wait:{},{poll}", start_controller.display());
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        // The closed port is on 127.0.0.2, so no 127.0.0.1 bind of this or
        // another test can take it.
        let closed = UdpSocket::bind("127.0.0.2:0")?;
        let address = closed.local_addr()?;
        drop(closed);
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.connect(address)?;
        let job = job(&dir, |v| {
            v["stop_on_stdin_eof"] = json!(true);
            v["dependencies"] = json!([{"id":"unused", "url":"http://127.0.0.1:9/unused",
                "headers":{"X-Secret":{"env":"UNITYRS_STDIN_SECRET"}}}]);
            v["http"] = json!({"attempts":1});
        });
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &points),
                ("UNITYRS_STDIN_SECRET", "stdin"),
            ],
            Stdio::from(OwnedFd::from(socket.try_clone()?)),
        );
        traced(scene, s.live, &trace, "main reached");
        // Each datagram's ICMP error is the watch's `POLLERR`.
        let until = Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
        let mut err = Vec::new();
        while !String::from_utf8_lossy(&err)
            .lines()
            .any(|l| l.ends_with("watch ended"))
        {
            assert!(scene.running(s.live)?, "ended before the watch did");
            assert!(Instant::now() < until, "the watch never ended");
            let _ = socket.send(b"x");
            while let Ok(more) = s.err.0.try_recv() {
                err.extend(more);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(String::from_utf8_lossy(&err).contains("*** watch ended"));
        assert!(!String::from_utf8_lossy(&err).contains("stdin"));
        fs::write(&start_controller, b"")?;
        traced(scene, s.live, &trace, "poll reached");
        let running = head(&dir);
        assert_eq!(running["status"], "running", "{running}");
        assert_eq!(running["warnings"]["stdin_watch_ended"], 1, "{running}");
        fs::write(&go, b"")?;
        let (code, line, _) = ended(scene, s);
        assert_ne!(code, 6, "no stop: {line}");
        let last = head(&dir);
        assert_ne!(last["status"], "running", "{last}");
        assert_eq!(last["warnings"]["stdin_watch_ended"], 1, "{last}");
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_parent_other_than_the_one_named_stops_before_the_head() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        // Above every pid Linux gives (`pid_max` is at most 2^22).
        let job = job(&dir, |v| v["parent_pid"] = json!(1 << 22));
        let s = start(scene, &job, &[], Stdio::null());
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 6, "{line} {err}");
        assert_eq!(line["reason"], "parent_died");
        // A stop before `running`: every input `not_run`, the counts still
        // partitioning `total`.
        let inputs = &line["inputs"];
        assert_eq!(inputs["total"], 2, "{line}");
        assert_eq!(inputs["not_run"], 2, "{line}");
        assert_eq!(
            (&inputs["running"], &inputs["queued"]),
            (&json!(0), &json!(0))
        );
        assert!(!dir.join("sink/_unityrs/r/manifest.json").exists());
        Ok(())
    })
    .unwrap();
}

#[test]
fn every_failed_spawn_and_a_short_start_count_is_69_with_nothing_written() {
    let cases = [
        ("UNITYRS_TEST_SPAWN", "progress"),
        ("UNITYRS_TEST_SPAWN", "finisher"),
        ("UNITYRS_TEST_SPAWN", "lifecycle"),
        ("UNITYRS_TEST_SPAWN", "worker"),
        ("UNITYRS_TEST_SPAWN", "controller"),
        ("UNITYRS_TEST_START_COUNT", "1"),
    ];
    for (var, value) in cases {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let job = job(&dir, |_| {});
            let s = start(scene, &job, &[(var, value)], Stdio::null());
            let (code, line, err) = ended(scene, s);
            assert_eq!(code, 69, "{value} {line} {err}");
            assert_eq!(line["reason"], "thread_spawn_failed", "{value}");
            assert!(line["status"].is_null(), "{value}");
            assert_eq!(fs::read_dir(dir.join("sink"))?.count(), 0, "{value}");
            assert_eq!(fs::read_dir(dir.join("spool"))?.count(), 0, "{value}");
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn step_1_refuses_a_lifecycle_point_and_an_unusable_trace_fifo() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir, |_| {});
        // A regular file where the FIFO should be: `fstat`.
        let trace = dir.join("trace");
        fs::write(dir.join("trace.lifecycle"), b"")?;
        let s = start(
            scene,
            &job,
            &[("UNITYRS_TEST_TRACE", trace.to_str().unwrap())],
            Stdio::null(),
        );
        let (code, line, _) = ended(scene, s);
        assert_eq!((code, &line["detail"]), (69, &json!("fstat")));
        assert_eq!(line["reason"], "startup_failed");
        // A FIFO no one reads: `open ENXIO`.
        let other = dir.join("other");
        let fifo = std::ffi::CString::new(format!("{}.lifecycle", other.display()))?;
        // SAFETY: a NUL-terminated path; mode bits only.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let s = start(
            scene,
            &job,
            &[("UNITYRS_TEST_TRACE", other.to_str().unwrap())],
            Stdio::null(),
        );
        let (code, line, _) = ended(scene, s);
        assert_eq!((code, &line["detail"]), (69, &json!("open ENXIO")));
        let s = start(
            scene,
            &job,
            &[("UNITYRS_TEST_POINT", "lifecycle_stop_woken=spin")],
            Stdio::null(),
        );
        let (code, line, _) = ended(scene, s);
        assert_eq!((code, &line["reason"]), (69, &json!("startup_failed")));
        assert_eq!(line["phase"], "validate");
        assert!(!dir.join("sink/_unityrs").exists());
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_controller_panic_hands_over_and_a_finisher_panic_exits_70() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let trace = dir.join("trace");
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir, |_| {});
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", "poll=panic"),
            ],
            Stdio::null(),
        );
        // The controller's drive catches its own panic: a stop `internal`.
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 70, "{line} {err}");
        assert_eq!(line["reason"], "internal");
        let head = head(&dir);
        assert_eq!(head["stop_reason"], "internal");
        assert_ne!(head["status"], "running");
        Ok(())
    })
    .unwrap();
    // A panic in the body past the drive: the host's boundary hands over
    // what the body built, then writes its line, the finisher held here
    // until the line is seen.
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let trace = dir.join("trace");
        let go = dir.join("go");
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir, |_| {});
        let point = format!("verified=panic,finisher=wait:{}", go.display());
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::null(),
        );
        let mut err = Vec::new();
        let until = Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
        while !String::from_utf8_lossy(&err).contains("test hook: verified") {
            assert!(scene.running(s.live)?, "{}", String::from_utf8_lossy(&err));
            assert!(Instant::now() < until, "no panic line");
            if let Ok(chunk) = s.err.0.recv_timeout(Duration::from_millis(10)) {
                err.extend(chunk);
            }
        }
        let err = String::from_utf8(err).unwrap();
        assert!(err.starts_with("unityrs batch: internal: "), "{err}");
        traced(scene, s.live, &trace, "finisher reached");
        let log = fs::read_to_string(&trace)?;
        assert!(log.lines().any(|l| l == "handover"), "{log}");
        fs::write(&go, b"")?;
        let (code, line, _) = ended(scene, s);
        assert_eq!(code, 70, "{line}");
        assert_eq!(line["reason"], "internal");
        let head = head(&dir);
        assert_eq!(head["stop_reason"], "internal");
        assert_eq!(head["phase"], "publish");
        assert_ne!(head["status"], "running");
        Ok(())
    })
    .unwrap();
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir, |_| {});
        let s = start(
            scene,
            &job,
            &[("UNITYRS_TEST_POINT", "finisher=panic")],
            Stdio::null(),
        );
        assert_eq!(scene.wait(s.live)?.code(), Some(70));
        Ok(())
    })
    .unwrap();
}
