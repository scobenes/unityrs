//! The process host's stderr lines at their edges (`docs/plans/batch.md`
//! 2.11's *Panics* and *The main thread's `exit` line*): the finisher's
//! own panic line, a drain held in its write against the finisher's
//! exit, the pending queue's overflow line, and a full descriptor table,
//! which is the host's 69 and never the spec's 78, and whose held stdout
//! never holds the exit. Built only with `test-hooks`.
#![cfg(target_os = "linux")]
mod common;
mod held;
#[path = "../../unityrs-batch/tests/run_support/mod.rs"]
mod run_support;
mod scenes;
use scenes::{head, job, start};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use unityrs_testkit::child::{self, Fifo};

fn collect(
    (rx, thread): (
        std::sync::mpsc::Receiver<Vec<u8>>,
        std::thread::JoinHandle<()>,
    ),
) -> String {
    thread.join().unwrap();
    String::from_utf8(rx.try_iter().flatten().collect()).unwrap()
}
/// The `textures:five` golden's spec in the scratch (its one input `x.bundle`).
fn golden(scratch: &unityrs_testkit::scratch::Scratch) -> PathBuf {
    let dir = scratch.path();
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["fixture"] == "textures:five" && c["exit_code"] == 0)
        .unwrap();
    let atlas = run_support::scene(&case);
    for name in ["spool", "sink"] {
        fs::create_dir_all(dir.join(name)).unwrap();
    }
    let (spec, _) = run_support::instantiate(&case, &atlas, scratch);
    let path = dir.join("job.json");
    fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
    path
}

/// A finisher's panic writes its line, after any line the run left.
#[test]
fn a_finisher_panic_writes_its_line_after_the_run_s() {
    let cases = [
        (false, "finisher=panic", ": test hook: finisher\n"),
        (
            true,
            "stage:x.bundle=panic,finisher=panic",
            ": test hook: stage:x.bundle\n",
        ),
    ]
    .into_iter()
    // The controller's own line, drained as the finisher's panic exits:
    // never lost to the finisher's drain (round after round, a race).
    .chain(std::iter::repeat_n(
        (true, "poll=panic,finisher=panic", ": test hook: poll\n"),
        20,
    ));
    for (golden_run, points, first) in cases {
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let job = if golden_run {
                golden(&scene.scratch)
            } else {
                job(&dir, |_| {})
            };
            let trace = dir.join("trace");
            let _fifo = Fifo::new(scene, "trace.lifecycle")?;
            let s = start(
                scene,
                &job,
                &[
                    ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                    ("UNITYRS_TEST_POINT", points),
                ],
                Stdio::null(),
            );
            let status = scene.wait(s.live)?;
            let err = collect(s.err);
            assert_eq!(status.code(), Some(70), "{points}: {err}");
            let lines: Vec<&str> = err.split_inclusive('\n').collect();
            let last = lines.last().copied().unwrap_or_default();
            assert!(
                last.starts_with("unityrs batch: the finisher: internal: ")
                    && last.ends_with(": test hook: finisher\n"),
                "{points}: {err:?}"
            );
            if golden_run {
                assert_eq!(lines.len(), 2, "{err:?}");
                assert!(
                    lines[0].starts_with("unityrs batch: ") && lines[0].ends_with(first),
                    "{err:?}"
                );
            } else {
                assert_eq!(lines.len(), 1, "{err:?}");
            }
            Ok(())
        })
        .unwrap();
    }
}

