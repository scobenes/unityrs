//! The main thread's writers on pipes of the test's own: [`within_on`],
//! which [`super::early::within`] runs on stdout and stderr, written within
//! its wait and held past it; and the non-blocking loop's budget, checked
//! before every try.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use super::early::{nonblocking, through_thread, within_on};

/// A pipe: its read end, and its write end as a `File`.
fn pipe() -> (std::io::PipeReader, File) {
    let (r, w) = std::io::pipe().unwrap();
    (r, File::from(OwnedFd::from(w)))
}

/// Fills the pipe of `w` through a non-blocking description of its own,
/// as another writer would, leaving `w`'s blocking, and keeps it.
fn fill(w: &File) -> File {
    let other = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}", w.as_raw_fd()))
        .unwrap();
    // Blocks, then single bytes, so not one byte of room is left.
    for block in [&[b'x'; 4096][..], b"x"] {
        loop {
            match (&other).write(block) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
    }
    other
}

fn read_all(mut r: std::io::PipeReader) -> Vec<u8> {
    let mut got = Vec::new();
    r.read_to_end(&mut got).unwrap();
    got
}

/// Both lines, each on its own descriptor, written within the wait.
#[test]
fn within_writes_both_lines_within_its_wait() {
    let (out_r, out_w) = pipe();
    let (err_r, err_w) = pipe();
    let started = Instant::now();
    let done = within_on(
        [(out_w.as_fd(), b"exit\n"), (err_w.as_fd(), b"err\n")],
        Duration::from_secs(1),
    );
    assert!(done);
    assert!(started.elapsed() < Duration::from_secs(1));
    drop((out_w, err_w));
    assert_eq!(read_all(out_r), b"exit\n");
    assert_eq!(read_all(err_r), b"err\n");
}

/// A zero wait writes nothing.
#[test]
fn within_a_zero_wait_writes_nothing() {
    let (r, w) = pipe();
    assert!(!within_on(
        [(w.as_fd(), b"exit\n"), (w.as_fd(), b"")],
        Duration::ZERO
    ));
    drop(w);
    assert_eq!(read_all(r), b"");
}

/// A line a full pipe holds is waited on for the wait alone; the writer
/// thread finishes it once the pipe drains, after `within` has returned.
#[test]
fn within_returns_at_its_wait_while_a_full_pipe_holds_the_line() {
    let (r, w) = pipe();
    let other = fill(&w);
    let wait = Duration::from_millis(200);
    let started = Instant::now();
    let done = within_on([(w.as_fd(), b"exit\n"), (w.as_fd(), b"")], wait);
    let took = started.elapsed();
    assert!(!done);
    assert!(took >= wait, "{took:?}");
    assert!(took < Duration::from_secs(1), "{took:?}");
    drop((w, other));
    let got = read_all(r);
    assert!(got.ends_with(b"exit\n"), "written once drained");
    assert!(got.iter().rev().skip(5).all(|&b| b == b'x'));
}

/// The thread with no line returns at once.
#[test]
fn through_thread_with_no_line_is_done() {
    assert!(through_thread(Vec::new(), Duration::ZERO));
}

/// A caller's non-blocking socket starts full and is drained only after
/// the writer reaches `WouldBlock`. The line must survive that error.
#[test]
fn the_writer_thread_retries_would_block_until_room_arrives() {
    use std::os::unix::net::UnixStream;
    let (mut r, w) = UnixStream::pair().unwrap();
    w.set_nonblocking(true).unwrap();
    let file = File::from(OwnedFd::from(w));
    let mut filled = 0;
    loop {
        match (&file).write(&[b'x'; 4096]) {
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => panic!("{e}"),
        }
    }
    let reader = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        got
    });
    assert!(through_thread(
        vec![(file, b"exit\n".to_vec())],
        Duration::from_secs(1)
    ));
    let got = reader.join().unwrap();
    assert_eq!(&got[..filled], vec![b'x'; filled]);
    assert_eq!(&got[filled..], b"exit\n");
}

#[test]
fn the_writer_thread_drops_would_block_at_its_deadline() {
    use std::os::unix::net::UnixStream;
    let (mut r, w) = UnixStream::pair().unwrap();
    w.set_nonblocking(true).unwrap();
    let file = File::from(OwnedFd::from(w));
    while (&file).write(&[b'x'; 4096]).is_ok() {}
    let wait = Duration::from_millis(50);
    let started = Instant::now();
    let _ = through_thread(vec![(file, b"exit\n".to_vec())], wait);
    assert!(started.elapsed() >= wait);
    let mut got = Vec::new();
    r.read_to_end(&mut got).unwrap();
    assert!(
        got.iter().all(|&b| b == b'x'),
        "the expired line was dropped"
    );
}

/// The non-blocking loop writes a line whole within its budget, gives up
/// on a full pipe when the budget is spent, and checks the budget before
/// its first try, so a spent one writes nothing even to an empty pipe.
#[test]
fn nonblocking_checks_its_budget_before_every_try() {
    let (r, w) = pipe();
    let own = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}", w.as_raw_fd()))
        .unwrap();
    nonblocking(&own, b"spent\n", Instant::now());
    nonblocking(&own, b"line\n", Instant::now() + Duration::from_secs(1));
    let other = fill(&w);
    let started = Instant::now();
    nonblocking(&own, b"held\n", started + Duration::from_millis(100));
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(100), "{took:?}");
    assert!(took < Duration::from_secs(1), "{took:?}");
    drop((w, own, other));
    let got = read_all(r);
    assert!(
        got.starts_with(b"line\nx"),
        "the spent budget wrote nothing"
    );
    assert!(!got.windows(5).any(|s| s == b"held\n"), "dropped");
}
