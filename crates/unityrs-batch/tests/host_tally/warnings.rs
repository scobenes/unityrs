//! Real startup warnings against caller-owned pipes with backpressure.
use super::*;
use std::io::{ErrorKind, Read};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;
use unityrs_testkit::guard::DEADLOCK_GUARD;

struct Retry {
    clock: Instant,
    control: fs::File,
    release: std::path::PathBuf,
    drains: bool,
    attempts: usize,
}
thread_local! {
    static RETRY: std::cell::RefCell<Option<Retry>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn retry_hooks() {
    let Some(control) = std::env::var_os("B9_RETRY_CONTROL") else {
        return;
    };
    RETRY.with_borrow_mut(|slot| {
        *slot = Some(Retry {
            clock: Instant::now(),
            control: fs::OpenOptions::new().write(true).open(control).unwrap(),
            release: std::env::var_os("B9_RETRY_RELEASE").unwrap().into(),
            drains: std::env::var_os("B9_RETRY_DRAIN").is_some(),
            attempts: 0,
        });
    });
    unityrs_batch::sys::stderr_retry_hooks(Some(unityrs_batch::sys::StderrRetryHooks {
        write: retry_write,
        now: || RETRY.with_borrow(|slot| slot.as_ref().unwrap().clock),
        sleep: |pause| {
            RETRY.with_borrow_mut(|slot| {
                let retry = slot.as_mut().unwrap();
                // Advance only virtual time: load cannot consume this budget.
                retry.clock += if retry.drains {
                    pause
                } else {
                    unityrs_batch::progress::early::OWN_WAIT
                };
            });
        },
        finished: || {
            RETRY.with_borrow_mut(|slot| {
                let retry = slot.as_mut().unwrap();
                writeln!(retry.control, "finished {}", retry.attempts).unwrap();
            });
        },
    }));
}

fn retry_write(bytes: &[u8]) -> std::io::Result<usize> {
    RETRY.with_borrow_mut(|slot| {
        let retry = slot.as_mut().unwrap();
        retry.attempts += 1;
        let result = std::io::stderr().write(bytes);
        if retry.attempts == 1 {
            assert_eq!(result.as_ref().unwrap_err().kind(), ErrorKind::WouldBlock);
            writeln!(retry.control, "blocked").unwrap();
            let until = Instant::now() + DEADLOCK_GUARD;
            while !retry.release.exists() {
                assert!(Instant::now() < until, "warning retry was never released");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        result
    })
}

fn full_pipe() -> (std::io::PipeReader, std::fs::File) {
    let (r, w) = std::io::pipe().unwrap();
    let w = std::fs::File::from(OwnedFd::from(w));
    fill(&w);
    (r, w)
}

pub(super) fn fill_stdout() {
    use std::os::fd::AsFd;
    fill(&fs::File::from(
        std::io::stdout().as_fd().try_clone_to_owned().unwrap(),
    ));
}

fn fill(w: &fs::File) {
    let mut fill = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}", w.as_raw_fd()))
        .unwrap();
    for bytes in [&[b'x'; 4096][..], b"x"] {
        loop {
            match fill.write(bytes) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
    }
}

#[test]
fn a_progress_spawn_failure_warns_even_when_stdout_stays_full() {
    progress_spawn_failure(false);
}

#[test]
fn a_full_warning_pipe_does_not_add_another_output_budget() {
    progress_spawn_failure(true);
}

fn progress_spawn_failure(stderr_full: bool) {
    child::scene(|scene| {
        let dir = scene.scratch.path();
        let job = job(dir);
        let log = dir.join("tally");
        fs::write(&log, b"")?;
        let (_out, full) = std::io::pipe()?;
        let (err, stderr) = if stderr_full {
            let (r, w) = full_pipe();
            (Some(r), Stdio::from(w))
        } else {
            (None, Stdio::piped())
        };
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["--exact", "tally_child", "--nocapture", "--test-threads=1"])
            .env("B9_JOB", job)
            .env("B9_TALLY", log)
            .env("UNITYRS_TEST_SPAWN", "progress")
            .env("B9_FILL_STDOUT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::from(full))
            .stderr(stderr);
        let started = Instant::now();
        let engine = child::live(scene, &mut cmd, false)?;
        drop(cmd);
        let reader = scene.take_stderr(engine).map(child::drain);
        let status = scene.wait(engine)?;
        assert_eq!(status.code(), Some(69));
        let elapsed = started.elapsed();
        let budget = unityrs_batch::progress::early::OWN_WAIT;
        // One production output budget plus process startup/teardown slack,
        // below the two seconds independent full-stream budgets would take.
        assert!(elapsed < budget + budget * 9 / 10, "{elapsed:?}");
        if let Some((rx, reader)) = reader {
            reader.join().unwrap();
            let err = String::from_utf8(rx.try_iter().flatten().collect()).unwrap();
            assert!(err.contains("tally note\n"), "{err:?}");
        }
        drop(err);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_successful_start_retries_a_warning_until_stderr_drains() {
    successful_warning(true, true);
}

#[test]
fn a_successful_start_counts_one_drop_when_stderr_never_drains() {
    successful_warning(false, true);
}

#[test]
fn a_fast_exit_counts_the_warning_still_waiting_on_stderr() {
    successful_warning(false, false);
}

fn successful_warning(drains: bool, hold: bool) {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir);
        let log = dir.join("tally");
        let trace = dir.join("trace");
        let _lifecycle = child::Fifo::new(scene, "trace.lifecycle")?;
        let release = dir.join("release");
        let retry_release = dir.join("retry.release");
        let control = child::Fifo::new(scene, "retry.control")?;
        fs::write(&log, b"")?;
        let (mut err, full) = full_pipe();
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["--exact", "tally_child", "--nocapture", "--test-threads=1"])
            .env("B9_JOB", job)
            .env("B9_TALLY", &log)
            .env("UNITYRS_TEST_TRACE", &trace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(full));
        if hold {
            cmd.env("B9_RETRY_CONTROL", control.path())
                .env("B9_RETRY_RELEASE", &retry_release)
                .env(
                    "UNITYRS_TEST_POINT",
                    format!("finisher=wait:{}", release.display()),
                );
            if drains {
                cmd.env("B9_RETRY_DRAIN", "1");
            }
        }
        let engine = child::live(scene, &mut cmd, false)?;
        drop(cmd);
        let stdin = scene.take_stdin(engine);
        let (out, reader) = child::drain(scene.take_stdout(engine).unwrap());
        if hold {
            assert_eq!(control.line_while(scene, engine)?, "blocked");
        }
        let until = Instant::now() + DEADLOCK_GUARD;
        while hold
            && !fs::read_to_string(&trace)
                .unwrap_or_default()
                .contains("finisher reached")
        {
            assert!(
                scene.running(engine)?,
                "child ended before the finisher hold"
            );
            assert!(Instant::now() < until, "finisher never reached its hold");
            std::thread::sleep(Duration::from_millis(1));
        }
        // Free a known amount before releasing the failed write. The frozen
        // retry clock makes the finisher hand-over independent of host load.
        if drains {
            err.read_exact(&mut [0; 4096])?;
        }
        let err_reader = drains.then(|| child::drain(err.try_clone().unwrap()));
        if hold {
            fs::write(&retry_release, b"")?;
            assert_eq!(
                control.line_while(scene, engine)?,
                if drains { "finished 2" } else { "finished 1" },
            );
        }
        fs::write(&release, b"")?;
        let status = scene.wait(engine)?;
        drop(stdin);
        reader.join().unwrap();
        if let Some((rx, thread)) = err_reader {
            thread.join().unwrap();
            let got: Vec<u8> = rx.try_iter().flatten().collect();
            assert!(
                got.windows(11).any(|b| b == b"tally note\n"),
                "warning was dropped"
            );
        } else {
            let mut got = Vec::new();
            err.read_to_end(&mut got)?;
            assert!(!got.windows(11).any(|b| b == b"tally note\n"));
        }
        assert_eq!(status.code(), Some(4));
        let log = fs::read_to_string(log)?;
        assert!(!log.contains("register exit-writer"), "{log}");
        let out = String::from_utf8(out.try_iter().flatten().collect()).unwrap();
        let head: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("sink/_unityrs/r/manifest.json"))?)?;
        assert_eq!(
            head["warnings"]["stderr_dropped"],
            u64::from(!drains),
            "{out}\n{head}"
        );
        Ok(())
    })
    .unwrap();
}