/// The controller's drain held in its write of a line to a regular file
/// (the `drain` hook, as writeback holds such a write) while the finisher
/// panics: the finisher waits for that drain no longer than a line's bound,
/// counts the line it could not write and reaches its exit, the drain
/// still held; once the write goes on, the drain writes the count after
/// its own line. Every line is written or counted, none out of order.
#[test]
fn a_drain_held_in_its_write_never_holds_the_finisher_s_panic_exit() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = golden(&scene.scratch);
        let trace = dir.join("trace");
        let [go, write, end] = ["go", "write", "end"].map(|name| dir.join(name));
        let err_path = dir.join("err");
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let points = format!(
            "poll=panic,drain=wait:{},finisher=wait:{},finisher=panic,exit_now=wait:{}",
            write.display(),
            go.display(),
            end.display()
        );
        let mut cmd = Command::new(common::BIN);
        cmd.arg("batch")
            .arg(&job)
            .env("UNITYRS_TEST_TRACE", &trace)
            .env("UNITYRS_TEST_POINT", &points)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(fs::File::create(&err_path)?));
        let live = child::live(scene, &mut cmd, false)?;
        let out = child::drain(scene.take_stdout(live).unwrap());
        scenes::traced(scene, live, &trace, "drain reached");
        scenes::traced(scene, live, &trace, "finisher reached");
        let released = Instant::now();
        fs::write(&go, b"")?;
        scenes::traced(scene, live, &trace, "exit_now reached");
        // The bound is a second; a finisher held on the drain would reach
        // its exit only with the lifecycle thread's, at `T_hard`, 19.5 s
        // after the stop.
        let took = released.elapsed();
        assert!(took < Duration::from_secs(10), "{took:?}");
        // The controller's line is still in its write.
        assert_eq!(fs::read_to_string(&err_path)?, "");
        fs::write(&write, b"")?;
        let counted = "unityrs batch: 1 more internal lines dropped\n";
        let until = Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
        while !fs::read_to_string(&err_path)?.ends_with(counted) {
            assert!(scene.running(live)?, "ended before the count line");
            assert!(Instant::now() < until, "no count line");
            std::thread::sleep(Duration::from_millis(1));
        }
        fs::write(&end, b"")?;
        let status = scene.wait(live)?;
        let _ = collect(out);
        assert_eq!(status.code(), Some(70));
        // The controller's line, then the finisher's counted.
        let err = fs::read_to_string(&err_path)?;
        let lines: Vec<&str> = err.split_inclusive('\n').collect();
        assert_eq!(lines.len(), 2, "{err:?}");
        assert!(
            lines[0].starts_with("unityrs batch: internal: ")
                && lines[0].ends_with(": test hook: poll\n"),
            "{err:?}"
        );
        assert_eq!(lines[1], counted, "{err:?}");
        Ok(())
    })
    .unwrap();
}

/// A job of the inputs `ids`, all on `a.bin`, at sixteen threads and
/// sixteen jobs' admission (`max_decoded_bytes` and `dep_cache_bytes`
/// each), so every one's peek is in flight at once.
fn burst_job(dir: &std::path::Path, ids: &[&str]) -> PathBuf {
    job(dir, |v| {
        v["threads"] = json!(16);
        v["download_concurrency"] = json!(16);
        let each =
            v["max_decoded_bytes"].as_u64().unwrap() + v["dep_cache_bytes"].as_u64().unwrap();
        v["max_inflight_uncompressed_bytes"] = json!(16 * each);
        let path = dir.join("a.bin");
        v["inputs"] = ids
            .iter()
            .map(|id| json!({"id": id, "path": path}))
            .collect::<Vec<Value>>()
            .into();
    })
}

