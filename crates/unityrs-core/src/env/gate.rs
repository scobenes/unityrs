//! A test's gate on the Environment's hook points ([`Hook`]): it stops
//! chosen threads, each at one or more chosen points in turn, until the
//! test lets them go, so a test runs two or more operations through their
//! steps in an order of its choosing, never by repetition or timing. Every
//! wait goes through `unityrs_testkit::guard`, a deadlock guard.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here carries one.
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(test)]
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(test)]
use std::thread::{JoinHandle, ThreadId};

#[cfg(test)]
use super::Environment;
#[cfg(test)]
use super::registry::{Hook, HookFn};
#[cfg(test)]
use unityrs_testkit::guard;

#[cfg(test)]
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What an armed thread tells the test.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// It reached the point it was stopped at.
    Stopped,
    /// Its operation returned (without reaching the point, or after it was
    /// let go).
    Done,
}

/// One armed thread: where it stops next (the `nth` time it reaches a
/// point, counted from its last stop) and after, and its channels.
#[cfg(test)]
struct Armed {
    thread: ThreadId,
    stops: VecDeque<(Hook, usize)>,
    seen: usize,
    events: Sender<Event>,
    go: Arc<Mutex<Receiver<()>>>,
}

/// Takes its thread's [`Armed`] off the gate as the thread leaves its
/// operation, however it leaves: the entry holds a sender of the test's
/// channel, so a thread that panicked before its point would leave the test
/// waiting out the deadlock guard for an answer that cannot come (review
/// round 15: ten minutes), when `guard::recv` fails at once on a closed
/// channel and says that the thread's own message tells why.
#[cfg(test)]
struct Leaving(Arc<Mutex<Vec<Armed>>>);

#[cfg(test)]
impl Drop for Leaving {
    fn drop(&mut self) {
        let me = std::thread::current().id();
        lock(&self.0).retain(|a| a.thread != me);
    }
}

/// The gate, installed on an Environment's registry and LRU hooks.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct Gate {
    armed: Arc<Mutex<Vec<Armed>>>,
}

/// A thread the gate runs, armed to stop once at a point.
#[cfg(test)]
pub(crate) struct Run<T> {
    events: Receiver<Event>,
    go: Sender<()>,
    join: Option<JoinHandle<T>>,
    /// Whether it returned (its `Done` was read).
    done: std::cell::Cell<bool>,
}

#[cfg(test)]
impl Gate {
    /// A gate on `env`'s hooks (the registry's and the LRU's).
    pub(crate) fn install(env: &Environment) -> Self {
        let gate = Self::default();
        let g = gate.clone();
        let f = HookFn(Arc::new(move |h| g.reach(h)));
        *lock(&env.inner.registry.hook) = Some(f.clone());
        *lock(&env.inner.cache.hook) = Some(f);
        gate
    }

    /// Runs `f` on a thread of its own, which stops the `nth` time it
    /// reaches `at` (from 1; `None`: never), until [`Run::release`].
    pub(crate) fn run<T: Send + 'static>(
        &self,
        at: Option<(Hook, usize)>,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Run<T> {
        self.run_stops(at.into_iter().collect(), f)
    }

    /// [`Self::run`] with stops in turn: the thread stops the `nth` time it
    /// reaches the first `(at, nth)`, and once let go, the `nth` time after
    /// that it reaches the next, and so on.
    pub(crate) fn run_stops<T: Send + 'static>(
        &self,
        stops: Vec<(Hook, usize)>,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Run<T> {
        let (events_tx, events) = mpsc::channel();
        let (go, go_rx) = mpsc::channel();
        let armed = self.armed.clone();
        let (ready_tx, ready) = mpsc::channel::<()>();
        let join = std::thread::spawn(move || {
            if !stops.is_empty() {
                lock(&armed).push(Armed {
                    thread: std::thread::current().id(),
                    stops: stops.into(),
                    seen: 0,
                    events: events_tx.clone(),
                    go: Arc::new(Mutex::new(go_rx)),
                });
            }
            let _ = ready_tx.send(());
            let leaving = Leaving(armed);
            let out = f();
            drop(leaving);
            let _ = events_tx.send(Event::Done);
            out
        });
        guard::wait(&ready, "the armed thread to start");
        Run {
            events,
            go,
            join: Some(join),
            done: std::cell::Cell::new(false),
        }
    }

    fn reach(&self, h: Hook) {
        let me = std::thread::current().id();
        let stop = {
            let mut armed = lock(&self.armed);
            let Some(i) = armed
                .iter()
                .position(|a| a.thread == me && a.stops.front().is_some_and(|(at, _)| *at == h))
            else {
                return;
            };
            let a = &mut armed[i];
            a.seen += 1;
            if a.stops.front().is_some_and(|(_, nth)| a.seen == *nth) {
                a.stops.pop_front();
                a.seen = 0;
                let channels = (a.events.clone(), a.go.clone());
                if a.stops.is_empty() {
                    armed.swap_remove(i);
                }
                Some(channels)
            } else {
                None
            }
        };
        if let Some((events, go)) = stop {
            let _ = events.send(Event::Stopped);
            guard::hold(&lock(&go), "the test to let the stopped thread go");
        }
    }
}

#[cfg(test)]
impl<T> Run<T> {
    /// What the thread did first: stopped at its point, or returned.
    pub(crate) fn next(&self) -> Event {
        let e = guard::recv(&self.events, "the armed thread to stop or return");
        if e == Event::Done {
            self.done.set(true);
        }
        e
    }

    /// Waits until it stops at its point.
    #[track_caller]
    pub(crate) fn stopped(&self) {
        assert_eq!(self.next(), Event::Stopped, "it returned before its point");
    }

    /// Lets it go on from its point.
    pub(crate) fn release(&self) {
        let _ = self.go.send(());
    }

    /// Lets it go if it stopped, and waits for its result.
    pub(crate) fn finish(mut self) -> T {
        while !self.done.get() {
            self.release();
            self.next();
        }
        match self.join.take().map(JoinHandle::join) {
            Some(Ok(v)) => v,
            Some(Err(e)) => std::panic::resume_unwind(e),
            None => unreachable!("joined once"),
        }
    }
}

/// An armed thread that panics before its point fails the test's wait at
/// once, by its closed channel, not by the deadlock guard ([`Leaving`]).
#[cfg(test)]
#[test]
fn an_armed_thread_that_panics_closes_its_channel() {
    let gate = Gate::default();
    let mut run = gate.run(Some((Hook::Published, 1)), || {
        panic!("the armed thread's own failure")
    });
    let joined = run.join.take().map(JoinHandle::join);
    assert!(matches!(joined, Some(Err(_))), "the thread did not panic");
    assert!(
        lock(&gate.armed).is_empty(),
        "the gate still holds a sender of the thread's channel"
    );
    let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.next()));
    let failure = waited.expect_err("a closed channel fails the wait");
    assert!(
        failure
            .downcast_ref::<String>()
            .is_some_and(|m| m.contains("panicked without answering")),
        "the wait failed some other way"
    );
}
