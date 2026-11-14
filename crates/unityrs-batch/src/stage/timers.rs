//! Active body time and injected-clock waits. Expiry wins equality, including
//! DATA/EOF or a connect failure becoming ready at the same instant.
use super::Failure;
use crate::control::Control;
use std::{
    future::{Future, poll_fn},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    task::Poll,
    time::{Duration, SystemTime},
};

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Duration;
    fn wall(&self) -> SystemTime;
    fn sleep_until(&self, deadline: Duration) -> impl Future<Output = ()>;
}
/// Runtime-local; construct after enabling the IO and time drivers.
pub struct RuntimeClock(tokio::time::Instant);
impl Default for RuntimeClock {
    fn default() -> Self {
        Self(tokio::time::Instant::now())
    }
}
impl Clock for RuntimeClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
    async fn sleep_until(&self, deadline: Duration) {
        if let Some(until) = self.0.checked_add(deadline) {
            tokio::time::sleep_until(until).await;
        } else {
            std::future::pending::<()>().await;
        }
    }
}
/// The production stage clock. On Linux, `CLOCK_BOOTTIME` since boot: it
/// counts a suspend, as the deadline's cutoff does (`lifetime::Policy`), so
/// the cutoff is [`Self::cutoff`] of the policy's and a retry never starts
/// past it after a suspend. A failing clock reads as `Duration::MAX`, which
/// expires every timer rather than extending one. Off Linux (non-normative
/// test hosts, with no boot clock and so no cutoff) a monotonic origin.
#[cfg_attr(target_os = "linux", derive(Default))]
pub struct BootClock {
    #[cfg(not(target_os = "linux"))]
    origin: std::time::Instant,
}
#[cfg(not(target_os = "linux"))]
impl Default for BootClock {
    fn default() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}
impl BootClock {
    /// The cutoff on this clock: the policy's cutoff, milliseconds on
    /// `CLOCK_BOOTTIME`, is this clock's reading at that instant.
    #[must_use]
    pub fn cutoff(cutoff: Option<crate::lifetime::Boot>) -> Option<Duration> {
        cutoff.map(|boot| Duration::from_millis(boot.0))
    }
}
impl Clock for BootClock {
    fn now(&self) -> Duration {
        #[cfg(target_os = "linux")]
        {
            crate::sys::boottime().unwrap_or(Duration::MAX)
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.origin.elapsed()
        }
    }
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
    /// Tokio's timers run on the monotonic clock, which a suspend stops:
    /// sleep at most a second at a time, re-reading this clock between.
    async fn sleep_until(&self, deadline: Duration) {
        loop {
            let left = deadline.saturating_sub(self.now());
            if left.is_zero() {
                return;
            }
            tokio::time::sleep(left.min(Duration::from_secs(1))).await;
        }
    }
}
/// B9 supplies the canonical control and cutoff. Its outer driver must freeze
/// this future on a stop wake; it must retain the stage and unreceived tasks.
pub struct Context<C> {
    pub clock: std::sync::Arc<C>,
    pub control: std::sync::Arc<dyn Control>,
    pub cutoff: Option<Duration>,
}
impl<C: Clock> Context<C> {
    pub fn check(&self) -> Result<(), Failure> {
        if self.control.stopped() || self.cutoff.is_some_and(|d| self.clock.now() >= d) {
            Err(Failure::Stopped)
        } else {
            Ok(())
        }
    }
    /// Cutoff checks bracket every wait; a cutoff wins a simultaneous result.
    pub async fn wait<F: Future>(&self, future: F) -> Result<F::Output, Failure> {
        self.check()?;
        let output = if let Some(cutoff) = self.cutoff {
            race(self.clock.as_ref(), cutoff, future)
                .await
                .ok_or(Failure::Stopped)?
        } else {
            future.await
        };
        self.check()?;
        Ok(output)
    }
    /// A process cutoff freezes an in-flight request. Keep both the pending
    /// future and a ready-but-unreceived response in this async frame until
    /// B9's outer driver parks it; returning Stopped here would close sockets.
    pub(crate) async fn network_wait<F: Future>(&self, future: F) -> Result<F::Output, Failure> {
        if self.check().is_err() {
            std::future::pending::<()>().await;
        }
        let mut future = std::pin::pin!(future);
        let output = if let Some(cutoff) = self.cutoff {
            race(self.clock.as_ref(), cutoff, future.as_mut()).await
        } else {
            Some(future.as_mut().await)
        };
        if self.check().is_err() {
            // `output` may own the response, and `future` may still own a
            // connecting socket. Neither is dropped at the freeze boundary.
            // An allocation refused in this poll is not lost with `output`:
            // the stage's poll wrapper has already requested it (`url.rs`).
            std::future::pending::<()>().await;
        }
        output.ok_or(Failure::Stopped)
    }
}
/// Clock is re-read after polling: a deterministic transport may advance it
/// while yielding DATA. The injected clock, not poll order, settles the tie.
pub(crate) async fn race<C: Clock, F: Future>(
    clock: &C,
    deadline: Duration,
    future: F,
) -> Option<F::Output> {
    let mut future = std::pin::pin!(future);
    let mut sleep = std::pin::pin!(clock.sleep_until(deadline));
    poll_fn(|cx| {
        if clock.now() >= deadline {
            return Poll::Ready(None);
        }
        let result = future.as_mut().poll(cx);
        if let Poll::Ready(value) = result {
            // The caller assesses elapsed windows before accepting the value.
            // Preserve a completed future when a satisfied rate window rolls.
            return Poll::Ready(Some(value));
        }
        if clock.now() >= deadline {
            return Poll::Ready(None);
        }
        if sleep.as_mut().poll(cx).is_ready() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    })
    .await
}

