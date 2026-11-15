//! An isolated subreaper owns even a child orphaned while spawn is blocked.
use std::{
    fs::{self, File},
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::Command,
};
use unityrs_testkit::{child, guard};

fn command() -> io::Result<Command> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args([
        "--exact",
        "sys::testing::tests::parent_gone_child",
        "--nocapture",
    ]);
    Ok(cmd)
}

#[test]
fn parent_death_releases_the_pre_exec_child() {
    let () = guard::finishes("orphaned pre-exec child", || {
        child::scene(|scene| {
            // Own the group here. Inner scenes inherit it, so observing the
            // dead parent cannot kill the orphan before it handles EOF.
            let mut cmd = command()?;
            cmd.env("UNITYRS_RENDEZVOUS_CASE", "supervisor");
            let supervisor = child::live(scene, &mut cmd, false)?;
            assert!(scene.wait(supervisor)?.success());
            Ok(())
        })
        .unwrap();
    });
}

#[test]
fn parent_gone_child() {
    let Ok(case) = std::env::var("UNITYRS_RENDEZVOUS_CASE") else {
        return;
    };
    if case == "parent" {
        let dir = PathBuf::from(std::env::var_os("UNITYRS_RENDEZVOUS_DIR").unwrap());
        let pid_file = File::create(dir.join("pid")).unwrap();
        let mut cmd = command().unwrap();
        cmd.env_remove("UNITYRS_RENDEZVOUS_CASE");
        // Register before the rendezvous: the supervisor needs the orphan's
        // pid even though spawn cannot return it to the parent yet.
        // SAFETY: only async-signal-safe getpid/write and errno reads; the
        // captured file stays live until fork, with no child-side destructor.
        unsafe {
            cmd.pre_exec(move || {
                let pid = libc::getpid();
                let n = super::super::retry_interrupted(|| {
                    let n = libc::write(
                        pid_file.as_raw_fd(),
                        (&raw const pid).cast(),
                        size_of::<i32>(),
                    );
                    if n == -1 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(n)
                    }
                })?;
                if n == 4 {
                    Ok(())
                } else {
                    Err(io::ErrorKind::WriteZero.into())
                }
            });
        }
        let socket = super::hold_before_exec(&mut cmd, guard::DEADLOCK_GUARD).unwrap();
        std::thread::spawn(move || {
            let mut ready = [0];
            socket.stream().read_exact(&mut ready).unwrap();
            assert_eq!(ready, [1]);
            fs::write(dir.join("ready"), b"").unwrap();
            // Abrupt parent death runs no rendezvous Drop or scene cleanup.
            std::process::exit(0);
        });
        child::scene(|scene| -> io::Result<()> {
            let _ = child::live(scene, &mut cmd, false)?;
            panic!("orphan execed instead of waiting for its parent");
        })
        .unwrap();
    } else {
        assert_eq!(case, "supervisor");
        super::subreaper().unwrap();
        child::scene(|scene| {
            let dir = scene.scratch.path().to_owned();
            let mut cmd = command()?;
            cmd.env("UNITYRS_RENDEZVOUS_CASE", "parent")
                .env("UNITYRS_RENDEZVOUS_DIR", &dir);
            let parent = child::live(scene, &mut cmd, false)?;
            assert!(scene.wait(parent)?.success());
            assert!(dir.join("ready").is_file());
            let bytes: [u8; 4] = fs::read(dir.join("pid"))?.try_into().unwrap();
            let pid = u32::from_ne_bytes(bytes);
            let orphan = scene.adopt(pid, false)?;
            // The spawn-error path must fail before exec. Rust may abort
            // when the dead parent can no longer read its errno pipe.
            assert!(!scene.wait(orphan)?.success());
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn interruptions_fail_before_exec() {
    let () = guard::finishes("interrupted pre-exec receive", || {
        // Install the process-wide handler only in an isolated test process.
        if std::env::var_os("UNITYRS_RENDEZVOUS_INTERRUPTED").is_none() {
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "sys::testing::tests::interruptions_fail_before_exec",
                "--nocapture",
            ])
            .env("UNITYRS_RENDEZVOUS_INTERRUPTED", "1");
            assert!(child::status(&mut cmd).unwrap().success());
            return;
        }
        interrupted_receive();
    });
}

fn interrupted_receive() {
    use std::{
        os::{fd::FromRawFd, unix::net::UnixStream},
        sync::mpsc,
        time::{Duration, Instant},
    };
    extern "C" fn handler(_: libc::c_int) {}

    // SAFETY: initialized sigaction/mask, valid signal, and an empty handler
    // that uses no locks or allocation. No SA_RESTART; install before fork.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler as *const () as usize;
        assert_eq!(libc::sigemptyset(&raw mut action.sa_mask), 0);
        assert_eq!(
            libc::sigaction(libc::SIGUSR1, &raw const action, std::ptr::null_mut()),
            0
        );
        let mut mask = action.sa_mask;
        assert_eq!(libc::sigaddset(&raw mut mask, libc::SIGUSR1), 0);
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const mask, std::ptr::null_mut()),
            0
        );
    }
    let (mut pid_reader, pid_writer) = UnixStream::pair().unwrap();
    pid_reader
        .set_read_timeout(Some(guard::DEADLOCK_GUARD))
        .unwrap();
    let mut cmd = command().unwrap();
    cmd.env_remove("UNITYRS_RENDEZVOUS_CASE");
    // SAFETY: only async-signal-safe getpid/write and errno reads. The stream
    // stays live until fork; the spawn error path exits without destructors.
    unsafe {
        cmd.pre_exec(move || {
            let pid = libc::getpid();
            if libc::write(
                pid_writer.as_raw_fd(),
                (&raw const pid).cast(),
                size_of::<i32>(),
            ) == -1
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let socket = super::hold_before_exec(&mut cmd, guard::DEADLOCK_GUARD).unwrap();
    let (done, completed) = mpsc::channel();
    let interrupter = std::thread::spawn(move || {
        let mut bytes = [0; 4];
        pid_reader.read_exact(&mut bytes).unwrap();
        let pid = i32::from_ne_bytes(bytes);
        let mut ready = [0];
        socket.stream().read_exact(&mut ready).unwrap();
        assert_eq!(ready, [1]);
        // SAFETY: scalar pidfd_open on the held child. The owned fd below
        // keeps subsequent signals safe even after spawn has reaped the pid.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(fd >= 0, "{}", io::Error::last_os_error());
        // SAFETY: pidfd_open returned a fresh descriptor owned by this thread.
        let pidfd = unsafe { File::from_raw_fd(fd.try_into().unwrap()) };
        // The ready byte precedes recv. Observe the syscall itself before
        // signalling, so even a descheduled child cannot miss the interruption.
        let syscall = format!("/proc/{pid}/syscall");
        let recv = libc::SYS_recvfrom.to_string();
        let deadline = Instant::now() + guard::DEADLOCK_GUARD;
        while fs::read_to_string(&syscall)
            .unwrap()
            .split_whitespace()
            .next()
            != Some(recv.as_str())
        {
            assert!(Instant::now() < deadline, "child never entered recv");
            std::thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..20 {
            // SAFETY: live pidfd, valid signal, no siginfo pointer or flags.
            let sent = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    libc::SIGUSR1,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
            if sent == -1 {
                assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
                guard::wait(&completed, "interrupted spawn result");
                return;
            }
            // This interval is the signal cadence, not a scheduler deadline.
            // Keep interrupting a retrying child until spawn answers.
            if completed
                .recv_timeout(guard::DEADLOCK_GUARD.min(Duration::from_millis(5)))
                .is_ok()
            {
                return;
            }
        }
        // Rescue the retrying regression too: Drop shuts down the socket, so
        // its spawn finishes with ECANCELED instead of leaking a held child.
    });
    let result = child::status(&mut cmd);
    let _ = done.send(());
    interrupter.join().unwrap();
    let error = result.expect_err("interrupted child execed");
    assert_eq!(error.raw_os_error(), Some(libc::EINTR));
}
