use serde_json::{Value, json};
use std::{
    fs, io,
    os::unix::fs::{MetadataExt, symlink},
    path::Path,
};
use unityrs_batch::{
    spec,
    start::{
        self, Unsupported,
        spool::{self, Class, FileSystem},
    },
};
use unityrs_testkit::{child, guard, scratch::Scratch};

struct Space {
    free: u64,
    in_memory: bool,
    cleanup_error: bool,
}
impl FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> io::Result<bool> {
        Ok(self.in_memory)
    }
    fn available(&self, _: &fs::File) -> io::Result<u64> {
        Ok(self.free)
    }
    fn cleanup(
        &self,
        root: &fs::File,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), unityrs_batch::spool::Error> {
        if self.cleanup_error {
            Err(unityrs_batch::spool::Error::HostIo(
                io::Error::from_raw_os_error(libc::EIO),
            ))
        } else {
            unityrs_batch::sys::remove_tree(root, Path::new("runs"), stopped)
        }
    }
}
fn job(spool: &Path, sink: &Path) -> Value {
    json!({"spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":268_435_456,"max_decoded_bytes":16_777_216,"dep_cache_bytes":33_554_432,
        "spool":{"dir":spool,"size_bytes":1_073_741_824,"reserve_bytes":10},
        "inputs":[{"id":"a","path":"/source"}],"outputs":{"textures":{"key":"{input}/{name}.png"}},"sink":{"kind":"directory","root":sink}})
}
fn parse(v: &Value) -> spec::model::Spec {
    spec::parse(serde_json::to_vec(v).unwrap()).unwrap().spec
}
fn old(root: &Path) {
    fs::write(root.join(".unityrs-spool"), b"").unwrap();
    fs::create_dir_all(root.join("runs/old/attempts/0")).unwrap();
    fs::write(root.join("runs/old/attempts/0/payload"), b"old").unwrap();
}
fn space() -> Space {
    Space {
        free: 10,
        in_memory: false,
        cleanup_error: false,
    }
}
#[test]
fn directory_archives_are_in_the_execution_subset() {
    let sink = Scratch::new("local-archives-sink");
    let mut v = job(Path::new("/spool"), sink.path());
    v["archives"] = json!([{"key":"archive.zip","members":[{"kind":"texture","path":"{name}"}]}]);
    assert!(start::supported(&parse(&v)).is_ok());
}
#[test]
fn urls_including_unused_dependencies_are_in_the_execution_subset() {
    let mut v = job(Path::new("/spool"), Path::new("/sink"));
    v["inputs"] = json!([{"id":"a","url":"http://127.0.0.1/a"}]);
    v["dependencies"] = json!([{"id":"unused","url":"https://example.invalid/a"}]);
    assert!(start::supported(&parse(&v)).is_ok());
}
#[test]
fn unsupported_execution_fields_are_typed_and_mutate_nothing() {
    let kind = Unsupported::ObjectStore;
    let scratch = Scratch::new("local-refusal");
    let sink = Scratch::new("local-refusal-sink");
    old(scratch.path());
    let mut v = job(scratch.path(), sink.path());
    v["sink"] = json!({"kind":"object_store","bucket":"bucket","region":"region","prefix":""});
    v["deadline"] = json!("2099-01-01T00:00:00Z");
    let spec = parse(&v);
    assert_eq!(start::supported(&spec).err(), Some(kind));
    assert_eq!(
        fs::read(scratch.join("runs/old/attempts/0/payload")).unwrap(),
        b"old"
    );
    assert!(!scratch.join("lock").exists());
    assert!(!sink.join("_unityrs").exists());
}
/// The lifetime fields are the process host's, accepted by the subset.
#[test]
fn lifetime_fields_are_in_the_execution_subset() {
    let sink = Scratch::new("local-lifetime-sink");
    let mut v = job(Path::new("/spool"), sink.path());
    v["deadline"] = json!("2099-01-01T00:00:00Z");
    v["stop_on_stdin_eof"] = json!(true);
    v["parent_pid"] = json!(2);
    assert!(start::supported(&parse(&v)).is_ok());
}
#[test]
fn cleanup_is_no_follow_and_creates_only_fresh_run_layout_and_zero_claims() {
    let scratch = Scratch::new("local-cleanup");
    let sink = Scratch::new("local-cleanup-sink");
    let outside = Scratch::new("local-cleanup-outside");
    old(scratch.path());
    fs::write(outside.join("sentinel"), b"safe").unwrap();
    symlink(outside.path(), scratch.join("runs/old/link")).unwrap();
    let spec = parse(&job(scratch.path(), sink.path()));
    let local = start::supported(&spec).unwrap();
    let started = spool::start(&local, &space(), &|| false).unwrap();
    assert!(!scratch.join("runs/old").exists());
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"safe");
    for name in ["in", "attempts", "fail", "done", "winners"] {
        assert!(started.run.join(name).is_dir());
    }
    assert_eq!(started.counter.used(), 0);
    assert!(scratch.join("lock").is_file());
    assert!(scratch.join(".unityrs-spool").is_file());
}
#[test]
fn dedication_memory_and_overlap_refusals_leave_old_artifacts_unchanged() {
    for cause in [
        "foreign",
        "lock_symlink",
        "runs_symlink",
        "memory",
        "overlap",
    ] {
        let scratch = Scratch::new("local-preflight");
        let sink = Scratch::new("local-preflight-sink");
        old(scratch.path());
        let mut sp = space();
        match cause {
            "foreign" => fs::write(scratch.join("foreign"), b"user").unwrap(),
            "lock_symlink" => symlink(sink.path(), scratch.join("lock")).unwrap(),
            "runs_symlink" => {
                fs::rename(scratch.join("runs"), scratch.join("saved")).unwrap();
                fs::remove_dir_all(scratch.join("saved")).unwrap();
                symlink(sink.path(), scratch.join("runs")).unwrap();
            }
            "memory" => sp.in_memory = true,
            "overlap" => symlink(scratch.path(), sink.join("alias")).unwrap(),
            _ => {}
        }
        let alias = sink.join("alias");
        let output = if cause == "overlap" {
            alias.as_path()
        } else {
            sink.path()
        };
        let spec = parse(&job(scratch.path(), output));
        let local = start::supported(&spec).unwrap();
        let e = spool::start(&local, &sp, &|| false).err().unwrap();
        assert_eq!(
            e.class,
            match cause {
                "memory" => Class::InMemory,
                "overlap" => Class::OverlapsSink,
                _ => Class::NotDedicated,
            }
        );
        if cause != "runs_symlink" {
            assert_eq!(
                fs::read(scratch.join("runs/old/attempts/0/payload")).unwrap(),
                b"old"
            );
        }
    }
}
#[test]
fn cleanup_fault_stop_and_space_refusal_have_distinct_boundaries() {
    for cause in ["cleanup", "stop", "space"] {
        let scratch = Scratch::new("local-fault");
        let sink = Scratch::new("local-fault-sink");
        old(scratch.path());
        let spec = parse(&job(scratch.path(), sink.path()));
        let local = start::supported(&spec).unwrap();
        let mut sp = space();
        if cause == "cleanup" {
            sp.cleanup_error = true;
        }
        if cause == "space" {
            sp.free = 9;
        }
        let e = spool::start(&local, &sp, &|| cause == "stop")
            .err()
            .unwrap();
        assert_eq!(
            e.class,
            match cause {
                "cleanup" => Class::HostIo,
                "stop" => Class::Stopped,
                _ => Class::NoSpace,
            }
        );
        assert_eq!(
            scratch.join("runs/old/attempts/0/payload").exists(),
            cause != "space"
        );
        assert!(!scratch.join("runs/r").exists());
    }
}
#[test]
fn independent_child_cannot_take_the_permanent_spool_lock() {
    let scratch = Scratch::new("local-lock");
    let sink = Scratch::new("local-lock-sink");
    let spec = parse(&job(scratch.path(), sink.path()));
    let local = start::supported(&spec).unwrap();
    let started = spool::start(&local, &space(), &|| false).unwrap();
    let inode = started.lock().metadata().unwrap();
    let path = scratch.path().to_owned();
    let output = sink.path().to_owned();
    let out = guard::finishes("independent spool lock", move || {
        child::output(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "spool_lock_child"])
                .env("UNITYRS_SPOOL_LOCK_PROBE", path)
                .env("UNITYRS_SPOOL_LOCK_SINK", output),
        )
        .unwrap()
    });
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    drop(started);
    let next = spool::start(&local, &space(), &|| false).unwrap();
    assert_eq!(next.lock().metadata().unwrap().ino(), inode.ino());
}
#[test]
fn spool_lock_child() {
    let Some(root) = std::env::var_os("UNITYRS_SPOOL_LOCK_PROBE") else {
        return;
    };
    let sink = std::env::var_os("UNITYRS_SPOOL_LOCK_SINK").unwrap();
    let spec = parse(&job(Path::new(&root), Path::new(&sink)));
    let local = start::supported(&spec).unwrap();
    assert_eq!(
        spool::start(&local, &space(), &|| false)
            .err()
            .unwrap()
            .class,
        Class::Locked
    );
}

