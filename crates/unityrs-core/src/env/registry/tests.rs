//! The LRU lets go of a dependency no registry slot reaches, whatever the
//! order in which two threads drop its slots or load it (review round 2: a
//! dependency whose last two slots dropped at once stayed in the LRU for
//! the Environment's life under `EnvOptions::unitypy()`, which never
//! evicts). Each interleaving is forced with a hook ([`Hook`]), never by
//! repetition; a stress run follows as a second check.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code (this file's `#[cfg(test)] mod tests;` is in
// another), so every item here that names a thread, a channel or a barrier
// carries one.
#[cfg(test)]
use std::sync::{Arc, Barrier, Mutex, mpsc};
#[cfg(test)]
use std::thread::ThreadId;

use super::{Hook, HookFn};
use crate::env::{EnvOptions, Environment, Source};
use crate::limits::Meter;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

/// A dependency bundle that registers two keys, `cab-x` and `cab-x.ress`.
fn two_keys() -> Source {
    Source::Bytes {
        name: RawStr::from("d.bundle"),
        data: Bytes::from(bundle(&[
            ("CAB-X", serialized(&[], &[Obj::named(1, "x1", &[])])),
            ("CAB-X.resS", res_bytes(64, 3)),
        ])),
    }
}

/// Stops the thread that arms it when it reaches `at`, until the test lets
/// it go; every other thread passes.
#[cfg(test)]
struct Pause {
    at: Hook,
    armed: Mutex<Option<ThreadId>>,
    stopped: Mutex<Option<mpsc::Sender<()>>>,
    go: Mutex<Option<mpsc::Receiver<()>>>,
}

/// The test's side of a [`Pause`].
#[cfg(test)]
struct Held {
    pause: Arc<Pause>,
    stopped: mpsc::Receiver<()>,
    go: mpsc::Sender<()>,
}

#[cfg(test)]
impl Held {
    fn install(env: &Environment, at: Hook) -> Self {
        let (stopped_tx, stopped) = mpsc::channel();
        let (go, go_rx) = mpsc::channel();
        let pause = Arc::new(Pause {
            at,
            armed: Mutex::new(None),
            stopped: Mutex::new(Some(stopped_tx)),
            go: Mutex::new(Some(go_rx)),
        });
        let p = pause.clone();
        *env.inner.cache.hook.lock().unwrap() = Some(HookFn(Arc::new(move |h| p.reach(h))));
        Self { pause, stopped, go }
    }

    fn wait_stopped(&self) {
        unityrs_testkit::guard::wait(&self.stopped, "the armed thread to reach its hook");
    }

    fn release(&self) {
        self.go.send(()).unwrap();
    }
}

#[cfg(test)]
impl Pause {
    /// Arms the calling thread.
    fn arm(&self) {
        *self.armed.lock().unwrap() = Some(std::thread::current().id());
    }

    fn reach(&self, h: Hook) {
        if h != self.at {
            return;
        }
        let me = std::thread::current().id();
        let stop = {
            let mut armed = self.armed.lock().unwrap();
            let stop = *armed == Some(me);
            if stop {
                *armed = None;
            }
            stop
        };
        if !stop {
            return;
        }
        if let Some(tx) = self.stopped.lock().unwrap().take() {
            let _ = tx.send(());
        }
        let go = self.go.lock().unwrap().take();
        if let Some(rx) = go {
            unityrs_testkit::guard::hold(&rx, "the test to let the held thread go");
        }
    }
}

/// The meter of the dependency the registry's keys name now: all the LRU
/// should hold.
fn current_dep_bytes(env: &Environment) -> u64 {
    let held = env
        .get_cab_held(&RawStr::from("cab-x"), &Meter::unlimited())
        .unwrap()
        .unwrap();
    held.hold.unwrap().meter().used()
}

