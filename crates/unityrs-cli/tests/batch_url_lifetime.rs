//! Process stops retain URL stages until the socket reset at exit.
#![cfg(target_os = "linux")]
mod common;
mod scenes;
#[path = "../../unityrs-batch/tests/url_run_support/mod.rs"]
mod url_run_support;
use scenes::{deadline, ended, head, job, lifecycle, start, traced};
use serde_json::{Value, json};
use std::{
    fs,
    process::Stdio,
    time::{Duration, Instant},
};
use unityrs_testkit::{
    child::{self, Fifo, Live, Scene},
    guard::DEADLOCK_GUARD,
};
use url_run_support::{Close, Origin, Reply};

#[test]
fn sigterm_mid_body_writes_not_staged_rows_and_resets_before_exit() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let origin = held_origin();
        let trace = dir.join("trace");
        let go = dir.join("exit-go");
        let fifo = Fifo::new(scene, "trace.lifecycle")?;
        let path = job(&dir, |spec| {
            spec["inputs"] = json!([{"id":"a", "url":origin.url("/body")}]);
            spec["http"] = json!({"idle_timeout_ms":60_000});
        });
        let point = format!("exit_now=wait:{}", go.display());
        let s = start(
            scene,
            &path,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::null(),
        );
        requests(scene, s.live, &origin, 1);
        received_body(scene, s.live, &dir, "i0");
        scene.signal(s.live, libc::SIGTERM)?;
        lifecycle(scene, s.live, &fifo, "lifecycle_pipe_read reached");
        traced(scene, s.live, &trace, "exit_now reached");
        assert!(scene.running(s.live)?);
        reset(scene, s.live, &origin);
        let head = head(&dir);
        assert_eq!(head["stop_reason"], "sigterm");
        let rows = bundle_rows(&dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "not_staged");
        assert!(rows[0]["size"].is_null());
        assert!(rows[0]["sha256"].is_null());
        fs::write(go, b"")?;
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 6, "{line} {err}");
        assert_eq!(line["reason"], "sigterm");
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_second_signal_resets_active_stages_while_the_finisher_is_held() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let origin = held_origin();
        let trace = dir.join("trace");
        let go = dir.join("exit-go");
        let fifo = Fifo::new(scene, "trace.lifecycle")?;
        let path = job(&dir, |spec| {
            spec["inputs"] = json!([{"id":"a", "url":origin.url("/body")}]);
            spec["http"] = json!({"idle_timeout_ms":60_000});
        });
        let point = format!(
            "finisher=wait:{},exit_now=wait:{}",
            dir.join("never").display(),
            go.display()
        );
        let s = start(
            scene,
            &path,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::null(),
        );
        requests(scene, s.live, &origin, 1);
        received_body(scene, s.live, &dir, "i0");
        scene.signal(s.live, libc::SIGTERM)?;
        lifecycle(scene, s.live, &fifo, "lifecycle_pipe_read reached");
        traced(scene, s.live, &trace, "finisher reached");
        // The first signal freezes the future and retains its connection.
        assert_eq!(origin.closes(), [None]);
        scene.signal(s.live, libc::SIGTERM)?;
        traced(scene, s.live, &trace, "exit_now reached");
        reset(scene, s.live, &origin);
        assert_eq!(head(&dir)["status"], "running");
        fs::write(go, b"")?;
        assert_eq!(scene.wait(s.live)?.code(), Some(6));
        Ok(())
    })
    .unwrap();
}

