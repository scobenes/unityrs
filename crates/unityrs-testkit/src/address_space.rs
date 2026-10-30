//! A cap on the process's address space: how a test makes the C allocator
//! refuse.
//!
//! liblzma allocates its dictionary with C's `malloc`, which no Rust global
//! allocator sees, so a [`crate::counted::Allocator`] cannot refuse it. A cap
//! on the address space can (`RLIMIT_AS`): a mapping, or a `brk`, that would
//! take the process past it fails, so `malloc` returns NULL and liblzma
//! answers `LZMA_MEM_ERROR`. The cap is process-wide, so a test sets it only
//! in a child process (this test binary again, running one ignored probe
//! test, [`run`]), and there only around the call it is about:
//! [`cap_above_current`] caps the soft limit a margin above what the process
//! maps already, and the [`Capped`] it returns puts the soft limit back
//! when it drops (a process may raise its soft limit up to the hard one).
//!
//! **One arena.** glibc gives a thread other than the main one an arena of
//! its own: a 64 MiB reservation, already mapped (so already under any cap
//! set later), that it grows into with `mprotect`. When the `mmap` of a
//! large block fails, glibc grows that arena instead, so a block of up to
//! 64 MiB passes the cap (libtest runs each test on a thread of its own).
//! [`run`] starts the child with `MALLOC_ARENA_MAX=1`: every thread then
//! takes the main arena, which grows with `brk`, which the cap refuses too.
//! A probe also allocates and frees no large block before its cap: glibc
//! keeps the memory of such a free (a freed `mmap`ped chunk raises its
//! `mmap` threshold, and the heap then keeps what is freed below it) and
//! serves a later block from it, which no cap sees.
//!
//! Linux only, on x86-64 and 64-bit Arm (whose `RLIMIT_AS` is 9 and `rlim_t`
//! 64 bits, as declared here: the crate takes no `libc` dependency for two
//! calls). Elsewhere [`SUPPORTED`] is false and [`cap_above_current`] is
//! [`std::io::ErrorKind::Unsupported`]: a caller says it skipped.
//!
//! Owner: package C (its codec tests and the TPK's).

use std::io;
use std::process::{Command, Output};

use crate::stack::{Probe, classify};

/// Whether this target can cap its address space here.
pub const SUPPORTED: bool = imp::SUPPORTED;

/// The address space this process maps now, in bytes (`VmSize` in
/// `/proc/self/status`).
///
/// # Errors
/// When the target has no `/proc/self/status`, or it names no `VmSize`.
pub fn mapped() -> io::Result<u64> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|kb| kb.trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .ok_or_else(|| io::Error::other("no VmSize in /proc/self/status"))
}

/// Runs this test binary's ignored test `probe` (its whole path, as
/// `--exact` matches it) in a child process with `var` set to `value`, on
/// one malloc arena (module docs).
///
/// # Panics
/// When the child can not start, or comes to anything but a passed probe
/// ([`classify`]): the message says what, and ends with the child's last
/// lines of output.
#[track_caller]
pub fn run(probe: &str, var: &str, value: &str) {
    let exe = std::env::current_exe().expect("the test binary's path");
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(exe)
        .args(["--exact", probe, "--ignored", "--test-threads=1"])
        .env(var, value)
        .env("MALLOC_ARENA_MAX", "1")
        .output()
        .unwrap_or_else(|e| panic!("cannot start probe {probe}: {e}"));
    match classify(status, &stdout, &stderr) {
        Ok(Probe::Fits) => {}
        Ok(Probe::Overflows) => panic!("probe {probe} overflowed a stack"),
        Err(e) => panic!("probe {probe}: {e}"),
    }
}

/// The soft address-space limit this process had before a
/// [`cap_above_current`], put back when it drops.
#[derive(Debug)]
#[must_use = "the cap lasts only while this lives"]
pub struct Capped {
    old: u64,
    /// The soft limit set.
    pub cap: u64,
}

