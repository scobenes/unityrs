#![cfg(feature = "test-hooks")]
//! A fork must not extend either a returned or a failed sink lease.
use std::{
    ffi::CStr,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use unityrs_batch::{
    manifest::head::HEAD_LIMIT,
    sink::{Class, directory::Directory, fs::FileSystem},
    sys,
};
use unityrs_testkit::{child, guard, scratch::Scratch};

struct ForkAtRun {
    socket: sys::testing::Rendezvous,
    acquired: mpsc::Sender<()>,
    seen: AtomicBool,
    fail: bool,
}
impl FileSystem for ForkAtRun {
    fn release(&self) -> io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe(&self, _: &File) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
    fn probe_sink_semantics(&self) -> bool {
        false
    }
    fn mount(&self, file: &File) -> io::Result<Option<u64>> {
        Ok(Some(file.metadata()?.dev()))
    }
    fn open(&self, parent: &File, name: &CStr, flags: i32, beneath: bool) -> io::Result<File> {
        // Opening the run is the first operation after the sink takes its lock.
        if name == c"r" && !self.seen.swap(true, Ordering::SeqCst) {
            self.acquired.send(()).unwrap();
            let mut ready = [0];
            self.socket.stream().read_exact(&mut ready)?;
            assert_eq!(ready, [1]);
            if self.fail {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
        }
        sys::rooted_open(parent, name, flags, beneath)
    }
}

#[test]
fn sink_lock_is_released_while_an_unrelated_child_is_before_exec() {
    let () = guard::finishes("sink lock inherited across fork", || {
        for fail in [false, true] {
            child::scene(|scene| {
                let scratch = Scratch::new("sink-lock-fork");
                let path = scratch.path().to_owned();
                let mut cmd = std::process::Command::new(std::env::current_exe()?);
                cmd.args(["--exact", "rendezvous_child"]);
                let socket = sys::testing::hold_before_exec(&mut cmd, guard::DEADLOCK_GUARD)?;
                let (acquired, held) = mpsc::channel();
                let adapter = Arc::new(ForkAtRun {
                    socket,
                    acquired,
                    seen: AtomicBool::new(false),
                    fail,
                });
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let first = Directory::preflight(
                        &path,
                        "r",
                        1,
                        adapter.clone(),
                        &mut vec![0; HEAD_LIMIT + 1],
                    )
                    .map(drop);
                    // The child still holds the description and cannot exec.
                    let next = Directory::preflight(
                        &path,
                        "r",
                        1,
                        adapter.clone(),
                        &mut vec![0; HEAD_LIMIT + 1],
                    )
                    .map(drop);
                    adapter.socket.stream().write_all(&[1]).unwrap();
                    tx.send((first, next)).unwrap();
                });
                guard::wait(&held, "preflight owns the sink lock");
                let inode = fs::metadata(scratch.join("_unityrs/lock"))?.ino();
                let live = child::live(scene, &mut cmd, false)?;
                assert!(scene.wait(live)?.success());
                let (first, next) = guard::recv(&rx, "pre-exec sink child");
                if fail {
                    assert_eq!(first.unwrap_err().class, Class::SinkRootUnusable);
                } else {
                    first.unwrap();
                }
                next.unwrap();
                assert_eq!(fs::metadata(scratch.join("_unityrs/lock"))?.ino(), inode);
                Ok(())
            })
            .unwrap();
        }
    });
}

#[test]
fn rendezvous_child() {}