/// A known burst past the queue's room: thirteen inputs' peeks held past
/// their stop checks, the lead's panic stopping the run, the controller's
/// drain then held in its write of that line and the finisher before its
/// own, and the twelve others' panics queued meanwhile, each worker's end
/// traced after its line (`end`). The drain then writes the eight lines
/// the ring kept and one line counting the four past its room, which the
/// head's `stderr_dropped` holds.
#[test]
fn panics_past_the_queue_s_room_are_counted_in_one_line() {
    const BURST: usize = 12;
    // The ring's room (2.11).
    let kept = 8;
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let burst: Vec<String> = (0..BURST).map(|i| format!("i{i:02}")).collect();
        let ids: Vec<&str> = std::iter::once("lead")
            .chain(burst.iter().map(String::as_str))
            .collect();
        let job = burst_job(&dir, &ids);
        let trace = dir.join("trace");
        let [lead, go, seen, write, finish] =
            ["lead", "go", "seen", "write", "finish"].map(|name| dir.join(name));
        // `end`'s wait only traces: its path is there from the start.
        fs::write(&seen, b"")?;
        let err_path = dir.join("err");
        let _fifo = Fifo::new(scene, "trace.lifecycle")?;
        let mut points = vec![
            format!("peek:lead=wait:{}", lead.display()),
            "peek:lead=panic".to_owned(),
            format!("drain=wait:{}", write.display()),
            format!("finisher=wait:{}", finish.display()),
        ];
        for id in &burst {
            points.push(format!("peek:{id}=wait:{}", go.display()));
            points.push(format!("peek:{id}=panic"));
            points.push(format!("end:{id}=wait:{}", seen.display()));
        }
        let mut cmd = Command::new(common::BIN);
        cmd.arg("batch")
            .arg(&job)
            .env("UNITYRS_TEST_TRACE", &trace)
            .env("UNITYRS_TEST_POINT", points.join(","))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(fs::File::create(&err_path)?));
        let live = child::live(scene, &mut cmd, false)?;
        let out = child::drain(scene.take_stdout(live).unwrap());
        // Every peek past its last stop check before its panic.
        for id in &ids {
            scenes::traced(scene, live, &trace, &format!("peek:{id} reached"));
        }
        fs::write(&lead, b"")?;
        scenes::traced(scene, live, &trace, "drain reached");
        scenes::traced(scene, live, &trace, "finisher reached");
        fs::write(&go, b"")?;
        for id in &burst {
            scenes::traced(scene, live, &trace, &format!("end:{id} reached"));
        }
        // The burst is queued: the ring's eight lines, and four counted.
        fs::write(&write, b"")?;
        // The drain's lines (the lead's, the kept, the count) before the
        // finisher's own drain may look.
        let want = 1 + kept + 1;
        let until = Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
        while fs::read_to_string(&err_path)?.matches('\n').count() < want {
            assert!(scene.running(live)?, "ended before the drain's lines");
            assert!(Instant::now() < until, "fewer than {want} lines");
            std::thread::sleep(Duration::from_millis(1));
        }
        fs::write(&finish, b"")?;
        let status = scene.wait(live)?;
        let _ = collect(out);
        let err = fs::read_to_string(&err_path)?;
        assert_eq!(status.code(), Some(70), "{err}");
        let lines: Vec<&str> = err.split_inclusive('\n').collect();
        assert_eq!(lines.len(), want, "{err}");
        // A peek's own catch names it (`a peek: `), the hook its input.
        let panicked = |line: &str| {
            let rest = line.strip_prefix("unityrs batch: a peek: internal: ")?;
            let id = rest
                .strip_suffix('\n')?
                .rsplit_once(": test hook: peek:")?
                .1;
            Some(id.to_owned())
        };
        assert_eq!(panicked(lines[0]).as_deref(), Some("lead"), "{err}");
        let mut written: Vec<String> = lines[1..=kept]
            .iter()
            .map(|l| panicked(l).unwrap_or_else(|| panic!("{l:?}: {err}")))
            .collect();
        written.sort_unstable();
        written.dedup();
        assert_eq!(written.len(), kept, "{err}");
        assert!(written.iter().all(|id| burst.contains(id)), "{err}");
        let counted = format!(
            "unityrs batch: {} more internal lines dropped\n",
            BURST - kept
        );
        assert_eq!(lines[kept + 1], counted, "{err}");
        let head = head(&dir);
        let dropped = head["warnings"]["stderr_dropped"].as_u64();
        assert_eq!(dropped, Some((BURST - kept) as u64), "{head}");
        Ok(())
    })
    .unwrap();
}

#[derive(Clone, Copy, Debug)]
enum Stderr {
    File,
    Null,
    Closed,
}

