//! Isolated process primitives. Engine integration/manifest acceptance belongs
//! to the next piece; these children deliberately retain owners after reset.
#![cfg(unix)]
use std::{
    io::{self, BufRead, Write},
    process::{Command, Stdio},
    sync::mpsc::Receiver,
};
use unityrs_batch::{
    lifetime::{Process, Stop},
    reason::{Code, StopReason},
    sys,
};
use unityrs_testkit::{child, guard};

struct NoLifecycleAllocations;
static ALLOCATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DEALLOCATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
impl unityrs_testkit::counted::Tally for NoLifecycleAllocations {
    fn refuses(&self, _: usize) -> bool {
        unityrs_testkit::counted::counted()
    }
    fn grew(&self, _: usize, _: bool) {
        ALLOCATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn shrank(&self, _: usize) {
        DEALLOCATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
#[global_allocator]
static ALLOC: unityrs_testkit::counted::Allocator<NoLifecycleAllocations> =
    unityrs_testkit::counted::Allocator(NoLifecycleAllocations);

fn line(rx: &Receiver<Vec<u8>>, wanted: &str) {
    let mut bytes = Vec::new();
    loop {
        bytes.extend(guard::recv(rx, "engine protocol line"));
        if String::from_utf8_lossy(&bytes).lines().any(|s| s == wanted) {
            return;
        }
    }
}
fn spawn(
    scene: &mut child::Scene,
    case: &str,
    address: Option<std::net::SocketAddr>,
) -> io::Result<(child::Live, Receiver<Vec<u8>>)> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "process_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("B9_CASE", case)
        .env("B9_TRACE", scene.scratch.join("lifecycle"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(address) = address {
        command.env("B9_ADDRESS", address.to_string());
    }
    let live = child::live(scene, &mut command, false)?;
    let (out, _) = child::drain(scene.take_stdout(live).unwrap());
    let (_err, _) = child::drain(scene.take_stderr(live).unwrap());
    Ok((live, out))
}
fn send(scene: &mut child::Scene, live: child::Live, text: &str) {
    scene
        .stdin(live)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}
#[test]
#[allow(clippy::too_many_lines)] // Isolated child protocol branches.
fn process_child() {
    let Ok(case) = std::env::var("B9_CASE") else {
        return;
    };
    #[cfg(target_os = "linux")]
    if case.starts_with("orphan_") {
        lifetime_support::orphan(&case);
        return;
    }
    if case == "panic_slot" {
        unityrs_batch::diagnostic::panic::install();
        let _ = std::panic::catch_unwind(|| panic!("{}", "é".repeat(2048)));
        let slot = unityrs_batch::diagnostic::panic::read();
        assert!(slot.cut);
        assert!(slot.len <= 1024);
        std::str::from_utf8(&slot.bytes[..slot.len]).unwrap();
        let _ = std::panic::catch_unwind(|| panic!("fresh payload"));
        let slot = unityrs_batch::diagnostic::panic::read();
        assert!(!slot.cut);
        assert!(
            std::str::from_utf8(&slot.bytes[..slot.len])
                .unwrap()
                .ends_with("fresh payload")
        );
        return;
    }
    if case.starts_with("reset_") {
        let table = sys::reset::SocketTable::new();
        let socket = table.socket(0, false).unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&socket);
        assert!(sys::test_zero_linger(fd).unwrap());
        let result = socket.connect(std::env::var("B9_ADDRESS").unwrap().parse().unwrap());
        #[cfg(target_os = "linux")]
        if case == "reset_connecting" {
            assert_eq!(
                result.as_ref().unwrap_err().raw_os_error(),
                Some(libc::EINPROGRESS)
            );
        }
        assert!(
            result.is_ok()
                || result
                    .as_ref()
                    .is_err_and(|e| e.raw_os_error() == Some(libc::EINPROGRESS))
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let registration = socket.register(runtime.handle()).unwrap();
        if case == "reset_connected" {
            runtime.block_on(registration.connected()).unwrap();
        }
        #[cfg(target_os = "linux")]
        let before = std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap();
        println!("\nready");
        let mut input = String::new();
        std::io::stdin().lock().read_line(&mut input).unwrap();
        table.reset();
        #[cfg(target_os = "linux")]
        {
            assert!(registration.disconnected().unwrap());
            assert_eq!(
                std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap(),
                before
            );
            runtime.block_on(registration.readable()).unwrap();
            assert_eq!(table.reset_errors(), 0);
        }
        assert!(sys::test_socket_live(fd));
        // Drop must retain a sealed generation; a second scan still uses that
        // same live description, with no runtime/owner lifetime violation.
        drop(registration);
        assert!(sys::test_socket_live(fd));
        table.reset();
        println!("reset held");
        input.clear();
        std::io::stdin().lock().read_line(&mut input).unwrap();
        return;
    }
    if case == "inherited" {
        sys::testing::ignore(libc::SIGINT).unwrap();
        sys::testing::ignore(libc::SIGHUP).unwrap();
    }
    sys::signals::block().unwrap();
    if case == "inherited" {
        sys::testing::raise(libc::SIGTERM).unwrap();
    }
    let process: &'static Process = Box::leak(Box::new(Process::prepare().unwrap()));
    if case == "init_hook" {
        sys::signals::install_as_init(process).unwrap();
    } else {
        sys::signals::install(process).unwrap();
    }
    sys::signals::parent(
        process,
        if case == "parent" {
            Some(4_194_304)
        } else {
            None
        },
    )
    .unwrap();
    if case == "inherited" {
        assert_eq!(process.state.live().unwrap().reason, StopReason::Sigterm);
        sys::testing::raise(libc::SIGINT).unwrap();
        sys::testing::raise(libc::SIGHUP).unwrap();
        assert_eq!(process.state.live().unwrap().reason, StopReason::Sigterm);
        process.exit(Code::Complete);
    }
    if case == "parent" {
        assert_eq!(process.state.live().unwrap().reason, StopReason::ParentDied);
        assert!(!process.state.second_signal());
        process.exit(Code::Stopped);
    }
    if case == "signals" {
        process.request(Stop::external(StopReason::StdinEof));
        process.state.store_final(Code::Host);
        println!("\nready");
        let mut input = String::new();
        std::io::stdin().lock().read_line(&mut input).unwrap();
        assert_eq!(process.state.live().unwrap().reason, StopReason::StdinEof);
        // The first SIGTERM was SENT before that line, but its handler may not
        // have run yet: a process-directed signal can wait for another thread
        // to be scheduled. Say "first handled" only once it has counted, or the
        // test's second SIGTERM can find the first still pending and merge
        // with it (standard signals do not queue), leaving one count, no exit.
        let deadline = std::time::Instant::now() + guard::DEADLOCK_GUARD;
        while process.state.handled_signals() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "deadlock guard: first SIGTERM never handled"
            );
            std::thread::yield_now();
        }
        assert_eq!(process.state.handled_signals(), 1);
        println!("first handled");
        loop {
            std::thread::park();
        }
    }
    if matches!(case.as_str(), "abort" | "init_hook" | "namespace_init") {
        println!("\nready");
        if case != "abort" {
            let mut input = String::new();
            std::io::stdin().lock().read_line(&mut input).unwrap();
            sys::testing::raise_abort();
        }
        loop {
            std::thread::park();
        }
    }
    #[cfg(target_os = "linux")]
    {
        use unityrs_batch::lifetime::{Policy, StdinWatch};
        let mut policy = Policy::at(None, 10_000, 0, sys::boottime().unwrap()).unwrap();
        if case == "deadline" {
            policy.cutoff = Some(unityrs_batch::lifetime::Boot::now().unwrap());
        }
        // Isolate the hard-exit mechanism without spending ten seconds in each
        // ordinary unit run. Minimum-grace arithmetic has deterministic tests.
        if case == "hard" {
            process
                .state
                .arm(unityrs_batch::lifetime::Boot::now().unwrap());
        }
        let controller = sys::Wake::new().unwrap();
        let finisher = sys::Wake::new().unwrap();
        let warning = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if case == "udp" {
            let release = std::path::PathBuf::from(std::env::var_os("B9_RELEASE").unwrap());
            let warning = warning.clone();
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + guard::DEADLOCK_GUARD;
                while !release.exists() {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "deadlock guard: UDP release missing"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                process.exit(if warning.load(std::sync::atomic::Ordering::SeqCst) {
                    Code::Complete
                } else {
                    Code::Internal
                });
            });
        }
        let writer = unityrs_batch::diagnostic::Writer::prepare().unwrap();
        let credentials = std::sync::OnceLock::new();
        let secrets = unityrs_batch::diagnostic::Secrets {
            values: &[],
            credentials: &credentials,
        };
        if matches!(case.as_str(), "stdin" | "udp") {
            // Its own thread, counted before the lifecycle's mark: the stdin
            // watch allocates nothing either. An atomic, not a channel, says
            // so, since a channel send could allocate after the count.
            let stdin = StdinWatch::prepare(process, &warning, &writer, secrets).unwrap();
            let counting = std::sync::atomic::AtomicBool::new(false);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    unityrs_testkit::counted::count_this_thread();
                    counting.store(true, std::sync::atomic::Ordering::SeqCst);
                    stdin.run();
                });
                while !counting.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                lifecycle(process, policy, &controller, &finisher, &case)
            });
        }
        lifecycle(process, policy, &controller, &finisher, &case);
    }
}
/// Run the lifecycle thread's loop on this thread, as the counted case asks.
#[cfg(target_os = "linux")]
fn lifecycle(
    process: &'static Process,
    mut policy: unityrs_batch::lifetime::Policy,
    controller: &sys::Wake,
    finisher: &sys::Wake,
    case: &str,
) -> ! {
    {
        let mut watch =
            unityrs_batch::lifetime::Watch::prepare(process, policy, controller, finisher).unwrap();
        watch.mark(|| {
            unityrs_testkit::counted::count_this_thread();
            sys::lifecycle::test_mark();
        });
        if case == "timer_failure" {
            policy.cutoff = Some(unityrs_batch::lifetime::Boot(
                unityrs_batch::lifetime::Boot::now().unwrap().0 + 1000,
            ));
            watch.policy = policy;
            watch.fail_timer();
        }
        if case == "signal_watch" {
            watch
                .trace(&std::path::PathBuf::from(
                    std::env::var_os("B9_TRACE").unwrap(),
                ))
                .unwrap();
        }
        if case == "hard" {
            process.request(Stop::external(StopReason::Deadline));
        }
        println!("\nready");
        watch.run();
    }
}

