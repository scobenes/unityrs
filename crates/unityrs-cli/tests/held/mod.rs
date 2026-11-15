//! A child's stdout held, for `batch_host_lines.rs`'s full-table scenes:
//! a pipe another writer keeps full, or one that writer fills at the
//! child's every write to fd 1, after that write's `poll` found room, so
//! the write blocks every time. The other writer writes through a
//! description of its own (`/proc/self/fd/<n>` opened again), so its
//! `O_NONBLOCK` never reaches the child's, which a dup would share.
//!
//! The fill at a write is a seccomp user notification (`seccomp(2)`,
//! `seccomp_unotify(2)`): the child installs a filter, before its `exec`,
//! whose every `write` to fd 1 waits for this process, which fills the
//! pipe, then lets the write run against it
//! (`SECCOMP_USER_NOTIF_FLAG_CONTINUE`, Linux 5.5). Where that cannot be
//! done (no filter for the architecture, seccomp refused, an older kernel)
//! the scene fails, saying so: it is never skipped.

use std::{
    fs,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::fs::OpenOptionsExt,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering::SeqCst},
    time::Duration,
};

/// How the child's stdout is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// Full before the child starts and kept full: each page read is
    /// written again at once, so room comes and goes.
    Contended,
    /// Empty, so a `poll` finds room, and filled at each write to fd 1
    /// before that write runs.
    AtWrite,
}

/// The pipe behind the child's stdout: its read end, the child's end, and
/// the other writer's own non-blocking description.
pub struct Pipe {
    pub read: fs::File,
    pub write: OwnedFd,
    pub refill: fs::File,
}
impl Pipe {
    pub fn new() -> io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: two ints for pipe2's descriptors.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both freshly made, owned here alone.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let refill = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(format!("/proc/self/fd/{}", write.as_raw_fd()))?;
        Ok(Self {
            read: fs::File::from(read),
            write,
            refill,
        })
    }
    /// Not one byte of room left.
    pub fn fill(&self) {
        super::common::fill_file(&self.refill);
    }
    /// [`Held::Contended`]'s other writer, until `stop`: each page read is
    /// written again at once.
    pub fn contend(&mut self, stop: &AtomicBool) {
        let (page, mut buf) = ([b'x'; 4096], [0_u8; 4096]);
        while !stop.load(SeqCst) {
            if self.read.read(&mut buf).is_ok_and(|n| n > 0) {
                let _ = self.refill.write(&page);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// The filter's architecture, as `seccomp_data.arch` names it; `None` on
/// one this module has no number for, where [`AtWrite::new`] fails.
#[cfg(target_arch = "x86_64")]
const ARCH: Option<u32> = Some(0xC000_003E);
#[cfg(target_arch = "aarch64")]
const ARCH: Option<u32> = Some(0xC000_00B7);
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const ARCH: Option<u32> = None;

/// A filter whose every `write` to fd 1 waits on a user notification, and
/// the socket pair its listener comes back on.
pub struct AtWrite {
    filter: [libc::sock_filter; 8],
    parent: OwnedFd,
    child: OwnedFd,
}
impl AtWrite {
    /// An error where [`ARCH`] has no number.
    pub fn new() -> io::Result<Self> {
        let Some(arch) = ARCH else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no seccomp_data.arch number for this target in held/mod.rs",
            ));
        };
        let op = |code: u32, k: u32, jt: u8, jf: u8| libc::sock_filter {
            code: u16::try_from(code).unwrap(),
            jt,
            jf,
            k,
        };
        let (load, jeq, ret) = (
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            libc::BPF_RET | libc::BPF_K,
        );
        // `seccomp_data`: `nr` at 0, `arch` at 4, `args[0]`'s low word at
        // 16 (both arches little-endian). Anything else is allowed.
        let filter = [
            op(load, 4, 0, 0),
            op(jeq, arch, 0, 5),
            op(load, 0, 0, 0),
            op(jeq, u32::try_from(libc::SYS_write).unwrap(), 0, 3),
            op(load, 16, 0, 0),
            op(jeq, 1, 0, 1),
            op(ret, libc::SECCOMP_RET_USER_NOTIF, 0, 0),
            op(ret, libc::SECCOMP_RET_ALLOW, 0, 0),
        ];
        let mut fds = [0; 2];
        // SAFETY: two ints for socketpair's descriptors.
        let made = unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        };
        if made != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both freshly made, owned here alone.
        let (parent, child) =
            unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(Self {
            filter,
            parent,
            child,
        })
    }
    /// The filter installed in `cmd`'s child before `then` (its own
    /// `pre_exec` work): no new privileges, the filter, and its listener
    /// sent back on the socket pair and closed, with async-signal-safe
    /// calls alone on what lives in the closure.
    pub fn arm(
        &self,
        cmd: &mut Command,
        then: impl Fn() -> io::Result<()> + Send + Sync + 'static,
    ) {
        let (filter, sock) = (self.filter, self.child.as_raw_fd());
        // SAFETY: prctl, seccomp, sendmsg and close are async-signal-safe;
        // the filter and the message's buffers are on this closure's stack.
        unsafe {
            cmd.pre_exec(move || {
                install(&filter, sock)?;
                then()
            });
        }
    }
    /// The listener, once the child has sent it.
    pub fn listener(&self) -> io::Result<OwnedFd> {
        let mut byte = [0_u8; 1];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut space = [0_u64; 4];
        // SAFETY: an all-zero msghdr is its empty value.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = space.as_mut_ptr().cast();
        // SAFETY: CMSG_SPACE only computes.
        msg.msg_controllen = unsafe { libc::CMSG_SPACE(4) }.try_into().unwrap();
        // Close-on-exec, so no scene spawned beside this one inherits it.
        let flags = libc::MSG_CMSG_CLOEXEC;
        // SAFETY: the message's buffers live through the call.
        if unsafe { libc::recvmsg(self.parent.as_raw_fd(), &raw mut msg, flags) } != 1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a received message's first header, checked below.
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&raw const msg) };
        // SAFETY: a non-null header of the message received above.
        if cmsg.is_null()
            || unsafe { ((*cmsg).cmsg_level, (*cmsg).cmsg_type) }
                != (libc::SOL_SOCKET, libc::SCM_RIGHTS)
        {
            return Err(io::Error::other("no listener sent"));
        }
        // SAFETY: an SCM_RIGHTS header carries the descriptor, now ours.
        Ok(unsafe {
            OwnedFd::from_raw_fd(std::ptr::read_unaligned(
                libc::CMSG_DATA(cmsg).cast::<RawFd>(),
            ))
        })
    }
}