/// Every `RLIMIT_NOFILE` from 3 to 24, stderr a file, `/dev/null` or
/// closed (the runtime's `/dev/null` in its place): a 69 with its `exit`
/// line and its stderr line, never the spec's 78, and `JOB.json`'s open at
/// a full table `startup_failed open EMFILE`.
#[test]
fn a_full_descriptor_table_is_a_69_never_a_78() {
    for mode in [Stderr::File, Stderr::Null, Stderr::Closed] {
        for limit in 3..=24_u64 {
            child::scene(|scene| {
                let dir = scene.scratch.path().to_owned();
                let job = job(&dir, |_| {});
                let err_path = dir.join("err");
                let mut cmd = Command::new(common::BIN);
                cmd.arg("batch")
                    .arg(&job)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(match mode {
                        Stderr::File => Stdio::from(fs::File::create(&err_path)?),
                        Stderr::Null | Stderr::Closed => Stdio::null(),
                    });
                let close = matches!(mode, Stderr::Closed);
                // SAFETY: close and setrlimit are async-signal-safe.
                unsafe {
                    cmd.pre_exec(move || {
                        if close {
                            libc::close(2);
                        }
                        let fds = libc::rlimit {
                            rlim_cur: limit,
                            rlim_max: limit,
                        };
                        if libc::setrlimit(libc::RLIMIT_NOFILE, &raw const fds) == 0 {
                            Ok(())
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    });
                }
                let live = child::live(scene, &mut cmd, false)?;
                let out = child::drain(scene.take_stdout(live).unwrap());
                let code = scene.wait(live)?.code().unwrap_or(-1);
                let out = collect(out);
                let err = fs::read_to_string(&err_path).unwrap_or_default();
                let at = format!("{mode:?} {limit}: {code} {out:?} {err:?}");
                // At 3 the dynamic loader finds no descriptor for a library
                // (127, its line on stderr): no engine ran.
                if limit == 3 && code == 127 && out.is_empty() {
                    return Ok(());
                }
                // Every limit here is below the 96 step 6 asks for.
                assert_eq!(code, 69, "{at}");
                let exit: Value =
                    serde_json::from_str(out.lines().last().unwrap_or_else(|| panic!("{at}")))
                        .unwrap();
                assert_eq!(exit["type"], "exit", "{at}");
                assert_eq!(exit["code"], code, "{at}");
                // Closed, stderr takes the runtime's `/dev/null`, leaving
                // `JOB.json` no descriptor at 3.
                if limit == 3 && matches!(mode, Stderr::Closed) {
                    assert_eq!(exit["reason"], "startup_failed", "{at}");
                    assert_eq!(exit["detail"], "open EMFILE", "{at}");
                }
                if matches!(mode, Stderr::File) {
                    assert!(err.starts_with("unityrs batch: "), "{at}");
                    assert!(err.ends_with('\n'), "{at}");
                }
                Ok(())
            })
            .unwrap();
        }
    }
}

/// A stop while the golden's one input's attempt is held: the finisher's lines
/// count the dispatched, uncommitted input in `running`, its `exit` line
/// in `not_run`, and every line partitions `total`.
#[test]
fn the_finisher_s_lines_keep_a_dispatched_input_running_until_the_exit_line() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let trace = dir.join("trace");
        let go = dir.join("go");
        let fifo = Fifo::new(scene, "trace.lifecycle")?;
        let job = golden(&scene.scratch);
        let point = format!("attempt:x.bundle=wait:{}", go.display());
        let s = start(
            scene,
            &job,
            &[
                ("UNITYRS_TEST_TRACE", trace.to_str().unwrap()),
                ("UNITYRS_TEST_POINT", &point),
            ],
            Stdio::null(),
        );
        scenes::traced(scene, s.live, &trace, "attempt:x.bundle reached");
        scene.signal(s.live, libc::SIGTERM)?;
        scenes::lifecycle(scene, s.live, &fifo, "lifecycle_stop_woken reached");
        fs::write(&go, b"")?;
        let status = scene.wait(s.live)?;
        let out = collect(s.out);
        let err = collect(s.err);
        assert_eq!(status.code(), Some(6), "{err}");
        let lines: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let sum = |i: &Value| {
            ["complete", "failed", "running", "queued", "not_run"]
                .iter()
                .map(|k| i[k].as_u64().unwrap())
                .sum::<u64>()
        };
        for line in &lines {
            assert_eq!(
                sum(&line["inputs"]),
                line["inputs"]["total"].as_u64().unwrap(),
                "{line}"
            );
        }
        let (exit, finish) = lines.split_last().unwrap();
        assert_eq!(exit["type"], "exit");
        assert_eq!(exit["inputs"]["running"], 0, "{exit}");
        let finishing: Vec<&Value> = finish.iter().filter(|l| l["phase"] == "finish").collect();
        assert!(!finishing.is_empty(), "{out}");
        for line in finishing {
            assert!(line["inputs"]["running"].as_u64().unwrap() >= 1, "{line}");
            assert_eq!(line["inputs"]["queued"], 0, "{line}");
        }
        Ok(())
    })
    .unwrap();
}

/// `--check` at a full descriptor table keeps 78 `bad_spec unreadable`:
/// the 69 is a run's alone.
#[test]
fn check_at_a_full_descriptor_table_is_78_unreadable() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir, |_| {});
        let mut cmd = Command::new(common::BIN);
        cmd.arg("batch")
            .arg("--check")
            .arg(&job)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // SAFETY: close and setrlimit are async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::close(2);
                let fds = libc::rlimit {
                    rlim_cur: 3,
                    rlim_max: 3,
                };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &raw const fds) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let live = child::live(scene, &mut cmd, false)?;
        let out = child::drain(scene.take_stdout(live).unwrap());
        let code = scene.wait(live)?.code();
        let out = collect(out);
        assert_eq!(code, Some(78), "{out}");
        let exit: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
        assert_eq!(exit["reason"], "bad_spec", "{out}");
        assert_eq!(exit["detail"], "unreadable", "{out}");
        Ok(())
    })
    .unwrap();
}