#[test]
fn sigabrt_keeps_ordinary_process_termination() {
    use std::os::unix::process::ExitStatusExt;
    child::scene(|scene| {
        let (live, out) = spawn(scene, "abort", None)?;
        line(&out, "ready");
        scene.signal(live, libc::SIGABRT)?;
        assert_eq!(scene.wait(live)?.signal(), Some(libc::SIGABRT));
        Ok(())
    })
    .unwrap();
}
#[test]
fn sigabrt_as_init_uses_explicit_134() {
    child::scene(|scene| {
        let (live, out) = spawn(scene, "init_hook", None)?;
        line(&out, "ready");
        send(scene, live, "abort\n");
        assert_eq!(scene.wait(live)?.code(), Some(134));
        Ok(())
    })
    .unwrap();
}
#[test]
fn panic_slots_and_inherited_pending_signals_are_isolated() {
    for case in ["panic_slot", "inherited"] {
        child::scene(|scene| {
            let (live, out) = spawn(scene, case, None)?;
            let status = scene.wait(live)?;
            let output: Vec<u8> = out.try_iter().flatten().collect();
            assert_eq!(
                status.code(),
                Some(0),
                "case={case}: {}",
                String::from_utf8_lossy(&output)
            );
            Ok(())
        })
        .unwrap();
    }
}