/// [`AtWrite::arm`]'s child side.
fn install(filter: &[libc::sock_filter; 8], sock: RawFd) -> io::Result<()> {
    let fail = || Err(io::Error::last_os_error());
    let prog = libc::sock_fprog {
        len: 8,
        filter: filter.as_ptr().cast_mut(),
    };
    // SAFETY: async-signal-safe calls on buffers that outlive them.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return fail();
        }
        let listener = libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_NEW_LISTENER,
            &raw const prog,
        );
        let Ok(listener) = RawFd::try_from(listener) else {
            return fail();
        };
        if listener < 0 {
            return fail();
        }
        let mut byte = [0_u8; 1];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut space = [0_u64; 4];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = space.as_mut_ptr().cast();
        msg.msg_controllen = libc::CMSG_SPACE(4).try_into().unwrap_or_default();
        let cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(4).try_into().unwrap_or_default();
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), listener);
        let sent = libc::sendmsg(sock, &raw const msg, 0);
        libc::close(listener);
        if sent != 1 {
            return fail();
        }
    }
    Ok(())
}

/// Answers `listener`'s notifications until `stop` or the child's end:
/// each write to fd 1 finds the pipe full, filled before it runs. Gives how
/// many writes it let run (`SECCOMP_USER_NOTIF_FLAG_CONTINUE` taken), or the
/// error of any call that failed but for a target gone (`ENOENT`: killed,
/// or its call interrupted, so nothing is held) or an interrupted `poll`:
/// a kernel without the flag (before 5.5, `EINVAL`) fails here, never
/// leaving the child stopped in its notification unseen.
pub fn serve(listener: &OwnedFd, pipe: &Pipe, stop: &AtomicBool) -> io::Result<usize> {
    let failed = |call: &str, e: io::Error| {
        io::Error::new(
            e.kind(),
            format!("{call} on the seccomp listener (Linux 5.5+, seccomp allowed): {e}"),
        )
    };
    let gone = |e: &io::Error| e.raw_os_error() == Some(libc::ENOENT);
    let interrupted = |e: &io::Error| e.kind() == io::ErrorKind::Interrupted;
    let mut served = 0;
    while !stop.load(SeqCst) {
        let mut poll = [libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: one initialized pollfd.
        match unsafe { libc::poll(poll.as_mut_ptr(), 1, 10) } {
            0 => continue,
            1 => {}
            _ => match io::Error::last_os_error() {
                e if interrupted(&e) => continue,
                e => return Err(failed("poll", e)),
            },
        }
        if poll[0].revents & libc::POLLIN == 0 {
            // No task left under the filter.
            break;
        }
        // SAFETY: an all-zero notification is RECV's empty input.
        let mut notif: libc::seccomp_notif = unsafe { std::mem::zeroed() };
        // SAFETY: the listener and a notification for RECV to fill.
        if unsafe {
            libc::ioctl(
                listener.as_raw_fd(),
                libc::SECCOMP_IOCTL_NOTIF_RECV,
                &raw mut notif,
            )
        } != 0
        {
            match io::Error::last_os_error() {
                e if gone(&e) || interrupted(&e) => continue,
                e => return Err(failed("SECCOMP_IOCTL_NOTIF_RECV", e)),
            }
        }
        pipe.fill();
        let answer = libc::seccomp_notif_resp {
            id: notif.id,
            val: 0,
            error: 0,
            flags: u32::try_from(libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE).unwrap(),
        };
        // SAFETY: the listener and RECV's notification answered.
        let sent = unsafe {
            libc::ioctl(
                listener.as_raw_fd(),
                libc::SECCOMP_IOCTL_NOTIF_SEND,
                &raw const answer,
            )
        };
        if sent != 0 {
            match io::Error::last_os_error() {
                e if gone(&e) => continue,
                e => {
                    return Err(failed(
                        "SECCOMP_IOCTL_NOTIF_SEND with SECCOMP_USER_NOTIF_FLAG_CONTINUE",
                        e,
                    ));
                }
            }
        }
        served += 1;
    }
    Ok(served)
}

/// The child's stdout end, as its `Stdio`.
pub fn stdout(pipe: &Pipe) -> io::Result<Stdio> {
    Ok(Stdio::from(pipe.write.try_clone()?))
}
