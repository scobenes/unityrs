//! Active body time and injected-clock waits. Expiry wins equality, including
//! DATA/EOF or a connect failure becoming ready at the same instant.
use super::Failure;
use crate::control::Control;
use std::{
    future::{Future, poll_fn},
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

pub(crate) struct BodyTime {
    start: Duration,
    paused: Duration,
    idle: Duration,
    window: Duration,
    period: Duration,
    floor: u64,
    bytes: u64,
}
impl BodyTime {
    pub fn new(start: Duration, period: Duration, floor: u32) -> Self {
        Self {
            start,
            paused: Duration::ZERO,
            idle: period,
            window: period,
            period,
            floor: u64::from(floor),
            bytes: 0,
        }
    }
    fn active(&self, now: Duration) -> Duration {
        now.saturating_sub(self.start).saturating_sub(self.paused)
    }
    pub fn deadline(&self) -> Duration {
        self.start
            .saturating_add(self.paused)
            .saturating_add(self.idle.min(self.window))
    }
    /// Complete windows are assessed before crediting a byte at their boundary.
    pub fn advance(&mut self, now: Duration) -> bool {
        let active = self.active(now);
        if active >= self.idle {
            return false;
        }
        if active >= self.window {
            if self.bytes < self.floor {
                return false;
            }
            self.bytes = 0;
            self.window = self.window.saturating_add(self.period);
            // No DATA in any skipped, complete window.
            if active >= self.window {
                return false;
            }
        }
        true
    }
    pub fn data(&mut self, now: Duration, count: usize) {
        if count != 0 {
            self.idle = self.active(now).saturating_add(self.period);
            self.bytes = self.bytes.saturating_add(count as u64);
        }
    }
    pub fn pause(&mut self, duration: Duration) {
        self.paused = self.paused.saturating_add(duration);
    }
}