#[cfg(target_os = "linux")]
mod lifetime_support;

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's scale job does"
)]
fn lifecycle_stops_allocate_nothing_after_the_mark() {
    for (case, code) in [
        ("deadline", 6),
        ("stdin", 6),
        ("signal_watch", 6),
        ("timer_failure", 70),
    ] {
        child::scene(|scene| {
            let trace =
                (case == "signal_watch").then(|| child::Fifo::new(scene, "lifecycle").unwrap());
            let (live, out) = spawn(scene, case, None)?;
            line(&out, "marked");
            if case == "stdin" {
                drop(scene.take_stdin(live));
            }
            if case == "signal_watch" {
                scene.signal(live, libc::SIGTERM)?;
                let trace = trace.as_ref().unwrap();
                assert_eq!(
                    trace.line_while(scene, live)?,
                    "lifecycle_pipe_read reached"
                );
                assert_eq!(
                    trace.line_while(scene, live)?,
                    "lifecycle_stop_woken reached"
                );
            }
            assert_eq!(
                scene.wait(live)?.code(),
                Some(code),
                "{case}: any lifecycle allocation aborts this child"
            );
            Ok(())
        })
        .unwrap();
    }
}

/// Accept the engine's connection with no unbounded wait: a nonblocking
/// accept under the deadlock guard that returns at once when the engine has
/// ended, so its outside SIGKILL reaches scene arbitration. (`"ready"` in the
/// connecting case proves only `EINPROGRESS`, not a queued connection.)
#[cfg(target_os = "linux")]
fn accept(
    scene: &mut child::Scene,
    live: child::Live,
    listener: &std::net::TcpListener,
) -> io::Result<std::net::TcpStream> {
    listener.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + guard::DEADLOCK_GUARD;
    loop {
        match listener.accept() {
            Ok((peer, _)) => {
                peer.set_nonblocking(false)?;
                return Ok(peer);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
        if !scene.running(live)? {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "engine ended before its connection was accepted",
            ));
        }
        assert!(
            std::time::Instant::now() < deadline,
            "deadlock guard: engine connection never accepted"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
/// The ECONNRESET acceptance test: the peer sees RST while the child is HELD
/// AFTER reset, before process exit or close. The connecting case has issued
/// nonblocking connect, without ever awaiting its completion. True `SYN_SENT`
/// has no peer socket (`sys/reset_tests.rs` tests it separately).
#[test]
#[cfg(target_os = "linux")]
fn peer_econnreset_after_af_unspec_reset_while_child_is_held() {
    use std::io::Read;
    use std::net::TcpListener;
    use unityrs_testkit::guard::DEADLOCK_GUARD;
    for case in ["reset_connected", "reset_connecting"] {
        child::scene(|scene| {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let (live, out) = spawn(scene, case, Some(listener.local_addr()?))?;
            line(&out, "ready");
            let mut peer = accept(scene, live, &listener)?;
            peer.set_read_timeout(Some(DEADLOCK_GUARD))?;
            send(scene, live, "reset\n");
            line(&out, "reset held");
            assert!(
                scene.running(live)?,
                "reset evidence must precede engine exit"
            );
            assert_eq!(
                peer.read(&mut [0; 1]).unwrap_err().raw_os_error(),
                Some(libc::ECONNRESET)
            );
            assert!(scene.running(live)?);
            send(scene, live, "release\n");
            assert!(scene.wait(live)?.success());
            Ok(())
        })
        .unwrap();
    }
}
#[test]
fn parent_check_is_a_stop_and_only_handled_signals_count() {
    child::scene(|scene| {
        let (live, _) = spawn(scene, "parent", None)?;
        assert_eq!(scene.wait(live)?.code(), Some(6));
        Ok(())
    })
    .unwrap();
    child::scene(|scene| {
        let (live, out) = spawn(scene, "signals", None)?;
        line(&out, "ready");
        scene.signal(live, libc::SIGTERM)?;
        send(scene, live, "check first\n");
        line(&out, "first handled");
        assert!(scene.running(live)?);
        scene.signal(live, libc::SIGTERM)?;
        assert_eq!(scene.wait(live)?.code(), Some(74));
        Ok(())
    })
    .unwrap();
}
#[test]
#[cfg(target_os = "linux")]
fn hard_exit_does_not_wait_for_owners() {
    child::scene(|scene| {
        let (live, _) = spawn(scene, "hard", None)?;
        assert_eq!(scene.wait(live)?.code(), Some(6));
        Ok(())
    })
    .unwrap();
}
