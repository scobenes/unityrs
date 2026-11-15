//! The process host's edges through the binary (`docs/plans/batch.md`
//! 2.11): every thread's panic and its stderr line, the codes a panic keeps,
//! `exit_now`'s hold (a full descriptor table's in `batch_host_lines.rs`), SIGABRT
//! as init, inherited signal state, a group's signal before the parent's
//! death, and the `files` and `host_root` hooks. Built only with
//! `test-hooks`.
#![cfg(target_os = "linux")]
mod common;
#[path = "../../unityrs-batch/tests/run_support/mod.rs"]
mod run_support;
mod scenes;
use scenes::{Started, ended, head, held, job, start, start_with, traced};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::process::{CommandExt, ExitStatusExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};
use unityrs_batch::reason::Code;
use unityrs_testkit::child::{self, Fifo, Scene};

/// A run of the `textures:five` golden in `scene`, which publishes: its
/// spec's path.
fn golden(scene: &Scene) -> PathBuf {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["fixture"] == "textures:five" && c["exit_code"] == 0)
        .unwrap();
    let atlas = run_support::scene(&case);
    for name in ["spool", "sink"] {
        fs::create_dir_all(scene.scratch.join(name)).unwrap();
    }
    let (spec, _) = run_support::instantiate(&case, &atlas, &scene.scratch);
    let path = scene.scratch.join("job.json");
    fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
    path
}
/// One panic: the code it ends with and its stderr line, `who` naming the
/// catch (empty for the controller's own and the main thread's).
fn panics(job: impl Fn(&Path) -> PathBuf, point: &str, who: &str, stdin: bool) {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let trace = dir.join("trace");
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir);
        let armed = format!("{point}=panic");
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &armed),
            ],
            if stdin { Stdio::piped() } else { Stdio::null() },
        );
        let status = scene.wait(s.live)?;
        let err = collect(s.err);
        assert_eq!(status.code(), Some(70), "{point}: {err}");
        let line = format!("unityrs batch: {who}internal: ");
        let ending = format!(": test hook: {point}\n");
        assert!(
            err.starts_with(&line) && err.ends_with(&ending),
            "{point}: {err:?}"
        );
        assert_eq!(err.lines().count(), 1, "{point}: {err:?}");
        Ok(())
    })
    .unwrap();
}
fn collect(
    (rx, thread): (
        std::sync::mpsc::Receiver<Vec<u8>>,
        std::thread::JoinHandle<()>,
    ),
) -> String {
    thread.join().unwrap();
    String::from_utf8(rx.try_iter().flatten().collect()).unwrap()
}

