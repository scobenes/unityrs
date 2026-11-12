//! `ms`: the milliseconds since the process started, in `CLOCK_BOOTTIME`
//! (`docs/BATCH_SPEC.md` section 10), so a suspend of the host counts.
//!
//! The start is `/proc/self/stat`'s `starttime`, the kernel's boot-relative
//! start of the process in clock ticks (`proc(5)`), and the time now is
//! `CLOCK_BOOTTIME`'s ([`crate::sys::boottime`]): both on the boot clock,
//! the start to a tick (10 ms at Linux's 100 a second). Where either
//! cannot be read (off Linux), the clock falls back to an `Instant` taken
//! when `main` started, which does not count a suspend.

use std::time::{Duration, Instant};

/// The process's clock for `ms`.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    origin: Origin,
}

#[derive(Clone, Copy, Debug)]
enum Origin {
    /// The start on the boot clock, and how to read the boot clock now.
    Boot {
        start: Duration,
        now: fn() -> Option<Duration>,
        fallback: Instant,
    },
    /// No boot clock: since `main` started.
    Main(Instant),
}

impl Clock {
    /// The process's clock: its start read from `/proc/self/stat`, else
    /// `main_start` (module docs).
    #[must_use]
    pub fn process(main_start: Instant) -> Self {
        let start = std::fs::read("/proc/self/stat")
            .ok()
            .zip(crate::sys::clock_ticks())
            .and_then(|(stat, ticks)| start_time(&stat, ticks));
        match start {
            Some(start) if crate::sys::boottime().is_some() => {
                Self::on_boot_clock(start, crate::sys::boottime, main_start)
            }
            _ => Self {
                origin: Origin::Main(main_start),
            },
        }
    }

    /// A clock whose start is `start` on a boot clock `now` reads, and
    /// which reads `fallback`'s `Instant` when `now` gives none (a test's
    /// clock).
    #[must_use]
    pub fn on_boot_clock(
        start: Duration,
        now: fn() -> Option<Duration>,
        fallback: Instant,
    ) -> Self {
        Self {
            origin: Origin::Boot {
                start,
                now,
                fallback,
            },
        }
    }

    /// Whether it reads the boot clock.
    #[must_use]
    pub fn is_boot_clock(&self) -> bool {
        matches!(self.origin, Origin::Boot { .. })
    }

    /// The milliseconds since the start.
    #[must_use]
    pub fn ms(&self) -> u64 {
        let since = match self.origin {
            Origin::Boot {
                start,
                now,
                fallback,
            } => now().map_or_else(|| fallback.elapsed(), |n| n.saturating_sub(start)),
            Origin::Main(at) => at.elapsed(),
        };
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    }
}

/// `starttime`, the 22nd field of a `/proc/<pid>/stat` line, as a time
/// since boot, with `ticks` clock ticks a second. The fields are read after
/// the last `)`, which ends the command name (a name may hold spaces and
/// parentheses of its own).
#[must_use]
pub fn start_time(stat: &[u8], ticks: u64) -> Option<Duration> {
    let rest = stat.get(stat.iter().rposition(|&b| b == b')')? + 1..)?;
    // After `)` come field 3 (`state`) onwards: `starttime` is the 20th.
    let field = rest
        .split(u8::is_ascii_whitespace)
        .filter(|f| !f.is_empty())
        .nth(19)?;
    let begun: u64 = std::str::from_utf8(field).ok()?.parse().ok()?;
    if ticks == 0 {
        return None;
    }
    let secs = begun / ticks;
    let nanos = (begun % ticks) * 1_000_000_000 / ticks;
    Some(Duration::new(secs, u32::try_from(nanos).ok()?))
}
