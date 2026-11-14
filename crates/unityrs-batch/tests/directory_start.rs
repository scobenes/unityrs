//! Startup-only safety tests; publication/outcome integration belongs downstream.
use std::{
    fs::{self, File},
    io,
    os::unix::fs::{MetadataExt, symlink},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    manifest::head::HEAD_LIMIT,
    sink::{Class, Context, directory::Directory, fs::FileSystem, path::Root},
};
use unityrs_testkit::{child, guard, scratch::Scratch};

mod directory_support;
use directory_support::*;

fn put(root: &Root, key: &str, cx: Context) -> Result<(), unityrs_batch::sink::Error> {
    let parent = root.parent(key, true, cx)?;
    let temp = parent.temporary("r", 0, cx)?;
    temp.write_all(b"new")?;
    temp.writeback()?;
    temp.rename(key.rsplit('/').next().unwrap())
}
#[test]
fn replaces_final_symlinks_fifos_regular_files_and_stale_temp_files_safely() {
    for fallback in resolvers() {
        let scratch = Scratch::new("directory-final");
        let outside = Scratch::new("directory-outside");
        let target = outside.join("target");
        fs::write(&target, b"safe").unwrap();
        let root = Root::open(scratch.path(), fs(fallback)).unwrap();
        symlink(&target, scratch.join("link")).unwrap();
        symlink(&target, scratch.join(".unityrs-tmp-r-0")).unwrap();
        put(&root, "link", Context::Output).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"safe");
        assert_eq!(fs::read(scratch.join("link")).unwrap(), b"new");
        fs::write(scratch.join("regular"), b"old").unwrap();
        fs::write(scratch.join(".unityrs-tmp-r-0"), b"partial").unwrap();
        put(&root, "regular", Context::Output).unwrap();
        let out =
            child::output(std::process::Command::new("mkfifo").arg(scratch.join("fifo"))).unwrap();
        assert!(out.status.success());
        put(&root, "fifo", Context::Output).unwrap();
        assert_eq!(fs::read(scratch.join("fifo")).unwrap(), b"new");
        fs::create_dir(scratch.join("directory")).unwrap();
        assert_eq!(
            put(&root, "directory", Context::Output).unwrap_err().class,
            Class::PublishFailed
        );
        fs::remove_file(scratch.join(".unityrs-tmp-r-0")).unwrap();
        fs::create_dir(scratch.join(".unityrs-tmp-r-0")).unwrap();
        assert_eq!(
            put(&root, "other", Context::Output).unwrap_err().class,
            Class::PublishFailed
        );
        assert!(scratch.join("directory").is_dir());
    }
}
#[test]
fn planted_intermediates_and_parent_creation_races_never_escape() {
    for fallback in resolvers() {
        for race in ["planted", "mkdir_symlink", "mkdir_directory"] {
            let scratch = Scratch::new("directory-parent");
            let outside = Scratch::new("directory-escape");
            fs::write(outside.join("sentinel"), b"safe").unwrap();
            let fs = fs(fallback);
            if race == "planted" {
                symlink(outside.path(), scratch.join("parent")).unwrap();
            } else {
                *fs.race.lock().unwrap() =
                    Some((scratch.join("parent"), outside.path().into(), race));
            }
            let root = Root::open(scratch.path(), fs).unwrap();
            let result = put(&root, "parent/file", Context::Output);
            if race == "mkdir_directory" {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().class, Class::PublishFailed);
            }
            assert!(!outside.join("file").exists());
            assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"safe");
        }
    }
}
#[test]
fn renamed_parent_with_symlink_substitution_refuses_commit() {
    for fallback in resolvers() {
        let scratch = Scratch::new("directory-rename");
        let outside = Scratch::new("directory-rename-target");
        fs::create_dir(scratch.join("parent")).unwrap();
        let root = Root::open(scratch.path(), fs(fallback)).unwrap();
        let parent = root.parent("parent/file", true, Context::Output).unwrap();
        let temp = parent.temporary("r", 0, Context::Output).unwrap();
        temp.write_all(b"new").unwrap();
        fs::rename(scratch.join("parent"), scratch.join("moved")).unwrap();
        symlink(outside.path(), scratch.join("parent")).unwrap();
        assert_eq!(temp.rename("file").unwrap_err().class, Class::PublishFailed);
        assert!(!scratch.join("moved/file").exists());
        assert!(!outside.join("file").exists());
        assert_eq!(
            root.parent("parent/file", false, Context::Output)
                .err()
                .unwrap()
                .class,
            Class::PublishFailed
        );
    }
}
#[test]
fn symlink_inserted_between_unlink_and_exclusive_create_is_not_followed() {
    let scratch = Scratch::new("directory-temp-race");
    let outside = Scratch::new("directory-temp-target");
    fs::write(outside.join("target"), b"safe").unwrap();
    let fs = fs(true);
    let root = Root::open(scratch.path(), fs.clone()).unwrap();
    *fs.race.lock().unwrap() = Some((
        scratch.join(".unityrs-tmp-r-0"),
        outside.join("target"),
        "temp_create",
    ));
    assert_eq!(
        put(&root, "file", Context::Output).unwrap_err().class,
        Class::PublishFailed
    );
    assert_eq!(fs::read(outside.join("target")).unwrap(), b"safe");
}
#[test]
fn head_order_short_writes_eintr_and_uncertain_rename_are_conservative() {
    for fault in [
        None,
        Some("write"),
        Some("fsync"),
        Some("rename"),
        Some("fsync_dir"),
    ] {
        for running in [false, true] {
            let scratch = Scratch::new("directory-head");
            let fs = Arc::new(Faults {
                short_write: true,
                interrupt: true,
                after_rename: fault == Some("rename"),
                fail: fault.map(|p| (p, libc::EIO)),
                disarmed: true.into(),
                crossing: AtomicUsize::new(usize::MAX),
                ..Faults::default()
            });
            fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
            let directory = preflight(scratch.path(), 1, fs.clone()).unwrap();
            fs.disarmed
                .store(false, std::sync::atomic::Ordering::SeqCst);
            fs.calls.lock().unwrap().clear();
            let result = directory.write_head(&head(1), running);
            if fault.is_some() {
                assert_eq!(
                    result.unwrap_err().class,
                    if running {
                        Class::HostIo
                    } else {
                        Class::PublishFailed
                    }
                );
            } else {
                result.unwrap();
                let calls = fs.calls.lock().unwrap();
                let tail: Vec<_> = calls
                    .iter()
                    .copied()
                    .filter(|s| ["fsync", "rename", "fsync_dir"].contains(s))
                    .collect();
                assert_eq!(tail, ["fsync", "rename", "fsync_dir"]);
                let renamed = calls.iter().rposition(|p| *p == "rename").unwrap();
                assert_eq!(&calls[renamed + 1..], ["fsync_dir"]);
            }
            let physical = scratch.join("_unityrs/r/manifest.json");
            assert_eq!(
                physical.exists(),
                matches!(fault, None | Some("rename" | "fsync_dir"))
            );
            if physical.exists() {
                assert_eq!(fs::read(physical).unwrap(), head(1));
            }
        }
    }
}
#[test]
fn errno_classes_are_context_specific_and_sink_capacity_is_host_io() {
    for errno in [
        libc::EIO,
        libc::ENOSPC,
        libc::EDQUOT,
        libc::EACCES,
        libc::EPERM,
    ] {
        for point in ["write", "fsync", "rename", "fsync_dir"] {
            let scratch = Scratch::new("directory-errno");
            let fs = Arc::new(Faults {
                fail: Some((point, errno)),
                disarmed: true.into(),
                crossing: AtomicUsize::new(usize::MAX),
                ..Faults::default()
            });
            fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
            let directory = preflight(scratch.path(), 1, fs.clone()).unwrap();
            fs.disarmed
                .store(false, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(
                directory.write_head(&head(1), true).unwrap_err().class,
                Class::HostIo
            );
        }
        for point in ["write", "writeback", "rename", "syncfs"] {
            let scratch = Scratch::new("directory-output-errno");
            let fs = Arc::new(Faults {
                fail: Some((point, errno)),
                crossing: AtomicUsize::new(usize::MAX),
                ..Faults::default()
            });
            fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
            let directory = preflight(scratch.path(), 1, fs).unwrap();
            let result = if point == "syncfs" {
                directory.syncfs()
            } else {
                put(directory.root(), "file", Context::Output)
            };
            assert_eq!(result.unwrap_err().class, Class::HostIo, "{point} {errno}");
        }
    }
}
#[test]
fn fallback_is_only_for_probe_enosys_or_eperm_and_checks_every_mount() {
    for errno in [
        libc::ENOSYS,
        libc::EPERM,
        libc::EINVAL,
        libc::EIO,
        libc::EXDEV,
    ] {
        let scratch = Scratch::new("directory-probe");
        let fs = Arc::new(Faults {
            fallback: Some(errno),
            crossing: AtomicUsize::new(usize::MAX),
            ..Faults::default()
        });
        let result = Root::open(scratch.path(), fs);
        assert_eq!(result.is_ok(), matches!(errno, libc::ENOSYS | libc::EPERM));
    }
    for (missing, error) in [(true, false), (false, true)] {
        let scratch = Scratch::new("directory-mount-absent");
        let fs = Arc::new(Faults {
            fallback: Some(libc::ENOSYS),
            missing_mount: missing,
            mount_error: error,
            ..Faults::default()
        });
        assert_eq!(
            Root::open(scratch.path(), fs).err().unwrap().class,
            Class::MountCheckUnavailable
        );
    }
    for cx in [
        Context::Preflight,
        Context::Output,
        Context::RunningHead,
        Context::Finish,
    ] {
        let scratch = Scratch::new("directory-crossing");
        let fs = fs(true);
        let root = Root::open(scratch.path(), fs.clone()).unwrap();
        fs.crossing
            .store(fs.mounts.load(Ordering::SeqCst), Ordering::SeqCst);
        let error = root.parent("parent/file", true, cx).err().unwrap();
        assert_eq!(
            error.class,
            match cx {
                Context::Preflight => Class::MountCrossing,
                Context::Finish => Class::PublishFailed,
                _ => Class::HostIo,
            }
        );
    }
}
#[test]
fn malformed_foreign_and_superseding_heads_never_authorize_overwrite() {
    for bytes in [
        b"{}".to_vec(),
        head(1).iter().copied().chain(*b"x").collect(),
        String::from_utf8(head(1))
            .unwrap()
            .replace("\"run_id\":\"r\"", "\"run_id\":\"other\"")
            .into_bytes(),
        vec![b' '; HEAD_LIMIT + 1],
        head(2),
        head(3),
    ] {
        let scratch = Scratch::new("directory-fence");
        fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
        let path = scratch.join("_unityrs/r/manifest.json");
        fs::write(&path, &bytes).unwrap();
        let error = preflight(scratch.path(), 2, fs(true)).err().unwrap();
        assert!(matches!(
            error.class,
            Class::ManifestUnreadable | Class::Superseded
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!scratch.join("_unityrs/r/2").exists());
    }
    let scratch = Scratch::new("directory-lower-fence");
    let one = preflight(scratch.path(), 1, fs(true)).unwrap();
    one.write_head(&head(1), true).unwrap();
    drop(one);
    let two = preflight(scratch.path(), 2, fs(true)).unwrap();
    two.write_head(&head(2), true).unwrap();
    assert_eq!(
        fs::read(scratch.join("_unityrs/r/manifest.json")).unwrap(),
        head(2)
    );
}
#[test]
fn independent_child_cannot_acquire_the_held_root_lock() {
    let scratch = Scratch::new("directory-lock");
    let owner = preflight(scratch.path(), 1, fs(true)).unwrap();
    let path = scratch.path().to_owned();
    // B7a probe: libtest captures failed assertions in stdout. Keep the status
    // and both streams so an unrelated child startup failure is diagnosable.
    let out = guard::finishes("independent root lock", move || {
        child::output(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "root_lock_child", "--nocapture"])
                .env("UNITYRS_ROOT_LOCK_PROBE", path),
        )
        .unwrap()
    });
    assert!(
        out.status.success(),
        "status={} stdout={} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    drop(owner);
    // The root lock is a `flock` on an open file description. A sibling test
    // that spawns a child (`mkfifo`) forks this process, and until that child
    // execs, its copy of the descriptor keeps the description, and the lock,
    // alive. Dropping `owner` inside that window leaves the root locked for a
    // moment, so the reacquire retries while the only refusal is that lock,
    // up to the deadlock guard; any other error fails at once.
    let start = std::time::Instant::now();
    loop {
        match preflight(scratch.path(), 1, fs(true)) {
            Ok(_) => break,
            Err(e) if e.class == Class::OutputLocked && start.elapsed() < guard::DEADLOCK_GUARD => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => panic!("reacquire after drop: {e:?}"),
        }
    }
}
#[test]
fn root_lock_child() {
    let Some(path) = std::env::var_os("UNITYRS_ROOT_LOCK_PROBE") else {
        return;
    };
    let result = Directory::preflight(
        Path::new(&path),
        "different-run",
        1,
        fs(true),
        &mut vec![0; HEAD_LIMIT + 1],
    );
    let error = result.err().expect("child acquired the held root lock");
    assert_eq!(error.class, Class::OutputLocked, "{error:?}");
}

#[test]
fn native_open_failures_do_not_enable_fallback_and_missing_child_masks_cross() {
    let probe = Scratch::new("directory-native-errors-probe");
    if native_available(probe.path()) {
        for cx in [
            Context::Preflight,
            Context::Output,
            Context::RunningHead,
            Context::Finish,
        ] {
            for errno in [libc::EXDEV, libc::EPERM, libc::ENOSYS, libc::EIO] {
                let scratch = Scratch::new("directory-native-error");
                let fs = Arc::new(Faults {
                    fail: Some(("open", errno)),
                    crossing: AtomicUsize::new(usize::MAX),
                    ..Faults::default()
                });
                let root = Root::open(scratch.path(), fs.clone()).unwrap();
                let e = root.parent("parent/file", true, cx).err().unwrap();
                let expected = match cx {
                    Context::Preflight if errno == libc::EXDEV => Class::MountCrossing,
                    Context::Preflight => Class::SinkRootUnusable,
                    Context::Finish => Class::PublishFailed,
                    _ => Class::HostIo,
                };
                assert_eq!(e.class, expected);
                assert_eq!(e.errno, Some(errno));
                assert_eq!(
                    fs.calls
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|p| **p == "probe")
                        .count(),
                    1
                );
            }
        }
    }
    let scratch = Scratch::new("directory-child-mask");
    let fs = fs(true);
    let root = Root::open(scratch.path(), fs.clone()).unwrap();
    fs.missing_mount_at.store(1, Ordering::SeqCst);
    assert_eq!(
        root.parent("parent/file", true, Context::Output)
            .err()
            .unwrap()
            .class,
        Class::HostIo
    );
}
#[test]
fn old_kernel_missing_root_and_invalid_paths_refuse_before_creating_metadata() {
    let scratch = Scratch::new("directory-start-refusal");
    let fs = Arc::new(Faults {
        release: Some((5, 7)),
        ..Faults::default()
    });
    assert_eq!(
        preflight(scratch.path(), 1, fs).err().unwrap().class,
        Class::KernelTooOld
    );
    assert!(!scratch.join("_unityrs").exists());
    assert_eq!(
        preflight(&scratch.join("missing"), 1, self::fs(true))
            .err()
            .unwrap()
            .class,
        Class::SinkRootUnusable
    );
    let root = Root::open(scratch.path(), self::fs(true)).unwrap();
    for key in [
        "/escape",
        "../escape",
        "parent/../escape",
        "parent//escape",
        "",
        "parent/",
        "parent/\0",
    ] {
        assert_eq!(
            root.parent(key, true, Context::Output).err().unwrap().class,
            Class::InvalidPath
        );
    }
    assert!(!scratch.join("parent").exists());
}

