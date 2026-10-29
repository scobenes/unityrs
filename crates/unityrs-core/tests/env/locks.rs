//! The lock rules: what a resolver or a fetch may do while the core calls it.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use unityrs_core::env::{Cab, CabResolver, EnvOptions, Environment, Fetch, LoadMode, PPtr, Source};
use unityrs_core::error::Error;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{Obj, serialized};

use crate::{file_of, key, name_of, named_bundle, object, serialized_cab, src, unlimited};

/// Runs `f` on its own thread and fails the test if it has not finished in
/// ten seconds (a deadlock), without waiting for it.
fn within_ten_seconds(f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("finished within 10 s");
}

/// A resolver that calls back into the Environment (`get_cab` and `deref`)
/// from inside its call, and one that takes a lock another thread holds
/// while it uses the Environment: neither deadlocks, in either mode.
#[test]
fn a_resolver_and_a_fetch_may_call_the_environment_and_take_caller_locks() {
    #[derive(Debug)]
    struct Reentrant {
        env: Mutex<Option<Environment>>,
        lock: Arc<Mutex<()>>,
        target: Bytes,
    }
    impl CabResolver for Reentrant {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            let _held = self.lock.lock().unwrap();
            let env = self.env.lock().unwrap().clone()?;
            let _ = env.get_cab(&RawStr::from("cab-a"), &Meter::unlimited());
            if let Ok(Some(Cab::Serialized(sf))) =
                env.get_cab(&RawStr::from("cab-a"), &Meter::unlimited())
            {
                let _ = env.deref(
                    &sf,
                    PPtr {
                        file_id: 0,
                        path_id: 1,
                    },
                );
            }
            (name == b"cab-b").then(|| Source::Fetch {
                name: RawStr::from("cab-b"),
                fetch: Arc::new(ReentrantFetch {
                    env: env.clone(),
                    lock: self.lock.clone(),
                    data: self.target.clone(),
                }),
            })
        }
    }
    #[derive(Debug)]
    struct ReentrantFetch {
        env: Environment,
        lock: Arc<Mutex<()>>,
        data: Bytes,
    }
    impl Fetch for ReentrantFetch {
        fn fetch(&self) -> Result<Bytes, Error> {
            let _held = self.lock.lock().unwrap();
            let _ = self
                .env
                .get_cab(&RawStr::from("cab-a"), &Meter::unlimited());
            Ok(self.data.clone())
        }
    }
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        within_ten_seconds(move || {
            let scene = unityrs_testkit::env::cross_bundle_pointer();
            let lock = Arc::new(Mutex::new(()));
            let resolver = Arc::new(Reentrant {
                env: Mutex::new(None),
                lock: lock.clone(),
                // A `SerializedFile`, so the pointer's target is in it (a
                // bundle would be the pointer's container, with no objects).
                target: Bytes::from(serialized(&[], &[Obj::named(7, "b7", &[])])),
            });
            let mut o = EnvOptions::unitypy();
            o.load = mode;
            o.resolver = Some(resolver.clone());
            let env = Environment::new(o);
            *resolver.env.lock().unwrap() = Some(env.clone());
            env.load(file_of(&scene, 0)).unwrap();
            // Another thread uses the Environment while holding the lock
            // the resolver and the fetch take.
            let other = {
                let env = env.clone();
                let lock = lock.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let _held = lock.lock().unwrap();
                        let _ = env.get_cab(&RawStr::from("cab-a"), &Meter::unlimited());
                        let _ = env.files().count();
                    }
                })
            };
            let sf = serialized_cab(&env, "cab-a");
            let t = env
                .deref(
                    &sf,
                    PPtr {
                        file_id: 1,
                        path_id: 7,
                    },
                )
                .unwrap();
            assert_eq!(name_of(&t), "b7");
            other.join().unwrap();
            *resolver.env.lock().unwrap() = None;
        });
    }
}

/// Meets another thread from inside a callback: says it is inside, then
/// waits for the other thread to finish what it does meanwhile.
#[derive(Debug)]
struct Rendezvous {
    inside: Mutex<Option<mpsc::Sender<()>>>,
    done: Mutex<Option<mpsc::Receiver<()>>>,
}

impl Rendezvous {
    fn new() -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (inside_tx, inside_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let r = Arc::new(Self {
            inside: Mutex::new(Some(inside_tx)),
            done: Mutex::new(Some(done_rx)),
        });
        (r, inside_rx, done_tx)
    }

    /// Meets once; later calls pass.
    fn meet(&self) {
        let Some(tx) = self.inside.lock().unwrap().take() else {
            return;
        };
        let _ = tx.send(());
        let rx = self.done.lock().unwrap().take().unwrap();
        rx.recv_timeout(Duration::from_secs(60)).expect(
            "the other thread's loads finished while the core called this one: \
             a core lock is held across a resolver or a fetch",
        );
    }
}

