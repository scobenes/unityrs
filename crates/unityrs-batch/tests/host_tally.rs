//! The `tally` hook through the process host (`docs/plans/batch.md` 2.11's
//! *Test hooks*): a library-API child registers every engine thread, the
//! pools' and the host's own, the main thread and the early lines' writer
//! thread among them, and the lifecycle thread and the stdin watch mark it
//! before their first `poll`. Built only with `test-hooks`.
#![cfg(target_os = "linux")]
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use unityrs_batch::{phase1, progress::clock::Clock, run::hooks::Tally, run::process, spec};
use unityrs_testkit::child;

/// Appends `<what> <thread name>` to the child's log.
fn note(what: &str) {
    let path = std::env::var_os("B9_TALLY").unwrap();
    let name = std::thread::current().name().unwrap_or("").to_owned();
    let mut log = fs::OpenOptions::new().append(true).open(path).unwrap();
    let _ = log.write_all(format!("{what} {name}\n").as_bytes());
}
fn register() {
    // The main thread's registration panicking: before the host exists.
    if std::env::var_os("B9_PANIC_EARLY").is_some()
        && std::thread::current().name() == Some("tally_child")
    {
        panic!("early registration");
    }
    note("register");
}
fn mark() {
    note("mark");
}
fn job(dir: &Path) -> std::path::PathBuf {
    for name in ["spool", "sink"] {
        fs::create_dir_all(dir.join(name)).unwrap();
    }
    fs::write(dir.join("a.bin"), b"not a bundle").unwrap();
    // An hour ahead: the note's early line goes through the writer thread.
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3_600;
    let at = unityrs_batch::sys::utc(i64::try_from(at).unwrap()).unwrap();
    let at = String::from_utf8(at.to_vec()).unwrap();
    let text = format!(
        r#"{{"spec_version":1,"run_id":"r","attempt":1,"threads":2,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":268435456,"max_decoded_bytes":16777216,
        "dep_cache_bytes":33554432,"max_view_bundles":4,"stop_on_stdin_eof":true,
        "deadline":"{at}",
        "spool":{{"dir":"{}","size_bytes":1073741824,"reserve_bytes":0}},
        "outputs":{{"textures":{{"key":"{{input}}/{{name}}.png"}}}},
        "sink":{{"kind":"directory","root":"{}"}},
        "inputs":[{{"id":"a","path":"{}"}}]}}"#,
        dir.join("spool").display(),
        dir.join("sink").display(),
        dir.join("a.bin").display(),
    );
    let path = dir.join("job.json");
    fs::write(&path, text).unwrap();
    path
}

/// The child: the binary's run through the library API, with the tally.
#[test]
fn tally_child() {
    let Some(job) = std::env::var_os("B9_JOB") else {
        return;
    };
    let clock = Clock::process(Instant::now());
    let mut startup = process::Startup::prepare().unwrap();
    startup.hooks().tally = Some(Tally { register, mark });
    startup.hooks().skip_host_checks = true;
    let bytes = spec::read::read_job(Path::new(&job)).unwrap();
    let prepared = phase1::prepare(spec::parse(bytes).unwrap()).unwrap();
    // A note, as the uncertified page size's, written at the start.
    process::run(startup, clock, prepared, 1 << 30, Some("tally note\n"))
}

#[test]
fn every_engine_thread_registers_and_the_two_watches_mark() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir);
        let log = dir.join("tally");
        fs::write(&log, b"")?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["--exact", "tally_child", "--nocapture", "--test-threads=1"])
            .env("B9_JOB", &job)
            .env("B9_TALLY", &log)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let engine = child::live(scene, &mut cmd, false)?;
        // Kept open until the run ends, so stdin's EOF stops nothing.
        let stdin = scene.take_stdin(engine);
        let status = scene.wait(engine)?;
        drop(stdin);
        // One input that is not a bundle: a failed input, 4.
        assert_eq!(status.code(), Some(4));
        let text = fs::read_to_string(&log)?;
        let lines: Vec<&str> = text.lines().collect();
        for name in [
            "tally_child",
            "exit-writer",
            "batch-progress",
            "batch-finisher",
            "batch-lifecycle",
            "batch-stdin",
            "batch-0",
            "batch-1",
            "batch-controller",
        ] {
            let line = format!("register {name}");
            assert!(lines.contains(&line.as_str()), "{name}: {text}");
        }
        // Both runtimes' blocking pools, `dc + 1` threads each.
        let pooled = lines
            .iter()
            .filter(|l| {
                l.starts_with("register ")
                    && !l.contains(" batch-")
                    && !l.ends_with(" tally_child")
                    && !l.ends_with(" exit-writer")
            })
            .count();
        assert!(pooled >= 4, "{text}");
        for name in ["batch-lifecycle", "batch-stdin"] {
            let line = format!("mark {name}");
            assert!(lines.contains(&line.as_str()), "{name}: {text}");
        }
        Ok(())
    })
    .unwrap();
}

/// A main-thread panic before the host exists still writes its line, by
/// the writer step 1 prepared, and exits 70.
#[test]
fn a_main_thread_panic_before_the_host_writes_its_line() {
    child::scene(|scene| {
        let dir = scene.scratch.path().to_owned();
        let job = job(&dir);
        let log = dir.join("tally");
        fs::write(&log, b"")?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["--exact", "tally_child", "--nocapture", "--test-threads=1"])
            .env("B9_JOB", &job)
            .env("B9_TALLY", &log)
            .env("B9_PANIC_EARLY", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let engine = child::live(scene, &mut cmd, false)?;
        let (rx, thread) = child::drain(scene.take_stderr(engine).unwrap());
        let status = scene.wait(engine)?;
        thread.join().unwrap();
        let err = String::from_utf8(rx.try_iter().flatten().collect()).unwrap();
        assert_eq!(status.code(), Some(70), "{err}");
        let line = err
            .lines()
            .find(|l| l.starts_with("unityrs batch: internal: "))
            .unwrap_or_else(|| panic!("{err:?}"));
        assert!(line.ends_with(": early registration"), "{err:?}");
        Ok(())
    })
    .unwrap();
}