#[cfg(feature = "test-hooks")]
#[test]
fn spool_lock_is_released_while_an_unrelated_child_is_before_exec() {
    use std::io::{Read, Write};
    struct ForkAtSpace {
        socket: unityrs_batch::sys::testing::Rendezvous,
        acquired: std::sync::mpsc::Sender<()>,
        free: u64,
    }
    impl FileSystem for ForkAtSpace {
        fn in_memory(&self, _: &fs::File) -> io::Result<bool> {
            Ok(false)
        }
        fn available(&self, _: &fs::File) -> io::Result<u64> {
            // Startup already owns the lock. Let the other thread fork,
            // then wait until the child confirms it is before exec.
            self.acquired.send(()).unwrap();
            let mut ready = [0];
            self.socket.stream().read_exact(&mut ready)?;
            assert_eq!(ready, [1]);
            Ok(self.free)
        }
    }
    let () = guard::finishes("spool lock inherited across fork", || {
        // Both a returned lease and a startup error must release the lock.
        for free in [10, 0] {
            child::scene(|scene| {
                let scratch = Scratch::new("local-lock-fork");
                let sink = Scratch::new("local-lock-fork-sink");
                let spec = parse(&job(scratch.path(), sink.path()));
                let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
                cmd.args(["--exact", "spool_lock_child"]);
                let socket =
                    unityrs_batch::sys::testing::hold_before_exec(&mut cmd, guard::DEADLOCK_GUARD)?;
                let (acquired, held) = std::sync::mpsc::channel();
                let fs = ForkAtSpace {
                    socket,
                    acquired,
                    free,
                };
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let local = start::supported(&spec).unwrap();
                    let first = spool::start(&local, &fs, &|| false).map(drop);
                    // The child inherited the locked description and cannot
                    // exec until we reply. CLOEXEC cannot close its copy yet.
                    let next = spool::start(&local, &space(), &|| false);
                    fs.socket.stream().write_all(&[1]).unwrap();
                    tx.send((first, next)).unwrap();
                });
                guard::wait(&held, "startup owns the spool lock");
                let inode = fs::metadata(scratch.join("lock"))?.ino();
                let live = child::live(scene, &mut cmd, false)?;
                assert!(scene.wait(live)?.success());
                // Reap before asserting so even a failure leaves no held child.
                let (first, next) = guard::recv(&rx, "pre-exec spool child");
                if free == 0 {
                    assert_eq!(first.unwrap_err().class, Class::NoSpace);
                } else {
                    first.unwrap();
                }
                assert_eq!(next.unwrap().lock().metadata()?.ino(), inode);
                Ok(())
            })
            .unwrap();
        }
    });
}