#[test]
fn sigabrt_resets_active_stages_including_under_the_init_hook() {
    use std::os::unix::process::ExitStatusExt;
    for mode in ["normal", "init-hook", "namespace"] {
        if mode == "namespace" && !namespace_available() {
            continue;
        }
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let origin = held_origin();
            let trace = dir.join("trace");
            let resolve_go = dir.join("resolve-go");
            let socket_go = dir.join("socket-go");
            let abort_go = dir.join("abort-go");
            let _fifo = Fifo::new(scene, "trace.lifecycle")?;
            let path = job(&dir, |spec| {
                spec["inputs"] = json!([
                    {"id":"late", "url":"http://late.invalid/late"},
                    {"id":"active", "url":"http://active.invalid/body"}
                ]);
                spec["download_concurrency"] = json!(2);
                spec["http"] = json!({"idle_timeout_ms":60_000,"connect_timeout_ms":60_000});
            });
            let resolve = format!(
                "late.invalid={}+hold:{},active.invalid={}",
                origin.address,
                resolve_go.display(),
                origin.address
            );
            let point = format!(
                "socket_publish:0=wait:{},abort_handler=wait:{}",
                socket_go.display(),
                abort_go.display()
            );
            let mut env = vec![
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_RESOLVE", resolve.as_str()),
                ("UNITYRS_TEST_POINT", point.as_str()),
            ];
            if mode == "init-hook" {
                env.push(("UNITYRS_TEST_INIT", "1"));
            }
            let s = if mode == "namespace" {
                namespace_start(scene, &path, &env)?
            } else {
                start(scene, &path, &env, Stdio::null())
            };
            requests(scene, s.live, &origin, 1);
            received_body(scene, s.live, &dir, "i1");
            traced(scene, s.live, &trace, "resolve late.invalid held");
            fs::write(resolve_go, b"")?;
            traced(scene, s.live, &trace, "socket_publish:0 reached");
            let pid = if mode == "namespace" {
                namespace_engine(scene, s.live)?
            } else {
                scene.id(s.live)
            };
            signal_pid(pid, libc::SIGABRT)?;
            traced(scene, s.live, &trace, "abort_handler reached");
            reset(scene, s.live, &origin);
            fs::write(socket_go, b"")?;
            traced(scene, s.live, &trace, "socket rejected");
            assert_eq!(origin.total(), 1, "late stage reached origin during abort");
            assert_eq!(head(&dir)["status"], "running");
            fs::write(abort_go, b"")?;
            let status = scene.wait(s.live)?;
            if mode == "normal" {
                assert_eq!(status.signal(), Some(libc::SIGABRT));
            } else {
                assert_eq!(status.code(), Some(134));
            }
            assert_eq!(origin.wait_closed(), [Close::Reset]);
            assert_eq!(head(&dir)["status"], "running");
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn the_deadline_cutoff_blocks_a_retry_after_retry_after_backoff() {
    for cutoff in ["0", "1500"] {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let origin = Origin::start();
            let arm = dir.join("cutoff-arm");
            assert!(child::status(std::process::Command::new("mkfifo").arg(&arm))?.success());
            origin.body("/retry", b"not a bundle".to_vec());
            origin.script("/retry", [Reply::Status(503, "Retry-After: 60\r\n".into())]);
            let path = job(&dir, |spec| {
                spec["inputs"] = json!([{"id":"a", "url":origin.url("/retry")}]);
                spec["deadline"] = json!(deadline(Duration::from_secs(600)));
                spec["shutdown_grace_ms"] = json!(10_000);
                spec["http"] =
                    json!({"attempts":3,"backoff_initial_ms":60_000,"backoff_max_ms":60_000});
            });
            let s = start(
                scene,
                &path,
                &[
                    ("UNITYRS_TEST_CUTOFF_IN", cutoff),
                    ("UNITYRS_TEST_CUTOFF_ARM", arm.to_str().unwrap()),
                ],
                Stdio::null(),
            );
            // A zero delay catches accidental arming during startup deterministically.
            requests(scene, s.live, &origin, 1);
            arm_cutoff(scene, s.live, &arm)?;
            let (code, line, err) = ended(scene, s);
            assert_eq!(code, 6, "{line} {err}");
            assert_eq!(line["reason"], "deadline");
            assert_eq!(origin.requests("/retry"), 1);
            assert_eq!(bundle_rows(&dir)[0]["status"], "not_staged");
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn cutoff_arm_returns_when_the_child_exits_before_the_write() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let origin = held_origin();
        let arm = dir.join("cutoff-arm");
        assert!(child::status(std::process::Command::new("mkfifo").arg(&arm))?.success());
        let path = job(&dir, |spec| {
            spec["inputs"] = json!([{"id":"a", "url":origin.url("/body")}]);
            spec["http"] = json!({"idle_timeout_ms":60_000});
        });
        let s = start(
            scene,
            &path,
            &[
                ("UNITYRS_TEST_CUTOFF_IN", "0"),
                ("UNITYRS_TEST_CUTOFF_ARM", arm.to_str().unwrap()),
            ],
            Stdio::null(),
        );
        requests(scene, s.live, &origin, 1);
        scene.kill(s.live)?;
        scene.wait(s.live)?;
        let error = arm_cutoff(scene, s.live, &arm).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        Ok(())
    })
    .unwrap();
}

fn arm_cutoff(scene: &mut Scene, live: Live, arm: &std::path::Path) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let until = Instant::now() + DEADLOCK_GUARD;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(arm)
        {
            Ok(mut writer) => return writer.write_all(b"x"),
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {}
            Err(error) => return Err(error),
        }
        if !scene.running(live)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "child ended before cutoff FIFO reader opened",
            ));
        }
        if Instant::now() >= until {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "deadlock guard: cutoff FIFO reader never opened",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_socket_published_or_connected_across_the_closing_scan_stays_disconnected() {
    for pause in ["socket_publish", "socket_connect", "socket_connected"] {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let origin = held_origin();
            let trace = dir.join("trace");
            let socket_go = dir.join("socket-go");
            let exit_go = dir.join("exit-go");
            let fifo = Fifo::new(scene, "trace.lifecycle")?;
            let path = job(&dir, |spec| {
                spec["inputs"] = json!([{"id":"late", "url":origin.url("/body")}]);
                spec["http"] = json!({"idle_timeout_ms":60_000});
            });
            let point = format!(
                "{pause}=wait:{},exit_now=wait:{}",
                socket_go.display(),
                exit_go.display()
            );
            let s = start(
                scene,
                &path,
                &[
                    ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                    ("UNITYRS_TEST_POINT", &point),
                ],
                Stdio::null(),
            );
            traced(scene, s.live, &trace, &format!("{pause} reached"));
            // Deliver to the process's main thread, whose second-signal
            // handler runs independently of the paused controller thread.
            main_signal(scene, s.live, libc::SIGTERM)?;
            lifecycle(scene, s.live, &fifo, "lifecycle_pipe_read reached");
            main_signal(scene, s.live, libc::SIGTERM)?;
            traced(scene, s.live, &trace, "exit_now reached");
            fs::write(&socket_go, b"")?;
            // Wait until the resumed path has checked the closing flag
            // and rejected or disconnected the socket, before exit can
            // terminate the controller's in-flight syscall.
            traced(scene, s.live, &trace, "socket rejected");
            assert_eq!(origin.total(), 0, "late request after {pause}");
            fs::write(exit_go, b"")?;
            assert_eq!(scene.wait(s.live)?.code(), Some(6));
            Ok(())
        })
        .unwrap();
    }
}

