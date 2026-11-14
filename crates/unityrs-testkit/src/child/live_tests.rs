use super::*;
use crate::child;
#[test]
fn deliberate_kill_does_not_retry_and_fifo_is_drained_before_wait() {
    let mut attempts = 0;
    child::scene(|scene| {
        attempts += 1;
        let fifo = Fifo::new(scene, "trace")?;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'ready\\n' > \"$1\"; exec cat")
            .arg("sh")
            .arg(fifo.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null());
        let live = child::live(scene, &mut cmd, false)?;
        assert_eq!(fifo.line(), "ready");
        scene.kill(live)?;
        assert!(super::super::killed(scene.wait(live)?));
        Ok(())
    })
    .unwrap();
    assert_eq!(attempts, 1);
}
#[test]
fn outside_kill_rebuilds_servers_fifos_and_scratch_even_after_error() {
    let mut attempts = 0;
    let mut old = None;
    child::scene(|scene| {
        attempts += 1;
        if let Some(path) = &old {
            assert!(!Path::new(path).exists());
        }
        old = Some(scene.scratch.path().to_path_buf());
        let _fifo = Fifo::new(scene, "trace")?;
        let mut server = Command::new("sh");
        server
            .arg("-c")
            .arg("exec cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null());
        let server = child::live(scene, &mut server, true)?;
        assert!(scene.running(server)?);
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(if attempts == 1 {
            "kill -KILL $$"
        } else {
            "exit 0"
        });
        let live = child::live(scene, &mut cmd, false)?;
        let status = scene.wait(live)?;
        assert!(
            status.success(),
            "outside kill should rebuild even after assertion"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(attempts, 2);
}

#[test]
fn discarded_observation_keeps_draining_until_the_child_ends() {
    use std::io::Write;
    child::scene(|scene| {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf 'ready\\n'; IFS= read -r release; head -c 131072 /dev/zero")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let engine = child::live(scene, &mut command, false)?;
        let (observations, drainer) = child::drain(scene.take_stdout(engine).unwrap());
        let _ready = crate::guard::recv(&observations, "child before dropped observation");
        drop(observations);
        scene.stdin(engine).unwrap().write_all(b"release\n")?;
        assert!(scene.wait(engine)?.success());
        drainer.join().unwrap();
        Ok(())
    })
    .unwrap();
}
#[test]
fn a_mark_wait_returns_when_a_watched_child_ends_first() {
    child::scene(|scene| {
        let mark = scene.scratch.join("mark");
        let mut writer = Command::new("sh");
        writer
            .arg("-c")
            .arg("touch \"$1\"; exec cat")
            .arg("sh")
            .arg(&mark);
        writer.stdin(std::process::Stdio::piped());
        let writer = child::live(scene, &mut writer, false)?;
        scene.mark(&mark, &[writer])?;
        drop(scene.take_stdin(writer));
        assert!(scene.wait(writer)?.success());
        let mut quitter = Command::new("sh");
        quitter.arg("-c").arg("exit 0");
        let quitter = child::live(scene, &mut quitter, false)?;
        let error = scene
            .mark(&scene.scratch.join("never"), &[quitter])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        Ok(())
    })
    .unwrap();
}
#[test]
fn only_a_child_of_this_process_can_be_adopted() {
    child::scene(|scene| {
        // Process 1 is never this test's child: waitid refuses it (ECHILD).
        assert!(scene.adopt(1, false).is_err());
        Ok(())
    })
    .unwrap();
}
