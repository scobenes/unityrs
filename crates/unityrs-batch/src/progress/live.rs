//! One replaceable snapshot and one priority terminal message. The writer
//! never holds the mailbox while formatting or touching stdout.
use super::{Exit, Snapshot, clock::Clock};
use std::{
    fs::File,
    io::{self, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

enum Initialization {
    Waiting,
    Ready,
}
struct State {
    initialization: Initialization,
    latest: Snapshot,
    pending: bool,
    exit: Option<Exit>,
    closed: bool,
    acknowledged: bool,
    spool: Option<Arc<crate::spool::Counter>>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    attempts: Arc<AtomicU64>,
    phase: Arc<AtomicU64>,
    sample_after: AtomicU64,
    started: Instant,
}
#[derive(Clone)]
pub struct Sender(Arc<Shared>);
pub struct Service {
    sender: Sender,
}
impl Sender {
    pub(crate) fn sample_due(&self) -> bool {
        let now = u64::try_from(self.0.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let next = self.0.sample_after.load(Ordering::Acquire);
        now >= next
            && self
                .0
                .sample_after
                .compare_exchange(
                    next,
                    now.saturating_add(100),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
    }
    #[must_use]
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.0.state.lock().ok().map(|s| s.latest)
    }

    pub fn post(&self, snapshot: Snapshot) {
        self.update(|s| *s = snapshot);
    }
    pub fn update(&self, update: impl FnOnce(&mut Snapshot)) {
        if let Ok(mut state) = self.0.state.lock() {
            if state.closed {
                return;
            }
            update(&mut state.latest);
            state.pending = true;
            self.0.changed.notify_one();
        }
    }
    #[must_use]
    pub fn attempts(&self) -> Arc<AtomicU64> {
        self.0.attempts.clone()
    }
    #[must_use]
    pub fn phase(&self) -> Arc<AtomicU64> {
        self.0.phase.clone()
    }
    pub fn spool(&self, counter: Arc<crate::spool::Counter>) {
        if let Ok(mut state) = self.0.state.lock() {
            state.spool = Some(counter);
        }
    }
}
impl Service {
    /// Startup fixed storage is `C_base`. No line storage grows at runtime.
    pub fn start(file: File, initial: Snapshot) -> io::Result<Self> {
        Self::start_with(file, initial, || {})
    }
    /// Delay/observe writer initialization in refusal tests.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn test_start(
        file: File,
        initial: Snapshot,
        initializing: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        Self::start_with(file, initial, initializing)
    }
    fn start_with(
        file: File,
        initial: Snapshot,
        initializing: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        let line = Line::new()?;
        let sender = Sender(Arc::new(Shared {
            state: Mutex::new(State {
                initialization: Initialization::Waiting,
                latest: initial,
                pending: true,
                exit: None,
                closed: false,
                acknowledged: false,
                spool: None,
            }),
            sample_after: AtomicU64::new(0),
            started: Instant::now(),
            changed: Condvar::new(),
            attempts: Arc::new(AtomicU64::new(0)),
            phase: Arc::new(AtomicU64::new(0)),
        }));
        let shared = sender.0.clone();
        std::thread::Builder::new()
            .name("batch-progress".into())
            .stack_size(2 << 20)
            .spawn(move || {
                initializing();
                writer(file, line, &shared);
            })?;
        let mut state = sender.0.state.lock().map_err(|_| io::ErrorKind::Other)?;
        while matches!(state.initialization, Initialization::Waiting) {
            state = sender
                .0
                .changed
                .wait(state)
                .map_err(|_| io::ErrorKind::Other)?;
        }
        drop(state);
        Ok(Self { sender })
    }
    #[must_use]
    pub fn sender(&self) -> Sender {
        self.sender.clone()
    }
    pub fn post(&self, snapshot: Snapshot) {
        self.sender.post(snapshot);
    }
    /// A full/broken stdout cannot extend the caller's acknowledgment budget.
    #[must_use]
    pub fn exit(&self, snapshot: Snapshot, exit: Exit, wait: Duration) -> bool {
        let until = Instant::now() + wait.min(Duration::from_secs(1));
        let Ok(mut state) = self.sender.0.state.lock() else {
            return false;
        };
        if !state.closed {
            state.latest = snapshot;
            state.exit = Some(exit);
            state.closed = true;
            self.sender.0.changed.notify_one();
        }
        while !state.acknowledged {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            let Ok((next, _)) = self.sender.0.changed.wait_timeout(state, left) else {
                return false;
            };
            state = next;
        }
        true
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        if let Ok(mut state) = self.sender.0.state.lock() {
            state.closed = true;
            self.sender.0.changed.notify_one();
        }
    }
}
struct Line {
    bytes: Vec<u8>,
}
impl Line {
    fn new() -> io::Result<Self> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(4096)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        Ok(Self { bytes })
    }
}
impl std::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if self.bytes.len() + text.len() > 4096 {
            return Err(std::fmt::Error);
        }
        self.bytes.extend_from_slice(text.as_bytes());
        Ok(())
    }
}
fn writer(mut file: File, mut line: Line, shared: &Shared) {
    let clock = Clock::process(Instant::now());
    {
        let Ok(state) = shared.state.lock() else {
            return;
        };
        // Prime platforms whose condvar storage is lazy before the host can
        // publish the running head, even if startup never needed to wait.
        let Ok((mut state, _)) = shared.changed.wait_timeout(state, Duration::ZERO) else {
            return;
        };
        state.initialization = Initialization::Ready;
        shared.changed.notify_all();
    }
    let mut seq = 0;
    loop {
        let (mut snapshot, exit) = {
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            if !state.pending && !state.closed {
                let Ok((next, _)) = shared.changed.wait_timeout(state, Duration::from_secs(1))
                else {
                    return;
                };
                state = next;
            }
            if state.closed && state.exit.is_none() {
                return;
            }
            let mut snapshot = state.latest;
            if let Some(spool) = &state.spool {
                snapshot.spool.used = spool.used();
            }
            // Reservation changes use this same lock and precede dispatch.
            // Sampling after unlocking could pair an old reservation with a
            // newly dispatched worker's charges.
            snapshot.memory.attempts_used = shared.attempts.load(Ordering::Acquire);
            snapshot.memory.phase_used = shared.phase.load(Ordering::Acquire);
            state.pending = false;
            (snapshot, state.exit.take())
        };
        snapshot.seq = seq;
        if exit.is_none() {
            snapshot.ms = clock.ms();
        }
        snapshot.memory.rss = super::rss();
        line.bytes.clear();
        if snapshot.render(exit.as_ref(), &mut line).is_err()
            || write_line(&mut file, &line.bytes).is_err()
        {
            return;
        }
        if exit.is_some() {
            if let Ok(mut state) = shared.state.lock() {
                state.acknowledged = true;
                shared.changed.notify_all();
            }
            return;
        }
        seq = seq.saturating_add(1);
    }
}
fn write_line(file: &mut File, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match file.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => bytes = bytes.get(n..).ok_or(io::ErrorKind::InvalidData)?,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn start_waits_for_a_delayed_writer_to_initialize() {
        let initialized = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready = initialized.clone();
        let service = Service::test_start(
            File::options().write(true).open("/dev/null").unwrap(),
            Snapshot::validate(0),
            move || {
                std::thread::sleep(Duration::from_millis(50));
                ready.store(true, Ordering::Release);
            },
        )
        .unwrap();
        let ready = initialized.load(Ordering::Acquire);
        let _ = service.exit(
            Snapshot::validate(1),
            Exit {
                status: Some("complete"),
                code: crate::reason::Code::Complete,
                reason: None,
                detail: None,
            },
            Duration::from_secs(1),
        );
        assert!(
            ready,
            "Service::start returned before writer initialization"
        );
    }
}
