use super::*;
use std::io::Write;
#[test]
fn stdin_refuses_files_null_closed_and_write_only_descriptions() {
    let file = std::fs::File::open("/dev/null").unwrap();
    assert!(Stdin::at(file.as_raw_fd()).is_err());
    assert!(Stdin::at(-1).is_err());
    let pipe = Pipe::new().unwrap();
    assert!(Stdin::at(pipe.read.as_raw_fd()).is_ok());
    assert!(Stdin::at(pipe.write.as_raw_fd()).is_err());
    let scratch = unityrs_testkit::scratch::Scratch::new("stdin");
    let file = std::fs::File::create(scratch.join("regular")).unwrap();
    assert!(Stdin::at(file.as_raw_fd()).is_err());
}
#[test]
fn socket_stdin_drains_before_eof_without_changing_flags() {
    let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
    let flags = crate::sys::testing::flags(reader.as_raw_fd()).unwrap();
    let stdin = Stdin::at(reader.as_raw_fd()).unwrap();
    let mut buffer = [0; 4096];
    writer.write_all(b"bytes").unwrap();
    assert_eq!(stdin.read(libc::POLLIN, &mut buffer), Input::Again);
    assert_eq!(&buffer[..5], b"bytes");
    assert_eq!(stdin.read(libc::POLLIN, &mut buffer), Input::Again);
    drop(writer);
    assert_eq!(stdin.read(libc::POLLHUP, &mut buffer), Input::Eof);
    assert_eq!(
        crate::sys::testing::flags(reader.as_raw_fd()).unwrap(),
        flags
    );
    assert_eq!(
        Stdin {
            fd: -1,
            socket: true,
            terminal: false
        }
        .read(libc::POLLNVAL, &mut buffer),
        Input::Ended
    );
}
#[test]
fn full_self_pipe_is_already_a_wake_and_can_be_drained() {
    let pipe = Pipe::new().unwrap();
    for _ in 0..100_000 {
        pipe.notify();
    }
    assert!(pipe.drain());
    assert!(!pipe.drain());
}
/// A pty whose master closed: `isatty` fails with `EIO`, yet it is a
/// terminal, watched, and its hang-up is EOF (`stdin_eof`), not 69.
#[cfg(target_os = "linux")]
#[test]
fn a_hung_up_pty_is_a_terminal_at_eof() {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: writable descriptor outputs; null name, termios and size.
    let opened = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0);
    // SAFETY: openpty returned these two owned descriptors.
    let (master, slave) = unsafe {
        (
            std::os::fd::OwnedFd::from_raw_fd(master),
            std::os::fd::OwnedFd::from_raw_fd(slave),
        )
    };
    drop(master);
    // SAFETY: scalar query of an open descriptor.
    assert_eq!(unsafe { libc::isatty(slave.as_raw_fd()) }, 0);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EIO));
    let stdin = Stdin::at(slave.as_raw_fd()).unwrap();
    assert!(stdin.terminal);
    let mut fds = [libc::pollfd {
        fd: slave.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    }];
    poll(&mut fds, 1_000).unwrap();
    let mut buffer = [0; 4096];
    assert_eq!(stdin.read(fds[0].revents, &mut buffer), Input::Eof);
}
