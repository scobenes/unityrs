//! A container preload walk sees every change to a key it read that is
//! published after the read (plan section 3.16, W1), and a walk's end
//! leaves the other walks' watches (W2), whatever the order in which the
//! threads take their steps. Each publishing operation (a load's
//! registration of the key, the `files` listing of it) is stopped at each
//! point between its steps, or runs whole, while a walk is stopped right
//! after its lookup read the key (in the resolver it asks on a miss), and
//! the two are let go in either order (review round 5: the listing decided
//! before it published whether to notify, so a walk that started in
//! between kept a stale miss). Walks are counted in steps
//! (`EnvStats::preload_lookups`), never by a clock.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread, a channel
// or a lock carries one.
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(test)]
use std::sync::{Arc, Mutex};

#[cfg(test)]
use super::gate::{Event, Gate, Run};
use super::registry::Hook;
use super::watch::{FILES, REGISTRY};
use super::{CabResolver, EnvOptions, Environment, Source, held};
use crate::files::File;
use crate::limits::Meter;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Bundle, Obj, bundle, row, serialized};
#[cfg(test)]
use unityrs_testkit::guard;
use unityrs_testkit::rng::Rng;

/// Stops the first `stops` lookups that miss and ask it (a walk's, right
/// after its lookup read the keys), until the test lets each go; then
/// answers none.
#[cfg(test)]
#[derive(Debug)]
struct Stops {
    count: usize,
    asked: AtomicUsize,
    told: Mutex<Sender<Told>>,
    go: Mutex<Receiver<()>>,
}

/// What a walk tells the test: it stopped in the resolver, or it ended.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
enum Told {
    Stopped,
    Ended,
}

#[cfg(test)]
impl CabResolver for Stops {
    fn resolve(&self, _name: &[u8]) -> Option<Source> {
        if self.asked.fetch_add(1, Ordering::SeqCst) < self.count {
            let _ = self.told.lock().map(|t| t.send(Told::Stopped));
            let go = self
                .go
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard::wait(&go, "the test to let the walk's lookup go");
        }
        None
    }
}

