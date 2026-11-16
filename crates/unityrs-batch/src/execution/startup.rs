//! Exercise retained runtime state before any running head is published.
use crate::sys::{Lock, RegisteredWake, Wake};
use std::{io, sync::Arc, task::Context, time::Duration};

static TASK_IDS: Lock<()> = Lock::new(());

pub(crate) fn prime(runtime: &tokio::runtime::Runtime) -> io::Result<()> {
    // Linux's retained mutexes are inline. A disposable registration there
    // would consume the reactor's initial deregistration capacity needlessly.
    if !cfg!(target_vendor = "apple") {
        return Ok(());
    }
    if tokio::runtime::Handle::try_current().is_ok() {
        // Library preparation can run inside a caller's Tokio task. Drive
        // our runtime outside that context, without nesting block_on.
        return std::thread::scope(|scope| {
            let startup =
                std::thread::Builder::new().spawn_scoped(scope, || prime_idle(runtime))?;
            startup
                .join()
                .map_err(|_| io::Error::other("runtime startup panicked"))?
        });
    }
    prime_idle(runtime)
}

fn prime_idle(runtime: &tokio::runtime::Runtime) -> io::Result<()> {
    let wake = Arc::new(Wake::new()?);
    let registered = RegisteredWake::new(&wake, runtime.handle())?;
    // Also initialize this disposable registration's private waiters lock;
    // otherwise its eventual reactor release can first lock it after startup.
    let _ = registered.poll_wait(&mut Context::from_waker(std::task::Waker::noop()));
    drop(registered);

    // Tokio 1.53.3's current-thread OwnedTasks has four shards, selected by
    // task id & 3. Spawn outside block_on to touch the remote inject queue.
    // Serialize our primers and submit bursts before driving them: alternating
    // spawn/block_on calls in concurrent primers can consume only odd or even
    // IDs indefinitely. Other Tokio callers still share the ID counter, so
    // observe coverage and allow a full startup second rather than 64 tries.
    let serial = TASK_IDS.lock();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut shards = 0_u8;
    loop {
        let tasks: [_; 4] = std::array::from_fn(|_| runtime.spawn(async {}));
        for task in tasks {
            let id: u64 = task.id().to_string().parse().map_err(io::Error::other)?;
            shards |= 1 << (id & 3);
            runtime.block_on(task).map_err(io::Error::other)?;
        }
        if shards == 15 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::yield_now();
    }
    drop(serial);
    // Drive the reactor's release queue and timer locks before any later
    // block_on or drop.
    runtime.block_on(async { tokio::time::sleep(Duration::ZERO).await });
    Ok(())
}