impl Drop for Capped {
    fn drop(&mut self) {
        // Raising the soft limit back up to the hard one is always allowed.
        let _ = imp::set_soft(self.old);
    }
}

/// Caps this process's address space `margin` bytes above what it maps
/// now ([`mapped`]): an allocation of more than `margin` bytes in one
/// piece then fails, while smaller ones made before it fit.
///
/// # Errors
/// When the target is not supported ([`SUPPORTED`]), or the limit cannot
/// be read or set (a hard limit below the cap).
pub fn cap_above_current(margin: u64) -> io::Result<Capped> {
    let (old, hard) = imp::get()?;
    let cap = mapped()?.saturating_add(margin);
    if cap > hard {
        return Err(io::Error::other(format!(
            "the hard address-space limit, {hard} bytes, is below the cap, {cap}"
        )));
    }
    imp::set_soft(cap)?;
    Ok(Capped { old, cap })
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod imp {
    use std::io;

    pub const SUPPORTED: bool = true;

    /// `RLIMIT_AS` on Linux x86-64 and 64-bit Arm.
    const RLIMIT_AS: i32 = 9;

    /// `struct rlimit`: `rlim_t` is `unsigned long`, 64 bits here.
    #[repr(C)]
    struct Rlimit {
        cur: u64,
        max: u64,
    }

    unsafe extern "C" {
        fn getrlimit(resource: i32, rlim: *mut Rlimit) -> i32;
        fn setrlimit(resource: i32, rlim: *const Rlimit) -> i32;
    }

    /// The soft and the hard limit.
    pub fn get() -> io::Result<(u64, u64)> {
        let mut r = Rlimit { cur: 0, max: 0 };
        // SAFETY: `r` is a valid, writable `struct rlimit` for the call.
        if unsafe { getrlimit(RLIMIT_AS, &raw mut r) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((r.cur, r.max))
    }

    /// Sets the soft limit to `cur`, keeping the hard one.
    pub fn set_soft(cur: u64) -> io::Result<()> {
        let (_, max) = get()?;
        let r = Rlimit { cur, max };
        // SAFETY: `r` is a valid `struct rlimit`, only read by the call.
        if unsafe { setrlimit(RLIMIT_AS, &raw const r) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
mod imp {
    use std::io;

    pub const SUPPORTED: bool = false;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "an address-space cap is set on Linux x86-64 and 64-bit Arm only",
        )
    }

    pub fn get() -> io::Result<(u64, u64)> {
        Err(unsupported())
    }

    pub fn set_soft(_: u64) -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::{SUPPORTED, cap_above_current, mapped, run};

    const VAR: &str = "UNITYRS_TESTKIT_ADDRESS_SPACE_PROBE";

    /// Run by the test below, in a child: under a cap 4 MiB above what it
    /// maps, an 8 MiB allocation fails (on this thread, libtest's own) and
    /// a 64 KiB one does not; once the cap drops, the 8 MiB one succeeds.
    #[test]
    #[ignore = "run by a_cap_refuses_what_passes_it_and_drops_with_its_guard"]
    fn probe() {
        if std::env::var(VAR).is_err() {
            return;
        }
        let big = 8 << 20;
        {
            let capped = cap_above_current(4 << 20).unwrap();
            assert!(capped.cap >= mapped().unwrap());
            let mut v: Vec<u8> = Vec::new();
            assert!(v.try_reserve_exact(big).is_err(), "8 MiB fit under the cap");
            let mut w: Vec<u8> = Vec::new();
            assert!(w.try_reserve_exact(64 << 10).is_ok());
        }
        let mut v: Vec<u8> = Vec::new();
        assert!(
            v.try_reserve_exact(big).is_ok(),
            "the cap outlived its guard"
        );
    }

    #[test]
    fn a_cap_refuses_what_passes_it_and_drops_with_its_guard() {
        if !SUPPORTED {
            eprintln!("skipped: no address-space cap on this target");
            return;
        }
        run("address_space::tests::probe", VAR, "1");
    }
}