struct Reads {
    bytes: AtomicUsize,
    calls: AtomicUsize,
}
impl FileSystem for Reads {
    fn release(&self) -> io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe_sink_semantics(&self) -> bool {
        false
    }
    fn probe(&self, _: &File) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
    fn mount(&self, file: &File) -> io::Result<Option<u64>> {
        Ok(Some(file.metadata()?.dev()))
    }
    fn read(&self, mut file: &File, bytes: &mut [u8]) -> io::Result<usize> {
        use std::io::Read;
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(io::Error::from_raw_os_error(libc::EINTR));
        }
        let cap = bytes.len().min(257);
        let n = file.read(&mut bytes[..cap])?;
        self.bytes.fetch_add(n, Ordering::SeqCst);
        Ok(n)
    }
}
#[test]
fn head_reads_are_bounded_even_with_short_reads_and_interruptions() {
    let scratch = Scratch::new("directory-read-cap");
    fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
    fs::write(
        scratch.join("_unityrs/r/manifest.json"),
        vec![b' '; 2 * HEAD_LIMIT],
    )
    .unwrap();
    let fs = Arc::new(Reads {
        bytes: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
    });
    let e = Directory::preflight(
        scratch.path(),
        "r",
        2,
        fs.clone(),
        &mut vec![0; HEAD_LIMIT + 100],
    )
    .err()
    .unwrap();
    assert_eq!(e.class, Class::ManifestUnreadable);
    assert_eq!(e.call, "too_large");
    assert_eq!(fs.bytes.load(Ordering::SeqCst), HEAD_LIMIT + 1);
}
#[test]
fn metadata_symlinks_and_non_regular_locks_are_never_followed_or_locked() {
    for kind in ["metadata", "run", "lock", "head"] {
        let scratch = Scratch::new("directory-metadata-symlink");
        let outside = Scratch::new("directory-metadata-target");
        fs::write(outside.join("sentinel"), b"safe").unwrap();
        match kind {
            "metadata" => symlink(outside.path(), scratch.join("_unityrs")).unwrap(),
            "run" => {
                fs::create_dir(scratch.join("_unityrs")).unwrap();
                symlink(outside.path(), scratch.join("_unityrs/r")).unwrap();
            }
            "lock" => {
                fs::create_dir_all(scratch.join("_unityrs/lock")).unwrap();
            }
            _ => {
                fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
                symlink(
                    outside.join("sentinel"),
                    scratch.join("_unityrs/r/manifest.json"),
                )
                .unwrap();
            }
        }
        assert!(preflight(scratch.path(), 1, fs(true)).is_err());
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"safe");
        assert!(!outside.join("lock").exists());
    }
}