/// The walked file: top-level `w`, whose one row's preload span holds two
/// pointers to object 1 of external `foo`. Its first element's lookup
/// misses; the second asks again only if a key the first read changed.
fn walked() -> Bytes {
    Bytes::from(serialized(
        &["foo"],
        &[
            Obj::named(1, "w1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 1), (1, 1)],
                    rows: vec![row("assets/w.prefab", (0, 2), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    ))
}

/// What publishes a key the walk's lookup read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Publisher {
    /// A load of a top-level `SerializedFile` named `foo`: the registry key
    /// `foo` (and the `files` key).
    Cab,
    /// A load of a bundle named `foo`: only the `files` key `foo` (rule 4),
    /// where the walk's next lookup finds a bundle, not a `SerializedFile`.
    Listing,
}

impl Publisher {
    fn source(self) -> Source {
        let data = match self {
            Self::Cab => serialized(&[], &[Obj::named(1, "foo1", &[])]),
            Self::Listing => bundle(&[("CAB-Z", serialized(&[], &[Obj::named(1, "z1", &[])]))]),
        };
        Source::Bytes {
            name: RawStr::from("foo"),
            data: Bytes::from(data),
        }
    }

    /// What a container pass that sees the key answers.
    fn after(self) -> Result<(), String> {
        match self {
            Self::Cab => Ok(()),
            Self::Listing => Err("pointer target foo is not a SerializedFile".to_owned()),
        }
    }
}

/// An Environment with `w` loaded, a gate, and the resolver's channels.
#[cfg(test)]
struct Rig {
    env: Environment,
    gate: Gate,
    told: Receiver<Told>,
    told_tx: Sender<Told>,
    go: Sender<()>,
}

#[cfg(test)]
impl Rig {
    fn new(stops: usize) -> Self {
        let (told_tx, told) = channel();
        let (go, go_rx) = channel();
        let mut o = EnvOptions::unitypy();
        o.resolver = Some(Arc::new(Stops {
            count: stops,
            asked: AtomicUsize::new(0),
            told: Mutex::new(told_tx.clone()),
            go: Mutex::new(go_rx),
        }));
        let env = Environment::new(o);
        env.load(Source::Bytes {
            name: RawStr::from("w"),
            data: walked(),
        })
        .unwrap();
        let gate = Gate::install(&env);
        Self {
            env,
            gate,
            told,
            told_tx,
            go,
        }
    }

    /// A container pass on a thread of its own: its answer, the error's
    /// message on failure. It tells [`Told::Ended`] when it ends.
    fn walk(&self) -> Run<Result<(), String>> {
        let env = self.env.clone();
        let told = self.told_tx.clone();
        self.gate.run(None, move || {
            let answer = env
                .container()
                .map(|items| {
                    items.count();
                })
                .map_err(|e| e.to_string());
            let _ = told.send(Told::Ended);
            answer
        })
    }

    /// `p`'s load on a thread of its own, stopped at `at`.
    fn publish(&self, p: Publisher, at: Option<Hook>) -> Run<()> {
        let env = self.env.clone();
        self.gate.run(at.map(|h| (h, 1)), move || {
            drop(env.load(p.source()).unwrap());
        })
    }

    fn lookups(&self) -> u64 {
        self.env.stats().preload_lookups
    }
}

/// The points a publishing load passes.
const POINTS: [Hook; 2] = [Hook::Published, Hook::Listing];

/// Each publisher stopped at each of its points, or run whole, while a walk
/// is stopped right after its first lookup read `foo`'s keys, and let go in
/// either order: when the publisher returns before the walk goes on, the
/// walk asks again (two lookups) and answers as UnityPy's next lookup does;
/// when the walk ends first, it answers as of either side of the change.
#[test]
fn a_walk_sees_every_change_published_after_its_read() {
    let mut cases = 0;
    for p in [Publisher::Cab, Publisher::Listing] {
        for at in std::iter::once(None).chain(POINTS.map(Some)) {
            for publisher_first in [true, false] {
                if at.is_none() && !publisher_first {
                    continue;
                }
                cases += 1;
                let what = format!("{p:?} at {at:?}, publisher first {publisher_first}");
                let rig = Rig::new(1);
                let publisher = at.map(|at| {
                    let run = rig.publish(p, Some(at));
                    assert_eq!(run.next(), Event::Stopped, "{what}");
                    run
                });
                let walk = rig.walk();
                if guard::recv(&rig.told, "the walk's first lookup") == Told::Ended {
                    // The change was published before the walk's read: its
                    // one lookup found it.
                    assert_eq!(walk.finish(), p.after(), "{what}");
                    assert_eq!(rig.lookups(), 1, "{what}");
                    if let Some(run) = publisher {
                        run.finish();
                    }
                } else if publisher_first {
                    match publisher {
                        Some(run) => run.finish(),
                        None => rig.publish(p, None).finish(),
                    }
                    rig.go.send(()).unwrap();
                    assert_eq!(walk.finish(), p.after(), "{what}");
                    assert_eq!(
                        rig.lookups(),
                        2,
                        "{what}: the kept miss was not asked again"
                    );
                } else {
                    rig.go.send(()).unwrap();
                    let answer = walk.finish();
                    assert!(answer.is_ok() || answer == p.after(), "{what}: {answer:?}");
                    if let Some(run) = publisher {
                        run.finish();
                    }
                }
                // Every walk has ended: no watch is left.
                assert_eq!(rig.env.watch_entries(), 0, "{what}");
                // A walked table is not walked again (UnityPy clears it): a
                // later pass answers as the walk did, and one after a walk
                // that failed walks again, after the change.
                let again = rig.env.container().map(|_| ()).map_err(|e| e.to_string());
                assert!(again.is_ok() || again == p.after(), "{what}: {again:?}");
            }
        }
    }
    assert_eq!(cases, 2 * 5);
}

/// A key's change and the setting of the walks' flags under it are one
/// step, under the lock the key is published under (W1; review round 14:
/// the flags were set after the lock, so a walk could find the change while
/// an answer it kept under the key still held): a load of `foo` stopped
/// right after the registry published its key ([`Hook::Published`]), or
/// right after the listing published its `files` entry ([`Hook::Listed`]),
/// shows the change to another thread, and the flag a walk set up under
/// the key before is already set and in the walk's log.
#[cfg(test)]
#[test]
fn a_change_is_published_with_the_flags_under_its_key_set() {
    for (at, lock) in [(Hook::Published, REGISTRY), (Hook::Listed, FILES)] {
        let env = Environment::new(EnvOptions::unitypy());
        let gate = Gate::install(&env);
        let meter = Meter::unlimited();
        let mut walk = env
            .inner
            .registry
            .watches()
            .start(&meter, [&env.inner.registry, &env.inner.files]);
        let foo = walk.hash(b"foo");
        let flag = walk.watch(&[(lock, foo)]).unwrap();
        let load = {
            let env = env.clone();
            gate.run(Some((at, 1)), move || {
                drop(env.load(Publisher::Cab.source()).unwrap());
            })
        };
        load.stopped();
        let name = RawStr::from("foo");
        let published = if lock == REGISTRY {
            env.get_cab(&name, &meter).unwrap().is_some()
        } else {
            env.files_match(&name).is_some()
        };
        assert!(published, "{at:?}: the change is not published");
        assert!(flag.changed(), "{at:?}: published with its flag unset");
        assert_eq!(walk.fired().collect::<Vec<_>>(), [flag.id()], "{at:?}");
        load.finish();
    }
}

/// `files`: each entry's name and the file it holds.
fn listed(env: &Environment) -> Vec<(Vec<u8>, usize)> {
    env.files()
        .map(|e| {
            let File::Serialized(sf) = &e.file else {
                panic!("the scene's files are SerializedFiles");
            };
            (e.name.as_bytes().to_vec(), Arc::as_ptr(sf) as usize)
        })
        .collect()
}

/// A `files` entry's change and the setting of the walks' flags under its
/// lowered key are one step, under the listing's lock (W1). Checked as each
/// lock the loading thread takes is released ([`held::after_each_release`]),
/// so a notify that leaves the lock, ahead of the entry or behind it, is
/// seen at the release in between, where another thread could see it
/// (review round 15: it was pinned only at a hook after the lock), for each
/// thing a load can do to the listing: `first` is what is listed first, and
/// `then` the name the next load lists.
#[test]
fn at_each_release_a_listings_entry_and_flags_agree() {
    let cases = [
        ("a new entry", None, "foo"),
        ("the entry replaced", Some("foo"), "foo"),
        ("a new entry that lowers to the key", Some("foo"), "FOO"),
    ];
    let load = |env: &Environment, name: &str| {
        env.load(Source::Bytes {
            name: RawStr::from(name),
            data: Bytes::from(serialized(&[], &[Obj::named(1, "foo1", &[])])),
        })
        .unwrap();
    };
    for (what, first, then) in cases {
        let env = Environment::new(EnvOptions::unitypy());
        if let Some(first) = first {
            load(&env, first);
        }
        // Held, so a replaced entry's file keeps its address.
        let kept: Vec<_> = env.files().collect();
        let meter = Meter::unlimited();
        let mut walk = env
            .inner
            .registry
            .watches()
            .start(&meter, [&env.inner.registry, &env.inner.files]);
        let foo = walk.hash(b"foo");
        let flag = walk.watch(&[(FILES, foo)]).unwrap();
        let before = listed(&env);
        let seen = std::rc::Rc::new(std::cell::Cell::new((0_u32, 0_u32)));
        held::after_each_release(Some(std::rc::Rc::new({
            let (env, flag, seen) = (env.clone(), flag.clone(), seen.clone());
            move || {
                let changed = listed(&env) != before;
                assert_eq!(
                    flag.changed(),
                    changed,
                    "{what}: at a release the entry's change and the walks' flag differ (W1)"
                );
                let (releases, with_change) = seen.get();
                seen.set((releases + 1, with_change + u32::from(changed)));
            }
        })));
        load(&env, then);
        held::after_each_release(None);
        let (releases, with_change) = seen.get();
        assert_ne!(releases, 0, "{what}: no lock was released");
        assert_ne!(with_change, 0, "{what}: the entry was not listed");
        drop(kept);
    }
}

/// Two walks both stopped right after their first lookup read `foo`'s keys:
/// when one ends before the change is published, the other's watch is
/// still there (W2: an ending walk takes out only its own flags), and the
/// other asks again after it: three lookups.
#[test]
fn a_walks_end_leaves_another_walks_watches() {
    for p in [Publisher::Cab, Publisher::Listing] {
        let rig = Rig::new(2);
        let (done_tx, done) = channel::<usize>();
        let walks: Vec<_> = (0..2)
            .map(|i| {
                let env = rig.env.clone();
                let done_tx = done_tx.clone();
                rig.gate.run(None, move || {
                    let answer = env
                        .container()
                        .map(|items| {
                            items.count();
                        })
                        .map_err(|e| e.to_string());
                    let _ = done_tx.send(i);
                    answer
                })
            })
            .collect();
        for _ in 0..2 {
            let told = guard::recv(&rig.told, "a walk's first lookup");
            assert_eq!(told, Told::Stopped, "{p:?}");
        }
        rig.go.send(()).unwrap();
        let first = guard::recv(&done, "a walk to end");
        assert_eq!(rig.lookups(), 2, "{p:?}");
        rig.publish(p, None).finish();
        rig.go.send(()).unwrap();
        let mut walks: Vec<Option<Run<_>>> = walks.into_iter().map(Some).collect();
        let ended = walks[first].take().map(Run::finish);
        assert_eq!(ended, Some(Ok(())), "{p:?}");
        let other = walks[1 - first].take().map(Run::finish);
        assert_eq!(other, Some(p.after()), "{p:?}");
        assert_eq!(
            rig.lookups(),
            3,
            "{p:?}: the other walk's miss was not asked again"
        );
        assert_eq!(rig.env.watch_entries(), 0, "{p:?}");
    }
}

/// Walks and publishing loads at random on four threads, the loads made in
/// turn (a lock logs their order): a walk fails only as of a load that
/// returned before it started or was made while it ran (W1), and when every
/// thread is done no watch is left (W2).
#[cfg(test)]
#[test]
fn walks_and_publications_keep_their_invariants_under_a_random_stress() {
    for seed in 0..16_u64 {
        let rig = Rig::new(0);
        let log = Arc::new(Mutex::new(Vec::<Publisher>::new()));
        let threads: Vec<_> = (0..4_u64)
            .map(|t| {
                let env = rig.env.clone();
                let log = log.clone();
                std::thread::spawn(move || {
                    let mut rng = Rng::new(seed * 8 + t);
                    for _ in 0..8 {
                        let p = match rng.below(3) {
                            0 => None,
                            1 => Some(Publisher::Cab),
                            _ => Some(Publisher::Listing),
                        };
                        if let Some(p) = p {
                            let mut log = log.lock().unwrap();
                            env.load(p.source()).unwrap();
                            log.push(p);
                            continue;
                        }
                        let start = log.lock().unwrap().len();
                        let answer = env
                            .container()
                            .map(|items| {
                                items.count();
                            })
                            .map_err(|e| e.to_string());
                        let log = log.lock().unwrap();
                        // A pass that walks `w` answers as of a load made
                        // before or while it ran; one that finds the table
                        // walked (UnityPy clears it) answers nothing more.
                        let allowed: Vec<_> = (start.max(1)..=log.len())
                            .map(|k| log[k - 1].after())
                            .collect();
                        assert!(
                            answer.is_ok() || allowed.contains(&answer),
                            "{answer:?}: {allowed:?}"
                        );
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(rig.env.watch_entries(), 0, "seed {seed}");
        assert_eq!(rig.env.inner.registry.watches().walks(), 0, "seed {seed}");
    }
}