/// What a thread does while the core calls another thread's resolver or
/// fetch: writers (a load, a dependency, a parse through a slot), which a
/// read lock held across the call would block, and readers.
fn writers(env: &Environment, i: usize) {
    let name = format!("CAB-W{i}");
    env.load(src(&format!("w{i}.bundle"), &named_bundle(&name, 16)))
        .unwrap();
    let dep = format!("CAB-V{i}");
    env.load_dependency(src(&format!("v{i}.bundle"), &named_bundle(&dep, 16)))
        .unwrap();
    let _ = serialized_cab(env, &name.to_lowercase());
    let _ = env.get_cab(&key("cab-b"), &unlimited());
    let _ = env.get_cab(&key("cab-f"), &unlimited());
    let _ = env.files().count();
    let _ = env.registry().keys().count();
}

/// While the core calls a resolver or a fetch, another thread loads,
/// registers and looks up (writers, which a read lock held across the call
/// would block), in both modes, while the callback waits for it (review
/// round 1: only readers ran beside a callback, so a lock held across one
/// went unseen).
#[test]
fn a_resolver_or_a_fetch_may_wait_for_a_thread_that_loads() {
    #[derive(Debug)]
    struct Meeting {
        meet: Arc<Rendezvous>,
        data: Bytes,
        fetch: bool,
    }
    impl CabResolver for Meeting {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            if name != b"cab-b" {
                return None;
            }
            if self.fetch {
                return Some(Source::Fetch {
                    name: RawStr::from("cab-b"),
                    fetch: Arc::new(MeetingFetch {
                        meet: self.meet.clone(),
                        data: self.data.clone(),
                    }),
                });
            }
            self.meet.meet();
            Some(Source::Bytes {
                name: RawStr::from("cab-b"),
                data: self.data.clone(),
            })
        }
    }
    #[derive(Debug)]
    struct MeetingFetch {
        meet: Arc<Rendezvous>,
        data: Bytes,
    }
    impl Fetch for MeetingFetch {
        fn fetch(&self) -> Result<Bytes, Error> {
            self.meet.meet();
            Ok(self.data.clone())
        }
    }
    let target = Bytes::from(serialized(&[], &[Obj::named(7, "b7", &[])]));
    let mut round = 0;
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        for fetch in [false, true] {
            round += 1;
            let (meet, inside, done) = Rendezvous::new();
            let mut o = EnvOptions::unitypy();
            o.load = mode;
            o.resolver = Some(Arc::new(Meeting {
                meet: meet.clone(),
                data: target.clone(),
                fetch,
            }));
            let env = Environment::new(o);
            let scene = unityrs_testkit::env::cross_bundle_pointer();
            env.load(file_of(&scene, 0)).unwrap();
            let caller = {
                let env = env.clone();
                std::thread::spawn(move || {
                    let sf = serialized_cab(&env, "cab-a");
                    let t = env.deref(
                        &sf,
                        PPtr {
                            file_id: 1,
                            path_id: 7,
                        },
                    );
                    name_of(&t.unwrap())
                })
            };
            inside
                .recv_timeout(Duration::from_secs(60))
                .expect("the core called the resolver");
            writers(&env, round);
            done.send(()).unwrap();
            assert_eq!(caller.join().unwrap(), "b7", "{mode:?} fetch {fetch}");
        }
    }
}

/// A fetch that meets another thread once it is armed.
#[derive(Debug)]
struct ArmedFetch {
    on: std::sync::atomic::AtomicBool,
    meet: Arc<Rendezvous>,
    data: Bytes,
}

impl Fetch for ArmedFetch {
    fn fetch(&self) -> Result<Bytes, Error> {
        if self.on.load(Ordering::SeqCst) {
            self.meet.meet();
        }
        Ok(self.data.clone())
    }
}

/// As above, for the fetch of an evicted `Fetch` dependency's reopen: another
/// thread loads, and asks for the same dependency, while it runs.
#[test]
fn an_evicted_dependencys_fetch_may_wait_for_a_thread_that_loads() {
    for (round, mode) in [(1, LoadMode::Eager), (2, LoadMode::Lazy)] {
        let (meet, inside, done) = Rendezvous::new();
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        o.dep_cache_bytes = 1;
        let env = Environment::new(o);
        let fetcher = Arc::new(ArmedFetch {
            on: std::sync::atomic::AtomicBool::new(false),
            meet,
            data: Bytes::from(named_bundle("CAB-F", 16)),
        });
        drop(
            env.load_dependency(Source::Fetch {
                name: RawStr::from("f.bundle"),
                fetch: fetcher.clone(),
            })
            .unwrap(),
        );
        drop(
            env.load_dependency(src("g.bundle", &named_bundle("CAB-G", 16)))
                .unwrap(),
        );
        assert!(env.stats().dependencies_evicted > 0);
        fetcher.on.store(true, Ordering::SeqCst);
        let caller = {
            let env = env.clone();
            std::thread::spawn(move || name_of(&object(&serialized_cab(&env, "cab-f"), 1)))
        };
        inside
            .recv_timeout(Duration::from_secs(60))
            .expect("the core called the fetch");
        writers(&env, round);
        done.send(()).unwrap();
        assert_eq!(caller.join().unwrap(), "CAB-F", "{mode:?}");
    }
}