#[test]
fn temp_descriptors_use_the_selected_resolver_and_fallback_mount_checks() {
    let scratch = Scratch::new("directory-temp-resolver");
    if native_available(scratch.path()) {
        let fs = Arc::new(Faults {
            native_temp: true,
            crossing: AtomicUsize::new(usize::MAX),
            ..Faults::default()
        });
        let root = Root::open(scratch.path(), fs).unwrap();
        put(&root, "native", Context::Output).unwrap();
    }
    let fs = self::fs(true);
    let root = Root::open(scratch.path(), fs.clone()).unwrap();
    let parent = root.parent("fallback", true, Context::Output).unwrap();
    fs.missing_mount_at
        .store(fs.mounts.load(Ordering::SeqCst), Ordering::SeqCst);
    assert_eq!(
        parent
            .temporary("r", 0, Context::Output)
            .err()
            .unwrap()
            .class,
        Class::HostIo
    );
}

#[test]
fn head_read_errors_are_preflight_failures_and_preserve_the_previous_head() {
    for errno in [libc::EIO, libc::EACCES, libc::ENOSPC, libc::EDQUOT] {
        let scratch = Scratch::new("directory-head-read-error");
        fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
        let path = scratch.join("_unityrs/r/manifest.json");
        fs::write(&path, head(1)).unwrap();
        let fs = Arc::new(Faults {
            fail: Some(("read", errno)),
            crossing: AtomicUsize::new(usize::MAX),
            ..Faults::default()
        });
        assert_eq!(
            preflight(scratch.path(), 2, fs).err().unwrap().class,
            Class::SinkRootUnusable
        );
        assert_eq!(fs::read(path).unwrap(), head(1));
    }
}

