//! Routing through `own`'s seam with real descriptor metadata and the
//! real writer thread. Reopening all character devices, omitting FIFOs
//! or omitting terminals must fail these tests.

use std::fs::OpenOptions;
use std::io::{IsTerminal, Read};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::process::Command;
use std::time::Instant;

use super::{OWN_WAIT, own_with_reopen, reopen};
use unityrs_testkit::{child, scratch::Scratch};

fn write_recording_reopen(fd: BorrowedFd<'_>) -> bool {
    let mut attempted = false;
    own_with_reopen(fd, b"exit\n", Instant::now() + OWN_WAIT, |fd| {
        attempted = true;
        reopen(fd)
    });
    attempted
}

#[test]
fn own_reopens_a_fifo() {
    let scratch = Scratch::new("own-fifo");
    let fifo = scratch.join("output");
    assert!(
        child::status(Command::new("mkfifo").arg(&fifo))
            .unwrap()
            .success()
    );
    let mut reader = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .unwrap();
    let file = OpenOptions::new().write(true).open(&fifo).unwrap();
    assert!(file.metadata().unwrap().file_type().is_fifo());
    assert!(write_recording_reopen(file.as_fd()));
    drop(file);
    let mut got = Vec::new();
    reader.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"exit\n");
}

#[test]
fn own_reopens_a_terminal() {
    let (_master, slave) = crate::sys::terminal_pair().unwrap();
    assert!(slave.is_terminal());
    assert!(write_recording_reopen(slave.as_fd()));
}

#[test]
fn own_keeps_a_nonterminal_character_devices_description() {
    let file = OpenOptions::new().write(true).open("/dev/null").unwrap();
    assert!(file.metadata().unwrap().file_type().is_char_device());
    assert!(!file.is_terminal());
    assert!(!write_recording_reopen(file.as_fd()));
}