/// One try's active body time. Its moving parts are atomics, read and
/// written by the one thread polling the stage, only so a shared borrow
/// (the credit closure's and the wait's at once) keeps the stage's future
/// `Send`: the controller retains it in a stage slot.
pub(crate) struct BodyTime {
    start: Duration,
    period: Duration,
    floor: u64,
    paused: AtomicU64,
    idle: AtomicU64,
    window: AtomicU64,
    bytes: AtomicU64,
}
fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}
fn load(cell: &AtomicU64) -> Duration {
    Duration::from_nanos(cell.load(Relaxed))
}
impl BodyTime {
    pub fn new(start: Duration, period: Duration, floor: u32) -> Self {
        Self {
            start,
            period,
            floor: u64::from(floor),
            paused: AtomicU64::new(0),
            idle: AtomicU64::new(nanos(period)),
            window: AtomicU64::new(nanos(period)),
            bytes: AtomicU64::new(0),
        }
    }
    fn active(&self, now: Duration) -> Duration {
        now.saturating_sub(self.start)
            .saturating_sub(load(&self.paused))
    }
    pub fn deadline(&self) -> Duration {
        self.start
            .saturating_add(load(&self.paused))
            .saturating_add(load(&self.idle).min(load(&self.window)))
    }
    /// Complete windows are assessed before crediting a byte at their boundary.
    pub fn advance(&self, now: Duration) -> bool {
        let active = self.active(now);
        if active >= load(&self.idle) {
            return false;
        }
        let window = load(&self.window);
        if active >= window {
            if self.bytes.load(Relaxed) < self.floor {
                return false;
            }
            self.bytes.store(0, Relaxed);
            let window = window.saturating_add(self.period);
            self.window.store(nanos(window), Relaxed);
            // No DATA in any skipped, complete window.
            if active >= window {
                return false;
            }
        }
        true
    }
    pub fn data(&self, now: Duration, count: usize) {
        if count != 0 {
            self.idle
                .store(nanos(self.active(now).saturating_add(self.period)), Relaxed);
            let bytes = self.bytes.load(Relaxed).saturating_add(count as u64);
            self.bytes.store(bytes, Relaxed);
        }
    }
    pub fn pause(&self, duration: Duration) {
        let paused = load(&self.paused).saturating_add(duration);
        self.paused.store(nanos(paused), Relaxed);
    }
}