#[test]
fn destination_mount_and_kind_rename_errors_are_distinct_from_host_failures() {
    for errno in [libc::EISDIR, libc::ENOTEMPTY, libc::ENOTDIR, libc::EBUSY] {
        let scratch = Scratch::new("directory-destination-kind");
        let fs = Arc::new(Faults {
            fail: Some(("rename", errno)),
            crossing: AtomicUsize::new(usize::MAX),
            ..Faults::default()
        });
        let root = Root::open(scratch.path(), fs).unwrap();
        assert_eq!(
            put(&root, "key", Context::Output).unwrap_err().class,
            Class::PublishFailed
        );
        assert!(!scratch.join("key").exists());
        assert_eq!(fs::read(scratch.join(".unityrs-tmp-r-0")).unwrap(), b"new");
    }
    for obstacle in ["regular", "fifo"] {
        let scratch = Scratch::new("directory-intermediate-kind");
        let path = scratch.join("parent");
        if obstacle == "regular" {
            fs::write(&path, b"safe").unwrap();
        } else {
            assert!(
                child::output(std::process::Command::new("mkfifo").arg(&path))
                    .unwrap()
                    .status
                    .success()
            );
        }
        for fallback in resolvers() {
            let root = Root::open(scratch.path(), fs(fallback)).unwrap();
            assert_eq!(
                put(&root, "parent/key", Context::Output).unwrap_err().class,
                Class::PublishFailed
            );
        }
    }
}