struct ReplaceSpool<'a> {
    path: &'a Path,
    moved: &'a Path,
    symlink: bool,
}
impl FileSystem for ReplaceSpool<'_> {
    fn in_memory(&self, _: &fs::File) -> io::Result<bool> {
        fs::rename(self.path, self.moved)?;
        if self.symlink {
            symlink(self.moved, self.path)?;
        } else {
            fs::create_dir(self.path)?;
            fs::write(self.path.join(".unityrs-spool"), b"")?;
        }
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> io::Result<u64> {
        Ok(10)
    }
}
#[test]
fn replaced_spool_path_refuses_before_cleanup_or_returning_an_unowned_run() {
    for link in [false, true] {
        let scratch = Scratch::new("local-canonical-race");
        let sink = Scratch::new("local-canonical-race-sink");
        let path = scratch.join("spool");
        let moved = scratch.join("moved");
        fs::create_dir(&path).unwrap();
        old(&path);
        let spec = parse(&job(&path, sink.path()));
        let local = start::supported(&spec).unwrap();
        let adapter = ReplaceSpool {
            path: &path,
            moved: &moved,
            symlink: link,
        };
        let error = spool::start(&local, &adapter, &|| false).err().unwrap();
        assert_eq!(error.class, Class::Unusable);
        assert_eq!(
            fs::read(moved.join("runs/old/attempts/0/payload")).unwrap(),
            b"old"
        );
        assert!(!path.join("runs/r").exists());
        assert!(!moved.join("runs/r").exists());
    }
}

