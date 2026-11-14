//! W20 preparation is independent of any spool-bound worker context. The
//! existing local Execution prepares channels/workers before spool startup;
//! B9b chooses the unified launch order when it adopts these two runtimes.
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
pub struct Runtimes {
    pub controller: tokio::runtime::Runtime,
    pub finisher: tokio::runtime::Runtime,
    per_runtime: usize,
}
struct Count {
    started: usize,
    closed: bool,
}
impl Runtimes {
    pub fn prepare(download_concurrency: u32) -> io::Result<Self> {
        Self::prepare_with(download_concurrency, || {})
    }
    /// Thread registration (including tally) is fixed before either pool starts.
    pub fn prepare_with(download_concurrency: u32, on_start: fn()) -> io::Result<Self> {
        if !(1..=256).contains(&download_concurrency) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let threads =
            usize::try_from(download_concurrency + 1).map_err(|_| io::ErrorKind::InvalidInput)?;
        let build = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .max_blocking_threads(threads)
                .on_thread_start(on_start)
                .thread_stack_size(2 << 20)
                .thread_keep_alive(Duration::MAX)
                .build()
        };
        Ok(Self {
            controller: build()?,
            finisher: build()?,
            per_runtime: threads,
        })
    }
    /// Exactly dc+1 tasks per runtime share one absolute second. The mutex is
    /// closed before return; late tasks cannot repair a failed startup. No
    /// task can finish and free a thread until all target tasks start.
    pub fn prewarm(&self) -> io::Result<()> {
        let target = 2 * self.per_runtime;
        let count = Arc::new((
            Mutex::new(Count {
                started: 0,
                closed: false,
            }),
            Condvar::new(),
        ));
        let deadline = Instant::now() + Duration::from_secs(1);
        for runtime in [&self.controller, &self.finisher] {
            for _ in 0..self.per_runtime {
                let task_count = count.clone();
                if catch_unwind(AssertUnwindSafe(|| {
                    runtime.spawn_blocking(move || {
                        let (lock, changed) = &*task_count;
                        let mut state = lock
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if !state.closed && Instant::now() < deadline {
                            state.started += 1;
                            changed.notify_all();
                        }
                        while !state.closed && state.started < target {
                            let left = deadline.saturating_duration_since(Instant::now());
                            if left.is_zero() {
                                break;
                            }
                            state = changed
                                .wait_timeout(state, left)
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .0;
                        }
                    })
                }))
                .is_err()
                {
                    let (lock, changed) = &*count;
                    lock.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .closed = true;
                    changed.notify_all();
                    return Err(io::ErrorKind::Other.into());
                }
            }
        }
        let (lock, changed) = &*count;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.started < target {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = changed
                .wait_timeout(state, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        state.closed = true;
        changed.notify_all();
        if state.started == target {
            Ok(())
        } else {
            Err(io::ErrorKind::TimedOut.into())
        }
    }
}
