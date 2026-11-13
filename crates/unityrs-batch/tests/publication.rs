mod common;
#[allow(dead_code)]
mod directory_support;
mod publication_support;
use directory_support::{Faults, fs, preflight};
use publication_support::Scene;
use std::os::unix::fs::MetadataExt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use unityrs_batch::{
    outcomes::{Code, Outcomes},
    phase::{Account, Error},
    publish::{self, Admission, Context, Gate, Hooks, Point, payload::Committed},
    sink::directory::Directory,
    spool::Host,
};
fn drive(
    scene: &Scene,
    directory: &Directory,
    outcomes: &Outcomes,
    gate: &Gate,
    resume: bool,
    hooks: &dyn Hooks,
    stop: &AtomicBool,
) -> Result<Admission, Error> {
    let account = Account::new(64 << 20);
    let result = publish::drive(&Context {
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        payloads: &Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        },
        directory,
        outcomes,
        gate,
        account: &account,
        run_id: "r",
        resume,
        stopped: &|| stop.load(Ordering::SeqCst),
        hooks,
    });
    assert_eq!(account.snapshot().input.used, 0);
    assert!(account.snapshot().input.peak <= publish::WORKING_SET);
    result
}
fn directory(scene: &Scene, fs: Arc<Faults>) -> Directory {
    std::fs::create_dir(scene.scratch.join("sink")).unwrap();
    preflight(&scene.scratch.join("sink"), 1, fs).unwrap()
}
#[test]
fn local_copy_and_resume_replace_only_the_exact_destination() {
    let scene = Scene::new(&["parent/key"], false, 4096);
    let adapter = fs(true);
    let dir = directory(&scene, adapter.clone());
    let gate = Gate::default();
    let stop = AtomicBool::new(false);
    let out = scene.outcomes("out1", &Host);
    let first = drive(&scene, &dir, &out, &gate, false, &publish::NoHooks, &stop).unwrap();
    assert_eq!(
        first,
        Admission::Drained(publish::Report {
            written: 1,
            present: 0,
            failed: 0
        })
    );
    assert_eq!(
        std::fs::read(scene.scratch.join("sink/parent/key")).unwrap(),
        b"payload"
    );
    assert_eq!(out.loose(0).unwrap(), Code::Written);
    adapter.calls.lock().unwrap().clear();
    let out = scene.outcomes("out2", &Host);
    assert_eq!(
        drive(&scene, &dir, &out, &gate, true, &publish::NoHooks, &stop).unwrap(),
        Admission::Drained(publish::Report {
            written: 0,
            present: 1,
            failed: 0
        })
    );
    assert_eq!(out.loose(0).unwrap(), Code::Present);
    assert!(!adapter.calls.lock().unwrap().contains(&"rename"));
    std::fs::write(scene.scratch.join("sink/parent/key"), b"payloae").unwrap();
    let out = scene.outcomes("out3", &Host);
    assert!(matches!(
        drive(&scene, &dir, &out, &gate, true, &publish::NoHooks, &stop),
        Ok(Admission::Drained(publish::Report { written: 1, .. }))
    ));
}
#[test]
fn every_directory_fault_keeps_the_outcome_conservative() {
    for operation in [
        "open",
        "mkdir",
        "unlink",
        "write",
        "writeback",
        "rename",
        "mount",
    ] {
        for errno in [libc::EIO, libc::ENOSPC, libc::EDQUOT] {
            let scene = Scene::new(&["parent/key"], false, 4096);
            let adapter = Arc::new(Faults {
                fallback: Some(libc::ENOSYS),
                fail: Some((operation, errno)),
                disarmed: AtomicBool::new(true),
                crossing: AtomicUsize::new(usize::MAX),
                ..Faults::default()
            });
            let dir = directory(&scene, adapter.clone());
            adapter.disarmed.store(false, Ordering::SeqCst);
            let out = scene.outcomes("out", &Host);
            let result = drive(
                &scene,
                &dir,
                &out,
                &Gate::default(),
                false,
                &publish::NoHooks,
                &AtomicBool::new(false),
            );
            assert!(
                matches!(result,Err(Error::Run(f)) if f.reason==unityrs_batch::attempt::ReasonCode::HostIo),
                "{operation} {errno}: {result:?}"
            );
            assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
        }
    }
}
#[test]
fn uncertain_rename_does_not_advertise_a_physically_replaced_file() {
    let scene = Scene::new(&["key"], false, 4096);
    let adapter = Arc::new(Faults {
        fallback: Some(libc::ENOSYS),
        fail: Some(("rename", libc::EIO)),
        after_rename: true,
        disarmed: AtomicBool::new(true),
        crossing: AtomicUsize::new(usize::MAX),
        ..Faults::default()
    });
    let dir = directory(&scene, adapter.clone());
    std::fs::write(scene.scratch.join("sink/key"), b"old").unwrap();
    adapter.disarmed.store(false, Ordering::SeqCst);
    let out = scene.outcomes("out", &Host);
    assert!(
        drive(
            &scene,
            &dir,
            &out,
            &Gate::default(),
            false,
            &publish::NoHooks,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(scene.scratch.join("sink/key")).unwrap(),
        b"payload"
    );
    assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
}
struct Stop<'a> {
    at: Point,
    stop: &'a AtomicBool,
}
impl Hooks for Stop<'_> {
    fn point(&self, point: Point) {
        if point == self.at {
            self.stop.store(true, Ordering::SeqCst);
        }
    }
}
#[test]
fn stops_on_each_side_of_authorization_and_frozen_gate() {
    for at in [
        Point::Admission,
        Point::Transfer,
        Point::Resume,
        Point::ResumeReleased,
        Point::Buffer,
        Point::BeforeCommit,
        Point::Authorized,
        Point::Outcome,
        Point::Drained,
    ] {
        let scene = Scene::new(&["key"], false, 4096);
        let dir = directory(&scene, fs(true));
        let out = scene.outcomes("out", &Host);
        let gate = Gate::default();
        let stop = AtomicBool::new(false);
        let result = drive(
            &scene,
            &dir,
            &out,
            &gate,
            true,
            &Stop { at, stop: &stop },
            &stop,
        );
        assert_eq!(result, Err(Error::Stopped), "{at:?}");
        let frozen = gate.freeze(&out).unwrap();
        assert_eq!(
            frozen.outcomes.loose(0).unwrap(),
            if matches!(at, Point::Authorized | Point::Outcome | Point::Drained) {
                Code::Written
            } else {
                Code::NotWritten
            }
        );
    }
}
#[test]
fn wrong_kinds_symlinks_and_partial_payloads() {
    use std::os::unix::fs::symlink;
    for kind in [
        "directory",
        "symlink",
        "intermediate",
        "temporary",
        "partial",
    ] {
        let scene = Scene::new(&["parent/key"], false, 4096);
        let dir = directory(&scene, fs(true));
        let parent = scene.scratch.join("sink/parent");
        if kind == "intermediate" {
            std::fs::write(&parent, b"obstacle").unwrap();
        } else {
            std::fs::create_dir(&parent).unwrap();
        }
        match kind {
            "directory" => std::fs::create_dir(parent.join("key")).unwrap(),
            "symlink" => symlink(scene.scratch.join("done/0/o0"), parent.join("key")).unwrap(),
            "temporary" => std::fs::create_dir(parent.join(".unityrs-tmp-r-0")).unwrap(),
            "partial" => {
                for i in 0..2 {
                    std::fs::write(scene.scratch.join(format!("done/{i}/o0")), b"pay").unwrap();
                }
            }
            _ => {}
        }
        let out = scene.outcomes("out", &Host);
        let result = drive(
            &scene,
            &dir,
            &out,
            &Gate::default(),
            true,
            &publish::NoHooks,
            &AtomicBool::new(false),
        );
        if kind == "partial" {
            assert_eq!(result, Err(Error::internal()));
            assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
        } else if kind == "symlink" {
            assert!(result.is_ok());
            assert_eq!(out.loose(0).unwrap(), Code::Written);
            assert!(
                !std::fs::symlink_metadata(parent.join("key"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        } else {
            assert!(result.is_ok(), "{kind}: {result:?}");
            assert_eq!(out.loose(0).unwrap(), Code::PublishFailed);
        }
    }
}
#[test]
fn outcome_failure_after_rename_is_spool_full_and_not_written() {
    let scene = Scene::new(&["key"], false, 4096);
    let dir = directory(&scene, fs(true));
    let armed = Arc::new(AtomicBool::new(false));
    let out = scene.outcomes("out", &publication_support::OutcomeFaults(armed.clone()));
    armed.store(true, Ordering::SeqCst);
    let result = drive(
        &scene,
        &dir,
        &out,
        &Gate::default(),
        false,
        &publish::NoHooks,
        &AtomicBool::new(false),
    );
    assert!(
        matches!(result,Err(Error::Run(f)) if f.reason==unityrs_batch::attempt::ReasonCode::SpoolFull && f.detail==Some(unityrs_batch::attempt::DetailCode::Enospc))
    );
    assert_eq!(
        std::fs::read(scene.scratch.join("sink/key")).unwrap(),
        b"payload"
    );
    assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
}
struct Observe<'a>(&'a AtomicBool);
impl Hooks for Observe<'_> {
    fn point(&self, p: Point) {
        if p == Point::BeforeCommit {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

#[test]
fn frozen_gate_excludes_a_late_commit() {
    let () = unityrs_testkit::guard::finishes("publication commit gate", || {
        let scene = Scene::new(&["key"], false, 4096);
        let dir = directory(&scene, fs(true));
        let out = scene.outcomes("out", &Host);
        let gate = Gate::default();
        let stop = AtomicBool::new(false);
        let reached = AtomicBool::new(false);
        let account = Account::new(64 << 20);
        let payloads = Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        };
        let stopped = || stop.load(Ordering::SeqCst);
        let hooks = Observe(&reached);
        let context = Context {
            handoff: &scene.handoff,
            workspace: &scene.workspace,
            payloads: &payloads,
            directory: &dir,
            outcomes: &out,
            gate: &gate,
            account: &account,
            run_id: "r",
            resume: false,
            stopped: &stopped,
            hooks: &hooks,
        };
        std::thread::scope(|scope| {
            let frozen = gate.freeze(&out).unwrap();
            let thread = scope.spawn(move || publish::drive(&context));
            while !reached.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            stop.store(true, Ordering::SeqCst);
            assert_eq!(frozen.outcomes.loose(0).unwrap(), Code::NotWritten);
            assert!(!scene.scratch.join("sink/key").exists());
            drop(frozen);
            assert_eq!(thread.join().unwrap(), Err(Error::Stopped));
        });
    });
}
#[test]
fn busy_is_scheduling_and_an_impossible_idle_envelope_is_internal() {
    let scene = Scene::new(&["key"], false, 4096);
    let dir = directory(&scene, fs(true));
    let out = scene.outcomes("out", &Host);
    let gate = Gate::default();
    let call = |account: &Account| {
        publish::drive(&Context {
            handoff: &scene.handoff,
            workspace: &scene.workspace,
            payloads: &Committed {
                done: &scene.done,
                states: &scene.prepared.inputs,
                fs: &Host,
            },
            directory: &dir,
            outcomes: &out,
            gate: &gate,
            account,
            run_id: "r",
            resume: false,
            stopped: &|| false,
            hooks: &publish::NoHooks,
        })
    };
    let account = Account::new(64 << 20);
    let held = unityrs_batch::phase::Buffer::<u8>::new(account.meter(), 1).unwrap();
    assert_eq!(call(&account), Ok(Admission::Busy));
    assert_eq!(account.snapshot().refusals(), 0);
    assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
    drop(held);
    assert!(matches!(call(&account), Ok(Admission::Drained(_))));
    let small = Account::new(publish::WORKING_SET - 1);
    assert_eq!(call(&small), Err(Error::internal()));
    assert_eq!(small.snapshot().refusals(), 0);
}
struct PartialWrite(AtomicUsize);
impl unityrs_batch::sink::fs::FileSystem for PartialWrite {
    fn release(&self) -> std::io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe(&self, _: &std::fs::File) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
    }
    fn entry(
        &self,
        parent: &std::fs::File,
        name: &std::ffi::CStr,
    ) -> std::io::Result<unityrs_batch::sys::Entry> {
        let mut entry = unityrs_batch::sys::rooted_entry(parent, name)?;
        entry.mount = self.mount(parent)?;
        Ok(entry)
    }
    fn mount(&self, file: &std::fs::File) -> std::io::Result<Option<u64>> {
        Ok(Some(file.metadata()?.dev()))
    }
    fn write(&self, mut file: &std::fs::File, b: &[u8]) -> std::io::Result<usize> {
        use std::io::Write;
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            file.write(&b[..b.len().min(3)])
        } else {
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        }
    }
}
#[test]
fn partial_temp_write_preserves_old_destination_and_claims_unknown() {
    let scene = Scene::new(&["key"], false, 4096);
    let root = scene.scratch.join("sink");
    std::fs::create_dir(&root).unwrap();
    let dir = Directory::preflight(
        &root,
        "r",
        1,
        Arc::new(PartialWrite(AtomicUsize::new(0))),
        &mut vec![0; 1_048_577],
    )
    .unwrap();
    std::fs::write(root.join("key"), b"old").unwrap();
    let out = scene.outcomes("out", &Host);
    let result = drive(
        &scene,
        &dir,
        &out,
        &Gate::default(),
        false,
        &publish::NoHooks,
        &AtomicBool::new(false),
    );
    assert!(
        matches!(result,Err(Error::Run(f)) if f.reason==unityrs_batch::attempt::ReasonCode::HostIo)
    );
    assert_eq!(std::fs::read(root.join("key")).unwrap(), b"old");
    assert_eq!(
        std::fs::read(root.join(".unityrs-tmp-r-0")).unwrap(),
        b"pay"
    );
    assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
}
#[test]
fn resume_io_failure_and_stop_during_hash_do_not_schedule_copy() {
    for fault in [true, false] {
        let scene = Scene::new(&["key"], false, 4096);
        let adapter = Arc::new(Faults {
            fallback: Some(libc::ENOSYS),
            fail: fault.then_some(("read", libc::EIO)),
            disarmed: AtomicBool::new(true),
            crossing: AtomicUsize::new(usize::MAX),
            ..Faults::default()
        });
        let dir = directory(&scene, adapter.clone());
        std::fs::write(scene.scratch.join("sink/key"), b"payload").unwrap();
        adapter.disarmed.store(false, Ordering::SeqCst);
        let stop = AtomicBool::new(false);
        let out = scene.outcomes("out", &Host);
        let hooks = Stop {
            at: Point::Buffer,
            stop: &stop,
        };
        let result = drive(
            &scene,
            &dir,
            &out,
            &Gate::default(),
            true,
            if fault { &publish::NoHooks } else { &hooks },
            &stop,
        );
        if fault {
            assert!(
                matches!(result,Err(Error::Run(f)) if f.reason==unityrs_batch::attempt::ReasonCode::HostIo)
            );
        } else {
            assert_eq!(result, Err(Error::Stopped));
        }
        assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
        assert!(!scene.scratch.join("sink/.unityrs-tmp-r-0").exists());
    }
}
struct FinalObstacle(bool);
impl unityrs_batch::sink::fs::FileSystem for FinalObstacle {
    fn release(&self) -> std::io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe(&self, _: &std::fs::File) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
    }
    fn entry(
        &self,
        parent: &std::fs::File,
        name: &std::ffi::CStr,
    ) -> std::io::Result<unityrs_batch::sys::Entry> {
        let mut entry = unityrs_batch::sys::rooted_entry(parent, name)?;
        entry.mount = self.mount(parent)?;
        Ok(entry)
    }
    fn mount(&self, file: &std::fs::File) -> std::io::Result<Option<u64>> {
        Ok(Some(file.metadata()?.dev()))
    }
    fn open(
        &self,
        parent: &std::fs::File,
        name: &std::ffi::CStr,
        flags: i32,
        beneath: bool,
    ) -> std::io::Result<std::fs::File> {
        if name == c"key" && flags & libc::O_ACCMODE == libc::O_RDONLY {
            return Err(std::io::Error::from_raw_os_error(if self.0 {
                libc::EXDEV
            } else {
                libc::EACCES
            }));
        }
        unityrs_batch::sys::rooted_open(parent, name, flags, beneath)
    }
    fn writeback(&self, _: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
    fn rename(
        &self,
        parent: &std::fs::File,
        from: &std::ffi::CStr,
        to: &std::ffi::CStr,
    ) -> std::io::Result<()> {
        if self.0 && to == c"key" {
            return Err(std::io::Error::from_raw_os_error(libc::EBUSY));
        }
        unityrs_batch::sys::rooted_rename(parent, from, to)
    }
}
#[test]
fn unreadable_final_destination_is_a_miss_and_final_mount_is_a_kind_failure() {
    for mount in [false, true] {
        let scene = Scene::new(&["key"], false, 4096);
        let root = scene.scratch.join("sink");
        std::fs::create_dir(&root).unwrap();
        let dir = Directory::preflight(
            &root,
            "r",
            1,
            Arc::new(FinalObstacle(mount)),
            &mut vec![0; 1_048_577],
        )
        .unwrap();
        std::fs::write(root.join("key"), b"payload").unwrap();
        let out = scene.outcomes("out", &Host);
        assert!(
            drive(
                &scene,
                &dir,
                &out,
                &Gate::default(),
                true,
                &publish::NoHooks,
                &AtomicBool::new(false)
            )
            .is_ok()
        );
        assert_eq!(
            out.loose(0).unwrap(),
            if mount {
                Code::PublishFailed
            } else {
                Code::Written
            }
        );
    }
}

struct NoObstacleOpen(AtomicUsize, &'static str, std::sync::atomic::AtomicU64);
impl unityrs_batch::sink::fs::FileSystem for NoObstacleOpen {
    fn release(&self) -> std::io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe(&self, _: &std::fs::File) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
    }
    fn entry(
        &self,
        parent: &std::fs::File,
        name: &std::ffi::CStr,
    ) -> std::io::Result<unityrs_batch::sys::Entry> {
        let mut entry = unityrs_batch::sys::rooted_entry(parent, name)?;
        entry.mount = self.mount(parent)?;
        if self.1 == "mount" {
            entry.mount = entry.mount.map(|m| m + 1);
        }
        Ok(entry)
    }
    fn mount(&self, file: &std::fs::File) -> std::io::Result<Option<u64>> {
        let meta = file.metadata()?;
        Ok(Some(
            meta.dev()
                + u64::from(self.1 == "mount" && meta.ino() == self.2.load(Ordering::SeqCst)),
        ))
    }
    fn open(
        &self,
        parent: &std::fs::File,
        name: &std::ffi::CStr,
        flags: i32,
        beneath: bool,
    ) -> std::io::Result<std::fs::File> {
        // O_PATH pins a name without invoking the inode's open handler.
        #[cfg(target_os = "linux")]
        if flags & libc::O_PATH != 0 {
            assert_ne!(flags & libc::O_NOFOLLOW, 0);
            return unityrs_batch::sys::rooted_open(parent, name, flags, beneath);
        }
        if name == c"key" && flags & libc::O_CREAT == 0 {
            self.0.fetch_add(1, Ordering::SeqCst);
            return Err(std::io::Error::from_raw_os_error(libc::ENXIO));
        }
        unityrs_batch::sys::rooted_open(parent, name, flags, beneath)
    }
    fn writeback(&self, _: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
}
fn resume_obstacle(kind: &'static str) {
    let () = unityrs_testkit::guard::finishes("resume obstacles", move || {
        let scene = Scene::new(&["key"], false, 4096);
        let root = scene.scratch.join("sink");
        std::fs::create_dir(&root).unwrap();
        let adapter = Arc::new(NoObstacleOpen(
            AtomicUsize::new(0),
            kind,
            std::sync::atomic::AtomicU64::new(0),
        ));
        let dir =
            Directory::preflight(&root, "r", 1, adapter.clone(), &mut vec![0; 1_048_577]).unwrap();
        let key = root.join("key");
        let _socket = if kind == "socket" {
            Some(std::os::unix::net::UnixListener::bind(&key).unwrap())
        } else if kind == "fifo" {
            let status =
                unityrs_testkit::child::status(std::process::Command::new("mkfifo").arg(&key))
                    .unwrap();
            assert!(status.success());
            None
        } else {
            std::fs::write(
                &key,
                if kind == "size" {
                    &b"short"[..]
                } else {
                    &b"payload"[..]
                },
            )
            .unwrap();
            None
        };
        adapter.2.store(
            std::fs::symlink_metadata(&key).unwrap().ino(),
            Ordering::SeqCst,
        );
        let out = scene.outcomes("obstacle", &Host);
        let result = drive(
            &scene,
            &dir,
            &out,
            &Gate::default(),
            true,
            &publish::NoHooks,
            &AtomicBool::new(false),
        );
        assert!(
            matches!(
                result,
                Ok(Admission::Drained(publish::Report { written: 1, .. }))
            ),
            "{kind}: {result:?}"
        );
        assert_eq!(adapter.0.load(Ordering::SeqCst), 0, "{kind} opened");
        assert_eq!(out.loose(0).unwrap(), Code::Written);
    });
}
#[test]
fn resume_rejects_socket_before_readable_open() {
    resume_obstacle("socket");
}
#[test]
fn resume_rejects_fifo_before_readable_open() {
    resume_obstacle("fifo");
}
#[test]
fn resume_checks_size_before_readable_open() {
    resume_obstacle("size");
}
#[test]
fn resume_checks_mount_before_readable_open() {
    resume_obstacle("mount");
}

#[path = "publication_support/resume.rs"]
mod resume_tests;