/// Two threads each hold one of a replaced dependency's two slots and drop
/// them at once: the first is stopped after it counts its slot out and
/// before it acts, the second drops its slot meanwhile. Exactly one of them
/// is the last, so the LRU lets go (the old test of the `Arc` count saw 3
/// on both threads, and neither did).
#[cfg(test)]
#[test]
fn two_slots_dropped_at_once_leave_the_lru_once() {
    let env = Environment::new(EnvOptions::unitypy());
    let m = Meter::unlimited();
    env.load_dependency(two_keys()).unwrap();
    let a = env
        .registry()
        .slot(&RawStr::from("cab-x"), &m)
        .unwrap()
        .unwrap();
    let b = env
        .registry()
        .slot(&RawStr::from("cab-x.ress"), &m)
        .unwrap()
        .unwrap();
    // A `find_file` reload: both keys now name a new dependency, and only
    // `a` and `b` reach the old one.
    env.load_dependency(two_keys()).unwrap();
    assert_eq!(env.inner.cache.len(), 2);
    let held = Held::install(&env, Hook::SlotReleased);
    let first = {
        let held_pause = held.pause.clone();
        std::thread::spawn(move || {
            held_pause.arm();
            drop(a);
        })
    };
    held.wait_stopped();
    drop(b);
    held.release();
    first.join().unwrap();
    assert_eq!(env.inner.cache.len(), 1, "the replaced dependency left");
    assert_eq!(env.stats().dependency_bytes, current_dep_bytes(&env));
}

/// A dependency load stopped after it touched the LRU and found its slots
/// alive, before it returns, while another load replaces every key it
/// registered: the last of its slots to go makes the LRU let go (the old
/// test of the `Arc` count counted the stopped load's own handle, so
/// neither slot did, nor the load).
#[cfg(test)]
#[test]
fn a_load_whose_keys_are_replaced_before_it_returns_leaves_the_lru() {
    let env = Environment::new(EnvOptions::unitypy());
    let held = Held::install(&env, Hook::LoadChecked);
    let first = {
        let env = env.clone();
        let held_pause = held.pause.clone();
        std::thread::spawn(move || {
            held_pause.arm();
            env.load_dependency(two_keys()).unwrap();
        })
    };
    held.wait_stopped();
    env.load_dependency(two_keys()).unwrap();
    held.release();
    first.join().unwrap();
    assert_eq!(env.inner.cache.len(), 1);
    assert_eq!(env.stats().dependency_bytes, current_dep_bytes(&env));
}

/// A dependency load stopped after it registered and before it touches the
/// LRU, while another load replaces every key: its slots went before the
/// LRU held it, so the load itself makes the LRU let go after its touch.
#[cfg(test)]
#[test]
fn a_load_whose_keys_are_replaced_before_its_touch_leaves_the_lru() {
    let env = Environment::new(EnvOptions::unitypy());
    let held = Held::install(&env, Hook::LoadRegistered);
    let first = {
        let env = env.clone();
        let held_pause = held.pause.clone();
        std::thread::spawn(move || {
            held_pause.arm();
            env.load_dependency(two_keys()).unwrap();
        })
    };
    held.wait_stopped();
    env.load_dependency(two_keys()).unwrap();
    held.release();
    first.join().unwrap();
    assert_eq!(env.inner.cache.len(), 1);
    assert_eq!(env.stats().dependency_bytes, current_dep_bytes(&env));
}

/// The stress run: many rounds of two slots of a replaced dependency
/// dropped by two threads released together. Passes on any schedule; the
/// old code left about one round in a thousand in the LRU.
#[cfg(test)]
#[test]
fn slots_dropped_together_never_leave_a_dependency_behind() {
    const ROUNDS: usize = 2_000;
    let env = Environment::new(EnvOptions::unitypy());
    let m = Meter::unlimited();
    env.load_dependency(two_keys()).unwrap();
    for round in 0..ROUNDS {
        let a = env
            .registry()
            .slot(&RawStr::from("cab-x"), &m)
            .unwrap()
            .unwrap();
        let b = env
            .registry()
            .slot(&RawStr::from("cab-x.ress"), &m)
            .unwrap()
            .unwrap();
        env.load_dependency(two_keys()).unwrap();
        let start = Arc::new(Barrier::new(2));
        let other = {
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                drop(b);
            })
        };
        start.wait();
        drop(a);
        other.join().unwrap();
        assert_eq!(env.inner.cache.len(), 1, "round {round}");
    }
}