#[test]
fn every_caught_panic_writes_its_line_and_exits_70() {
    let plain = |dir: &Path| job(dir, |_| {});
    for (point, who) in [
        ("poll", ""),
        ("commit:a", ""),
        ("stage:a", "a local copy: "),
        ("peek:a", "a peek: "),
        ("main", ""),
        ("progress", "the progress thread: "),
        ("lifecycle", "the lifecycle thread: "),
    ] {
        panics(plain, point, who, false);
    }
    let watched = |dir: &Path| job(dir, |v| v["stop_on_stdin_eof"] = json!(true));
    panics(watched, "stdin", "the stdin watch: ", true);
    for (point, who) in [
        ("attempt:x.bundle", "an attempt: "),
        ("transfer", "a directory transfer: "),
        ("drained", "publication: "),
    ] {
        child::scene(|scene| {
            let job = golden(scene);
            let trace = scene.scratch.join("trace");
            let _fifo = Fifo::new(scene, "trace.lifecycle")?;
            let armed = format!("{point}=panic");
            let s = start(
                scene,
                &job,
                &[
                    ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                    ("UNITYRS_TEST_POINT", &armed),
                ],
                Stdio::null(),
            );
            let status = scene.wait(s.live)?;
            let err = collect(s.err);
            assert_eq!(status.code(), Some(70), "{point}: {err}");
            let line = format!("unityrs batch: {who}internal: ");
            assert!(err.starts_with(&line), "{point}: {err:?}");
            assert!(err.ends_with(&format!(": test hook: {point}\n")), "{err:?}");
            assert_eq!(err.lines().count(), 1, "{point}: {err:?}");
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn a_finisher_panic_keeps_a_worse_code_it_received() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let plan = dir.join("plan");
        // `statvfs`'s figure below the spool's reserve.
        fs::write(&plan, "spool.available * figure 0\n")?;
        let job = job(&dir, |v| v["spool"]["reserve_bytes"] = json!(1));
        let s = start(
            scene,
            &job,
            &[("UNITYRS_TEST_FILES", plan.to_str().unwrap())],
            Stdio::null(),
        );
        let (code, line, err) = ended(scene, s);
        assert_eq!(
            (code, &line["reason"]),
            (69, &json!("spool_no_space")),
            "{err}"
        );
        let calls = fs::read_to_string(dir.join("plan.log"))?;
        assert!(calls.lines().any(|l| l == "spool.available"), "{calls}");
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_FILES", plan.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", "finisher=panic"),
            ],
            Stdio::null(),
        );
        let want = Code::Internal.worst(Code::CouldNotStart).value();
        assert_eq!(scene.wait(s.live)?.code(), Some(i32::from(want)));
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_host_root_hook_answers_step_6() {
    for (files, reason) in [
        (&[][..], "procfs_unreadable"),
        (
            &[("proc/sys/vm/overcommit_memory", "2\n")][..],
            "overcommit_strict",
        ),
    ] {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let root = dir.join("root");
            fs::create_dir(&root)?;
            for (path, text) in files {
                let path = root.join(path);
                fs::create_dir_all(path.parent().unwrap())?;
                fs::write(path, text)?;
            }
            let job = job(&dir, |_| {});
            let s = start(
                scene,
                &job,
                &[("UNITYRS_TEST_HOST_ROOT", root.to_str().unwrap())],
                Stdio::null(),
            );
            let (code, line, err) = ended(scene, s);
            assert_eq!((code, &line["reason"]), (69, &json!(reason)), "{err}");
            assert!(
                err.starts_with(&format!("unityrs batch: {reason}")),
                "{err}"
            );
            assert!(!dir.join("sink/_unityrs").exists());
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn exit_now_holds_after_the_exit_line_until_released() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, go, _) = held(&dir);
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir, |_| {});
        let point = format!("exit_now=wait:{}", go.display());
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::null(),
        );
        traced(scene, s.live, &trace, "exit_now reached");
        // Held: the exit line is out, the process not yet gone.
        let mut out = Vec::new();
        let until = Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
        while !String::from_utf8_lossy(&out).contains(r#""type":"exit""#) {
            assert!(Instant::now() < until, "no exit line");
            if let Ok(chunk) = s.out.0.recv_timeout(std::time::Duration::from_millis(10)) {
                out.extend(chunk);
            }
        }
        assert!(scene.running(s.live)?);
        fs::write(&go, b"")?;
        let status = scene.wait(s.live)?;
        let text = String::from_utf8(out).unwrap();
        let line: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(
            status.code(),
            line["code"].as_i64().and_then(|c| i32::try_from(c).ok())
        );
        Ok(())
    })
    .unwrap();
}

/// A run held at the controller's wait, then `signal`; the status.
fn signalled(
    env: &[(&str, &str)],
    signal: i32,
    edit: impl Fn(&mut Command) + Copy,
) -> (std::process::ExitStatus, Option<Value>) {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, go, point) = held(&dir);
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir, |_| {});
        let mut vars = vec![
            ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
            ("UNITYRS_TEST_POINT", point.as_str()),
        ];
        vars.extend_from_slice(env);
        let s: Started = start_with(scene, &job, &vars, Stdio::null(), edit);
        traced(scene, s.live, &trace, "poll reached");
        scene.signal(s.live, signal)?;
        fs::write(&go, b"")?;
        let status = scene.wait(s.live)?;
        let head = dir
            .join("sink/_unityrs/r/manifest.json")
            .exists()
            .then(|| head(&dir));
        Ok((status, head))
    })
    .unwrap()
}
fn no_core(cmd: &mut Command) {
    // SAFETY: setrlimit is async-signal-safe; a fixed value.
    unsafe {
        cmd.pre_exec(|| {
            let none = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            libc::setrlimit(libc::RLIMIT_CORE, &raw const none);
            Ok(())
        });
    }
}

#[test]
fn sigabrt_exits_134_as_init_and_aborts_otherwise() {
    let (status, _) = signalled(&[("UNITYRS_TEST_INIT", "1")], libc::SIGABRT, no_core);
    assert_eq!(status.code(), Some(134), "{status:?}");
    let (status, _) = signalled(&[], libc::SIGABRT, no_core);
    assert_eq!(status.signal(), Some(libc::SIGABRT), "{status:?}");
}

/// An ignore of SIGTERM, SIGINT and SIGHUP and a mask blocking them,
/// inherited at exec (`docs/BATCH_SPEC.md` 9.2).
fn inherited(ignored: bool) -> impl Fn(&mut Command) + Copy {
    move |cmd: &mut Command| {
        // SAFETY: signal and sigprocmask are async-signal-safe; stack sets.
        unsafe {
            cmd.pre_exec(move || {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&raw mut set);
                for n in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                    if ignored {
                        libc::signal(n, libc::SIG_IGN);
                    }
                    libc::sigaddset(&raw mut set, n);
                }
                libc::sigprocmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
                Ok(())
            });
        }
    }
}

