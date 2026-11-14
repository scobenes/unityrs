use super::*;
use std::{
    fs,
    time::{Duration, Instant},
};
fn command(case: &str) -> std::io::Result<Command> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args([
        "--exact",
        "process_child",
        "--nocapture",
        "--test-threads=1",
    ])
    .env("B9_CASE", case)
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    Ok(cmd)
}
/// Three levels. The supervisor (a subreaper) runs the parent, which runs the
/// engine; the supervisor kills the parent, adopts the orphaned engine into
/// its scene, and reaps it. Every filesystem-mark wait watches the processes
/// that could still write the mark, so an outside SIGKILL at any level is a
/// scene retry, never a guard wait or an assertion. Marks live in the
/// supervisor scene's fresh scratch (`B9_ORPHAN`), so a retry sees none stale.
pub fn orphan(case: &str) {
    if case == "orphan_engine" {
        let folder = std::path::PathBuf::from(std::env::var_os("B9_ORPHAN").unwrap());
        sys::signals::block().unwrap();
        let process = Box::leak(Box::new(Process::prepare().unwrap()));
        sys::signals::install(process).unwrap();
        sys::signals::parent(
            process,
            Some(std::env::var("B9_PARENT").unwrap().parse().unwrap()),
        )
        .unwrap();
        // Renamed into place, so a reader never sees a partial pid.
        fs::write(folder.join("engine.tmp"), std::process::id().to_string()).unwrap();
        fs::rename(folder.join("engine.tmp"), folder.join("engine")).unwrap();
        let deadline = Instant::now() + guard::DEADLOCK_GUARD;
        while process.state.live().is_none() {
            assert!(
                Instant::now() < deadline,
                "deadlock guard: parent-death stop missing"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(process.state.live().unwrap().reason, StopReason::ParentDied);
        assert!(!process.state.second_signal());
        fs::write(folder.join("stopped"), b"").unwrap();
        // The supervisor's own death kills this engine with its group.
        let release = folder.join("release");
        let deadline = Instant::now() + guard::DEADLOCK_GUARD;
        while !release.exists() {
            assert!(
                Instant::now() < deadline,
                "deadlock guard: orphan release missing"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        process.exit(Code::Stopped);
    }
    if case == "orphan_parent" {
        let folder = std::path::PathBuf::from(std::env::var_os("B9_ORPHAN").unwrap());
        child::scene(|scene| -> std::io::Result<()> {
            // A retried scene's engine writes its pid afresh; the earlier
            // engine, killed, was reaped by this scene before the retry.
            let _ = fs::remove_file(folder.join("engine"));
            let mut cmd = command("orphan_engine")?;
            cmd.env("B9_PARENT", std::process::id().to_string());
            let engine = child::live(scene, &mut cmd, true)?;
            scene.mark(&folder.join("engine"), &[engine])?;
            fs::write(folder.join("parent_ready"), b"")?;
            loop {
                std::thread::park();
            }
        })
        .unwrap();
    }
    assert_eq!(case, "orphan_supervisor");
    sys::testing::subreaper().unwrap();
    child::scene(|scene| {
        let folder = scene.scratch.path().to_path_buf();
        let mut cmd = command("orphan_parent")?;
        cmd.env("B9_ORPHAN", &folder);
        let parent = child::live(scene, &mut cmd, true)?;
        scene.mark(&folder.join("parent_ready"), &[parent])?;
        scene.kill(parent)?;
        assert!(child::killed(scene.wait(parent)?));
        // The parent has exited, so its engine is now this subreaper's child,
        // and nothing but this scene reaps it: its pid cannot be reused.
        let pid: u32 = fs::read_to_string(folder.join("engine"))?.parse().unwrap();
        let engine = scene.adopt(pid, false)?;
        scene.mark(&folder.join("stopped"), &[engine])?;
        fs::write(folder.join("release"), b"")?;
        assert_eq!(scene.wait(engine)?.code(), Some(6));
        Ok(())
    })
    .unwrap();
}
#[test]
fn parent_thread_death_stops_and_supervisor_reaps_the_orphan() {
    child::scene(|scene| {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args([
            "--exact",
            "process_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("B9_CASE", "orphan_supervisor")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        let supervisor = child::live(scene, &mut cmd, false)?;
        assert!(scene.wait(supervisor)?.success());
        Ok(())
    })
    .unwrap();
}
#[test]
fn namespace_pid_one_handles_sigabrt() {
    child::scene(|scene| {
        let mut cmd = Command::new("unshare");
        cmd.args([
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--mount-proc",
        ])
        .arg(std::env::current_exe()?)
        .args([
            "--exact",
            "process_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("B9_CASE", "namespace_init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let wrapper = child::live(scene, &mut cmd, false)?;
        let (out, _) = child::drain(scene.take_stdout(wrapper).unwrap());
        let (err, _) = child::drain(scene.take_stderr(wrapper).unwrap());
        // Unsupported namespaces are a stated local skip; CI must exercise it.
        let mut ready = false;
        while let Ok(bytes) = out.recv_timeout(guard::DEADLOCK_GUARD) {
            if String::from_utf8_lossy(&bytes)
                .lines()
                .any(|s| s == "ready")
            {
                ready = true;
                break;
            }
        }
        if !ready {
            let status = scene.wait(wrapper)?;
            let stderr: Vec<u8> = err.try_iter().flatten().collect();
            assert!(
                std::env::var_os("CI").is_none(),
                "CI requires PID namespace acceptance: {status}: {}",
                String::from_utf8_lossy(&stderr)
            );
            eprintln!(
                "namespace scene skipped locally: {status}: {}",
                String::from_utf8_lossy(&stderr)
            );
            return Ok(());
        }
        send(scene, wrapper, "abort\n");
        assert_eq!(scene.wait(wrapper)?.code(), Some(134));
        Ok(())
    })
    .unwrap();
}
/// Send until the engine's stdin watch ends. One datagram's ICMP error is
/// enough, but a resend covers a datagram another socket received, and this
/// shared description's own send consuming a pending error (`ECONNREFUSED`,
/// ignored) before the engine reads it. The engine's end returns at once.
fn ended(
    scene: &mut child::Scene,
    engine: child::Live,
    socket: &std::net::UdpSocket,
    err: &std::sync::mpsc::Receiver<Vec<u8>>,
) -> std::io::Result<()> {
    let deadline = Instant::now() + guard::DEADLOCK_GUARD;
    let mut bytes = Vec::new();
    loop {
        let _ = socket.send(b"x");
        while let Ok(more) = err.recv_timeout(Duration::from_millis(100)) {
            bytes.extend(more);
        }
        if String::from_utf8_lossy(&bytes)
            .lines()
            .any(|s| s == "stdin watch ended")
        {
            return Ok(());
        }
        if !scene.running(engine)? {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        assert!(
            Instant::now() < deadline,
            "deadlock guard: stdin watch never ended"
        );
    }
}
#[test]
fn udp_watch_ends_without_a_stop_or_lifecycle_allocation() {
    use std::{
        net::UdpSocket,
        os::fd::{AsRawFd, OwnedFd},
    };
    child::scene(|scene| {
        // The closed port is on 127.0.0.2, so neither this test's own socket
        // (127.0.0.1) nor another test's 127.0.0.1 bind can take it and turn
        // the ICMP error into a delivered datagram.
        let closed = UdpSocket::bind("127.0.0.2:0")?;
        let address = closed.local_addr()?;
        drop(closed);
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.connect(address)?;
        let flags = sys::testing::flags(socket.as_raw_fd())?;
        let inherited: OwnedFd = socket.try_clone()?.into();
        let release = scene.scratch.join("release");
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args([
            "--exact",
            "process_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("B9_CASE", "udp")
        .env("B9_RELEASE", &release)
        .stdin(Stdio::from(inherited))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let engine = child::live(scene, &mut cmd, false)?;
        let (out, _) = child::drain(scene.take_stdout(engine).unwrap());
        let (err, _) = child::drain(scene.take_stderr(engine).unwrap());
        line(&out, "marked");
        ended(scene, engine, &socket, &err)?;
        assert!(scene.running(engine)?);
        fs::write(release, b"")?;
        assert_eq!(scene.wait(engine)?.code(), Some(0));
        assert_eq!(sys::testing::flags(socket.as_raw_fd())?, flags);
        Ok(())
    })
    .unwrap();
}