struct CrashBeforeLock;
impl FileSystem for CrashBeforeLock {
    fn in_memory(&self, _: &fs::File) -> io::Result<bool> {
        Ok(false)
    }
    fn open(&self, root: &fs::File, name: &std::ffi::CStr, flags: i32) -> io::Result<fs::File> {
        if name == c"lock" {
            // Abrupt child exit: no startup cleanup or unwinding runs.
            std::process::exit(0);
        }
        unityrs_batch::sys::rooted_open(root, name, flags, false)
    }
}
#[test]
fn first_use_crash_before_lock_leaves_a_marker_and_restart_succeeds() {
    let scratch = Scratch::new("local-first-use-crash");
    let sink = Scratch::new("local-first-use-crash-sink");
    let path = scratch.path().to_owned();
    let output = sink.path().to_owned();
    let out = guard::finishes("first-use crash", move || {
        child::output(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "first_use_crash_child"])
                .env("UNITYRS_FIRST_USE_CRASH", path)
                .env("UNITYRS_FIRST_USE_SINK", output),
        )
        .unwrap()
    });
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(scratch.join(".unityrs-spool").is_file());
    assert!(!scratch.join("lock").exists());
    let spec = parse(&job(scratch.path(), sink.path()));
    let local = start::supported(&spec).unwrap();
    let restarted = spool::start(&local, &space(), &|| false).unwrap();
    assert!(restarted.run.join("in").is_dir());
}
#[test]
fn first_use_crash_child() {
    let Some(path) = std::env::var_os("UNITYRS_FIRST_USE_CRASH") else {
        return;
    };
    let sink = std::env::var_os("UNITYRS_FIRST_USE_SINK").unwrap();
    let spec = parse(&job(Path::new(&path), Path::new(&sink)));
    let local = start::supported(&spec).unwrap();
    let _ = spool::start(&local, &CrashBeforeLock, &|| false);
    panic!("startup did not reach the injected crash");
}
#[test]
fn lone_lock_is_still_not_a_dedicated_spool() {
    let scratch = Scratch::new("local-foreign-lock");
    let sink = Scratch::new("local-foreign-lock-sink");
    fs::write(scratch.join("lock"), b"foreign").unwrap();
    let spec = parse(&job(scratch.path(), sink.path()));
    let local = start::supported(&spec).unwrap();
    assert_eq!(
        spool::start(&local, &space(), &|| false)
            .err()
            .unwrap()
            .class,
        Class::NotDedicated
    );
    assert_eq!(fs::read(scratch.join("lock")).unwrap(), b"foreign");
    assert!(!scratch.join(".unityrs-spool").exists());
}

#[test]
fn spool_alias_is_canonicalized_before_the_no_follow_open() {
    let scratch = Scratch::new("local-spool-alias");
    let sink = Scratch::new("local-spool-alias-sink");
    let real = scratch.join("real");
    let alias = scratch.join("alias");
    fs::create_dir(&real).unwrap();
    symlink(&real, &alias).unwrap();
    let spec = parse(&job(&alias, sink.path()));
    let local = start::supported(&spec).unwrap();
    let started = spool::start(&local, &space(), &|| false).unwrap();
    let actual = fs::metadata(&started.run).unwrap();
    let expected = fs::metadata(real.join("runs/r")).unwrap();
    assert_eq!(
        (actual.dev(), actual.ino()),
        (expected.dev(), expected.ino())
    );
    assert!(started.run.join("in").is_dir());
}

#[cfg(target_os = "linux")]
#[test]
fn a_long_canonical_spool_keeps_view_paths_bounded_and_pins_the_root() {
    let scratch = Scratch::new("local-long-spool-alias");
    let sink = Scratch::new("local-long-spool-alias-sink");
    let mut real = scratch.path().to_path_buf();
    for n in 0..8 {
        real.push(format!("{n}{}", "x".repeat(190)));
    }
    fs::create_dir_all(&real).unwrap();
    let alias = scratch.join("alias");
    symlink(&real, &alias).unwrap();
    let mut raw = job(&alias, sink.path());
    let run_id = "r".repeat(64);
    raw["run_id"] = json!(run_id);
    let spec = parse(&raw);
    let local = start::supported(&spec).unwrap();
    let started = spool::start(&local, &space(), &|| false).unwrap();
    assert!(fs::canonicalize(&alias).unwrap().as_os_str().len() > 1_024);
    // The 64-byte run id and a full u32 generated index still leave a
    // short path for each worker's view, regardless of canonical depth.
    let staged = unityrs_batch::stage::path(&started.run.join("in"), u32::MAX, 1, false);
    assert!(staged.as_os_str().len() <= 128, "view path: {staged:?}");
    fs::write(&staged, b"pinned staged bytes").unwrap();
    let basename = staged.file_name().unwrap();
    let physical = real.join("runs").join(&run_id).join("in").join(basename);
    assert_eq!(fs::read(&physical).unwrap(), b"pinned staged bytes");
    let renamed = real.with_file_name("renamed-spool");
    fs::rename(&real, &renamed).unwrap();
    assert!(!alias.exists());
    assert_eq!(fs::read(&staged).unwrap(), b"pinned staged bytes");
    let actual = fs::metadata(&staged).unwrap();
    let expected =
        fs::metadata(renamed.join("runs").join(&run_id).join("in").join(basename)).unwrap();
    assert_eq!(
        (actual.dev(), actual.ino()),
        (expected.dev(), expected.ino())
    );
}