fn held_origin() -> Origin {
    let origin = Origin::start();
    origin.body("/body", vec![7; 1024 * 1024]);
    origin.script("/body", [Reply::Hold(128)]);
    origin
}
fn requests(scene: &mut Scene, live: Live, origin: &Origin, count: usize) {
    let until = Instant::now() + DEADLOCK_GUARD;
    while origin.total() < count {
        assert!(scene.running(live).unwrap(), "child ended before requests");
        assert!(Instant::now() < until, "request missing");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn received_body(scene: &mut Scene, live: Live, dir: &std::path::Path, file: &str) {
    let until = Instant::now() + DEADLOCK_GUARD;
    let path = dir.join("spool/runs/r/in").join(file);
    while !fs::metadata(&path).is_ok_and(|m| m.len() == 128) {
        assert!(
            scene.running(live).unwrap(),
            "child ended before partial body persisted"
        );
        assert!(
            Instant::now() < until,
            "partial body not persisted: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn reset(scene: &mut Scene, live: Live, origin: &Origin) {
    let until = Instant::now() + DEADLOCK_GUARD;
    loop {
        assert!(
            scene.running(live).unwrap(),
            "exit_now hold did not retain child"
        );
        let closes = origin.closes();
        if closes.iter().all(Option::is_some) {
            assert!(
                closes.iter().all(|c| *c == Some(Close::Reset)),
                "{closes:?}"
            );
            break;
        }
        assert!(Instant::now() < until, "peer never observed reset");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn bundle_rows(dir: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(dir.join("sink/_unityrs/r/1/bundles.0.ndjson"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn main_signal(scene: &mut Scene, live: Live, signal: i32) -> std::io::Result<()> {
    signal_pid(scene.id(live), signal)
}
fn signal_pid(pid: u32, signal: i32) -> std::io::Result<()> {
    // SAFETY: scalar tgkill targets the live child's main thread only.
    if unsafe { libc::syscall(libc::SYS_tgkill, pid, pid, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn namespace_available() -> bool {
    use std::process::Command;
    let result = child::output(Command::new("unshare").args([
        "--user",
        "--map-root-user",
        "--pid",
        "--fork",
        "--mount-proc",
        "true",
    ]));
    match result {
        Ok(output) if output.status.success() => true,
        result => {
            let why = match result {
                Ok(output) => format!(
                    "{}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => error.to_string(),
            };
            assert!(
                std::env::var_os("CI").is_none(),
                "CI requires PID namespace acceptance: {why}"
            );
            eprintln!("namespace scene skipped locally: {why}");
            false
        }
    }
}
fn namespace_start(
    scene: &mut Scene,
    job: &std::path::Path,
    env: &[(&str, &str)],
) -> std::io::Result<scenes::Started> {
    use std::process::Command;
    let mut command = Command::new("unshare");
    command
        .args([
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--mount-proc",
            "--kill-child=KILL",
        ])
        .arg(common::BIN)
        .arg("batch")
        .arg(job)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let live = child::live(scene, &mut command, false)?;
    let out = child::drain(scene.take_stdout(live).unwrap());
    let err = child::drain(scene.take_stderr(live).unwrap());
    Ok(scenes::Started { live, out, err })
}
fn namespace_engine(scene: &mut Scene, live: Live) -> std::io::Result<u32> {
    let wrapper = scene.id(live);
    // Read from the test's host proc mount: unshare's direct child is PID 1
    // inside its namespace, while tgkill needs that child's host PID.
    let children = fs::read_to_string(format!("/proc/{wrapper}/task/{wrapper}/children"))?;
    let children: Vec<u32> = children
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert_eq!(
        children.len(),
        1,
        "namespace wrapper children: {children:?}"
    );
    Ok(children[0])
}