/// A child's `pre_exec` work for a full table: stderr closed, and
/// `RLIMIT_NOFILE` at `limit`, async-signal-safe calls alone.
fn full_table(limit: u64) -> impl Fn() -> std::io::Result<()> + Send + Sync + 'static {
    move || {
        // SAFETY: close and setrlimit are async-signal-safe.
        unsafe {
            libc::close(2);
            let fds = libc::rlimit {
                rlim_cur: limit,
                rlim_max: limit,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &raw const fds) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
    }
}

/// A full descriptor table, at a deadline, with stdout held: a pipe
/// another writer keeps full, or one that writer fills at the child's
/// every write to fd 1, after its `poll` found room (`held`). With a
/// deadline the main thread's two lines take a description each, so up to
/// 4, a spare short, both go on the borrowed descriptors, and the write
/// the filler holds is the borrowed writer's, blocking every time: it
/// never holds the process past the lines' budget. At 4 that write must
/// have been let run, or the scene proved nothing; where the filler cannot
/// be had (no seccomp user notification), it fails, saying so.
#[test]
fn a_held_stdout_at_a_full_table_never_holds_the_exit() {
    use held::{AtWrite, Held, Pipe};
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    /// Ends the other writer's thread however the scene ends.
    struct Stop<'a>(&'a AtomicBool);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            self.0.store(true, SeqCst);
        }
    }
    for held in [Held::AtWrite, Held::Contended] {
        for limit in 3..=6_u64 {
            child::scene(|scene| {
                let dir = scene.scratch.path().to_owned();
                let job = job(&dir, |v| {
                    v["deadline"] = json!(scenes::deadline(Duration::from_secs(600)));
                });
                let mut pipe = Pipe::new()?;
                let at_write = match held {
                    Held::AtWrite => Some(
                        AtWrite::new()
                            .unwrap_or_else(|e| panic!("{held:?} {limit}: no seccomp filter: {e}")),
                    ),
                    Held::Contended => {
                        pipe.fill();
                        None
                    }
                };
                let mut cmd = Command::new(common::BIN);
                cmd.arg("batch")
                    .arg(&job)
                    .stdin(Stdio::null())
                    .stdout(held::stdout(&pipe)?)
                    .stderr(Stdio::null());
                let then = full_table(limit);
                match &at_write {
                    Some(at_write) => at_write.arm(&mut cmd, then),
                    // SAFETY: as `then` says.
                    None => unsafe {
                        cmd.pre_exec(then);
                    },
                }
                let stop = AtomicBool::new(false);
                std::thread::scope(|scope| {
                    let _stop = Stop(&stop);
                    let started = Instant::now();
                    let live = child::live(scene, &mut cmd, false).unwrap_or_else(|e| {
                        panic!(
                            "{held:?} {limit}: the child's start failed ({e}): with \
                             AtWrite, its seccomp filter needs Linux 5.5+ with seccomp \
                             user notification allowed"
                        )
                    });
                    let served = if let Some(at_write) = &at_write {
                        let listener = at_write.listener()?;
                        let (pipe, stop) = (&pipe, &stop);
                        Some(scope.spawn(move || held::serve(&listener, pipe, stop)))
                    } else {
                        let (pipe, stop) = (&mut pipe, &stop);
                        scope.spawn(move || pipe.contend(stop));
                        None
                    };
                    // A held process fails here, not at the scene's guard.
                    while scene.running(live)? && started.elapsed() < Duration::from_secs(5) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    let took = started.elapsed();
                    assert!(
                        !scene.running(live)?,
                        "{held:?} {limit}: held past {took:?}"
                    );
                    let code = scene.wait(live)?.code();
                    assert!(
                        code == Some(69) || (limit == 3 && code == Some(127)),
                        "{held:?} {limit}: {code:?}"
                    );
                    stop.store(true, SeqCst);
                    if let Some(served) = served {
                        let served = served
                            .join()
                            .unwrap()
                            .unwrap_or_else(|e| panic!("{held:?} {limit}: the filler failed: {e}"));
                        // Up to 4, a spare short, the borrowed writer's
                        // write is the one held, let run against a full
                        // pipe: the scene's point, so it must have run.
                        // Past 4 the lines take descriptions of their own,
                        // and nothing writes fd 1.
                        if limit <= 4 && code == Some(69) {
                            assert!(served > 0, "{held:?} {limit}: no write held");
                        } else {
                            assert_eq!(served, 0, "{held:?} {limit}");
                        }
                    }
                    Ok::<_, std::io::Error>(())
                })
            })
            .unwrap();
        }
    }
}