#[test]
fn an_inherited_mask_never_hides_a_stop_and_only_sigterm_s_ignore_is_reset() {
    // Blocked, never ignored: every one stops the run.
    for (signal, name) in [
        (libc::SIGTERM, "sigterm"),
        (libc::SIGINT, "sigint"),
        (libc::SIGHUP, "sighup"),
    ] {
        let (status, head) = signalled(&[], signal, inherited(false));
        assert_eq!(status.code(), Some(6), "{name} {status:?}");
        assert_eq!(head.unwrap()["stop_reason"], name);
    }
    // Ignored as well: SIGTERM's ignore is reset, SIGINT's and SIGHUP's
    // kept (a `nohup`'s), so the run goes on to its own end.
    let (status, head) = signalled(&[], libc::SIGTERM, inherited(true));
    assert_eq!(status.code(), Some(6), "{status:?}");
    assert_eq!(head.unwrap()["stop_reason"], "sigterm");
    for signal in [libc::SIGINT, libc::SIGHUP] {
        let (status, head) = signalled(&[], signal, inherited(true));
        assert_eq!(status.code(), Some(4), "{signal} {status:?}");
        assert!(head.unwrap()["stop_reason"].is_null());
    }
}

#[test]
fn the_parent_s_death_after_a_group_sigterm_is_no_second_signal() {
    // The engine, its parent gone, becomes this process's child to reap.
    unityrs_batch::sys::testing::subreaper().unwrap();
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let (trace, go, point) = held(&dir);
        let fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = job(&dir, |_| {});
        // The shell ignores SIGTERM; the engine resets that ignore.
        let mut cmd = Command::new("sh");
        // It ends, the engine's parent with it, once `$2` exists.
        let script = r#"trap "" TERM; "$0" batch "$1" & echo "$!"
            until [ -e "$2" ]; do sleep 0.01; done"#;
        let end = dir.join("end");
        cmd.args(["-c", script, common::BIN])
            .arg(&job)
            .arg(&end)
            .env("UNITYRS_TEST_TRACE", &trace)
            .env("UNITYRS_TEST_POINT", &point)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let parent = child::live(scene, &mut cmd, false)?;
        let mut out = std::io::BufReader::new(scene.take_stdout(parent).unwrap());
        let mut pid = String::new();
        std::io::BufRead::read_line(&mut out, &mut pid)?;
        let pid: u32 = pid.trim().parse().unwrap();
        let drained = child::drain(out);
        traced(scene, parent, &trace, "poll reached");
        // The group's SIGTERM (the scene signals a child alone), its stop
        // seen, then the parent's death.
        let group = i32::try_from(scene.id(parent)).unwrap();
        // SAFETY: the group of a child of this scene's, not yet reaped.
        assert_eq!(unsafe { libc::kill(-group, libc::SIGTERM) }, 0);
        scenes::lifecycle(scene, parent, &fifo, "lifecycle_stop_woken reached");
        fs::write(&end, b"")?;
        // The shell is reaped last: its reap kills what is left of its group.
        let until = Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
        let engine = loop {
            match scene.adopt(pid, false) {
                Ok(engine) => break engine,
                Err(e) => assert!(Instant::now() < until, "not adopted: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        fs::write(&go, b"")?;
        let status = scene.wait(engine)?;
        let text = collect(drained);
        assert_eq!(status.code(), Some(6), "{status:?} {text}");
        let line: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(line["reason"], "sigterm");
        let head = head(&dir);
        assert_eq!(head["stop_reason"], "sigterm");
        assert_ne!(head["status"], "running");
        assert!(scene.wait(parent)?.success());
        Ok(())
    })
    .unwrap();
}

#[test]
fn final_counts_follow_the_finisher_s_reconcile() {
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
                ("UNITYRS_TEST_POINT", "renamed:a=panic"),
            ],
            Stdio::null(),
        );
        let (code, line, err) = ended(scene, s);
        assert_eq!(code, 70, "{line} {err}");
        let log = fs::read_to_string(&trace)?;
        assert!(log.lines().any(|l| l == "commit found at take: a"), "{log}");
        let inputs = &line["inputs"];
        assert_eq!(inputs["running"], 0);
        assert_eq!(inputs["queued"], 0);
        // `a`'s commit, found at the take, is counted; `b` never ran.
        assert_eq!(
            inputs["failed"].as_u64().unwrap() + inputs["complete"].as_u64().unwrap(),
            1
        );
        assert_eq!(inputs["not_run"], 1);
        Ok(())
    })
    .unwrap();
}
