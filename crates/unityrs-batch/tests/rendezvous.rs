#![cfg(feature = "test-hooks")]
//! Cancellation must release a child even before spawn can register it.
use std::{
    io::{Read, Write},
    sync::mpsc,
    time::Duration,
};
use unityrs_batch::sys::testing;
use unityrs_testkit::{child, guard};

#[test]
fn cancellation_releases_the_pre_exec_child() {
    for panic in [false, true] {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "rendezvous_child"]);
        let socket = testing::hold_before_exec(&mut cmd, guard::DEADLOCK_GUARD).unwrap();
        // A rescue endpoint lets the unfixed regression reap its child too.
        let mut rescue = socket.stream().try_clone().unwrap();
        let (tx, rx) = mpsc::channel();
        let launcher = std::thread::spawn(move || {
            tx.send(child::status(&mut cmd)).unwrap();
        });
        let mut ready = [0];
        socket.stream().read_exact(&mut ready).unwrap();
        assert_eq!(ready, [1]);
        let unwound = std::panic::catch_unwind(move || {
            let _socket = socket;
            assert!(!panic, "injected replying-thread panic");
        });
        assert_eq!(unwound.is_err(), panic);
        let done = rx.recv_timeout(guard::DEADLOCK_GUARD);
        if done.is_err() {
            let _ = rescue.write_all(&[1]);
            let _ = guard::recv(&rx, "rescue unfixed rendezvous");
        }
        launcher.join().unwrap();
        let error = done
            .expect("cancellation left spawn blocked")
            .expect_err("child execed after cancellation");
        assert_eq!(error.raw_os_error(), Some(libc::ECANCELED));
    }
}

#[test]
fn rendezvous_child() {}

#[test]
fn an_unanswered_child_times_out_before_exec() {
    let error = guard::finishes("child receive timeout", || {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "rendezvous_child"]);
        // Exercise the timeout itself with a short injected bound. The outer
        // guard still allows a loaded runner the full testkit deadlock guard.
        let socket = testing::hold_before_exec(
            &mut cmd,
            guard::DEADLOCK_GUARD.min(Duration::from_millis(50)),
        )
        .unwrap();
        let result = child::status(&mut cmd);
        drop(socket);
        result.expect_err("unanswered child execed")
    });
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
        "{error}"
    );
}
