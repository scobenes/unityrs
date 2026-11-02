//! A load another thread makes while a container preload walk is in a
//! lookup's call to the resolver: the walk gives what UnityPy's gives with
//! the load made at that point, as `tools/fixtures/env.py` recorded it for
//! the race scenes (`env.json`'s `race_scenes`; the core's unit test
//! `env::race_tests` stops the walk at every point between its steps). The
//! resolver has another thread make the load and waits for it to end
//! (review round 14: areas d's and x's probes, and the case the removed
//! `preload.rs` test
//! `a_load_on_another_thread_puts_back_no_index_the_walk_dereferenced`
//! pinned the other way: UnityPy's walk finds the change at a later row).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value as J, json};
use unityrs_core::container::ContainerPath;
use unityrs_core::env::{CabResolver, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::error::{Error, NotFound};
use unityrs_core::files::File;
use unityrs_core::{Bytes, FileContext, ObjectRef, RawStr};
use unityrs_testkit::env::race_scenes;
use unityrs_testkit::guard;

/// UnityPy's `env.path` over a race scene's resolver files (the file of the
/// name asked, else the first whose name ends with it, keyed `@/<name>`),
/// which, the first time it is asked for `trigger`, has another thread make
/// the scene's other load and waits for it to end.
#[derive(Debug)]
struct Racing {
    files: Vec<(Vec<u8>, Bytes)>,
    trigger: &'static [u8],
    asked: AtomicBool,
    go: Mutex<Sender<bool>>,
    done: Mutex<Receiver<()>>,
}

impl CabResolver for Racing {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        if name == self.trigger && !self.asked.swap(true, Ordering::SeqCst) {
            self.go.lock().unwrap().send(true).unwrap();
            guard::wait(&self.done.lock().unwrap(), "the other thread's load");
        }
        let (n, data) = self
            .files
            .iter()
            .find(|(n, _)| n == name)
            .or_else(|| self.files.iter().find(|(n, _)| n.ends_with(name)))?;
        let mut key = b"@/".to_vec();
        key.extend_from_slice(n);
        Some(Source::Bytes {
            name: RawStr::new(key),
            data: data.clone(),
        })
    }

    fn find_file(&self, name: &[u8]) -> Result<Option<Source>, Error> {
        self.resolve(name)
            .map(Some)
            .ok_or_else(|| Error::NotFound(NotFound::Cab(RawStr::new(name.to_vec()))))
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn path_of(env: &Environment, o: &ObjectRef) -> J {
    match env.container_path(o) {
        Ok(None) => J::Null,
        Ok(Some(ContainerPath::Str(s))) => json!(text(s.as_bytes())),
        other => json!(format!("{other:?}")),
    }
}

/// `env.py`'s `paths` and `dep_paths`: every object's `container` path,
/// and `[key, path id, path]` for the objects of each dependency.
fn paths(env: &Environment) -> (J, J) {
    let paths: Vec<J> = env.objects().map(|o| path_of(env, &o.unwrap())).collect();
    let mut deps = Vec::new();
    for f in env.files().filter(|f| f.is_dependency) {
        let File::Serialized(sf) = &f.file else {
            panic!("a scene's resolver file is a SerializedFile");
        };
        let key = text(f.name.as_bytes());
        deps.extend(
            sf.objects()
                .map(|o| json!([key, o.path_id(), path_of(env, &o)])),
        );
    }
    (json!(paths), json!(deps))
}

/// What UnityPy recorded for race scene `name` with its other load made
/// before its walk looks at element `index` of row `row`'s span: `paths`
/// and `dep_paths`.
fn unitypy(name: &str, row: usize, index: usize) -> (J, J) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/d/env.json");
    let doc: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let case = doc["race_scenes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap();
    let point = case["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["at"] == json!([row, index]))
        .unwrap();
    (point["paths"].clone(), point["dep_paths"].clone())
}

/// Race scene `name` under `o`: its files loaded, then a container pass
/// whose resolver call for `trigger` has another thread make the other
/// load; the Environment, once both have ended.
fn raced(name: &str, trigger: &'static [u8], mut o: EnvOptions) -> Environment {
    let (_, race) = race_scenes().into_iter().find(|(n, _)| *n == name).unwrap();
    let (go_tx, go) = channel::<bool>();
    let (done_tx, done) = channel::<()>();
    let files = race
        .scene
        .resolver
        .iter()
        .map(|(n, d)| (n.clone(), Bytes::from(d.clone())))
        .collect();
    let racing = Arc::new(Racing {
        files,
        trigger,
        asked: AtomicBool::new(false),
        go: Mutex::new(go_tx.clone()),
        done: Mutex::new(done),
    });
    o.resolver = Some(racing.clone());
    let env = Environment::new(o);
    for (n, d) in &race.scene.files {
        env.load(Source::Bytes {
            name: RawStr::new(n.clone()),
            data: Bytes::from(d.clone()),
        })
        .unwrap();
    }
    let other = env.clone();
    let (n, d) = race.other;
    let loader = thread::spawn(move || {
        while guard::recv(&go, "a load request") {
            other
                .load(Source::Bytes {
                    name: RawStr::new(n.clone()),
                    data: Bytes::from(d.clone()),
                })
                .unwrap();
            done_tx.send(()).unwrap();
        }
    });
    let walk = env.clone();
    let rows = guard::finishes("the container pass", move || {
        walk.container().map(Iterator::count).unwrap()
    });
    assert_ne!(rows, 0, "{name}");
    go_tx.send(false).unwrap();
    loader.join().unwrap();
    assert!(racing.asked.load(Ordering::SeqCst), "{name}: no call asked");
    env
}

fn options(mode: LoadMode, batch: bool) -> EnvOptions {
    let mut o = if batch {
        EnvOptions::batch(Arc::new(FileContext::unitypy()))
    } else {
        EnvOptions::unitypy()
    };
    o.load = mode;
    o
}

/// A load another thread makes is seen at a later row by an index the walk
/// dereferenced before it, under every mode and option set: row 0 spans
/// `CAB-A`'s 5 and `t`'s 1, row 1 the first; the resolver's call for `t`
/// waits for another thread's load of a `CAB-A` holding 5, so UnityPy's
/// row 1 gives that 5 its path (review round 14: the walk put back no index
/// for a change another thread made, so row 1 gave none). Three
/// dereferences and three lookups: the first index's second is the one the
/// change puts back.
#[test]
fn a_load_on_another_thread_puts_back_the_indexes_whose_answers_it_changed() {
    let (want, deps) = unitypy("race_in_a_lookup", 0, 1);
    for (mode, batch) in [
        (LoadMode::Eager, false),
        (LoadMode::Eager, true),
        (LoadMode::Lazy, false),
        (LoadMode::Lazy, true),
    ] {
        let env = raced("race_in_a_lookup", b"t", options(mode, batch));
        let what = format!("{mode:?} batch {batch}");
        assert_eq!(paths(&env), (want.clone(), deps.clone()), "{what}");
        let stats = env.stats();
        assert_eq!(stats.preload_dereferences, 3, "{what}");
        assert_eq!(stats.preload_lookups, 3, "{what}");
    }
}

/// Another thread's load of a key, then the walk's own (review round 14,
/// area d's probe): row 0 spans `CAB-A`'s 5, `t`'s 1 and `a`'s 9, row 1 the
/// first; the resolver's call for `t` waits for another thread's load of a
/// `CAB-A` without 5, then `a` loads `env.path`'s `cab-a` (5 and 9) under
/// that key. UnityPy's row 1 finds the walk's own copy's 5 (the other
/// thread's change had taken the walk's flag from the key, so its own
/// change put back nothing and row 1 gave none). Four dereferences and four
/// lookups, as with no other load.
#[test]
fn another_threads_change_of_a_key_leaves_the_walks_own_change_its_revisit() {
    let (want, deps) = unitypy("race_before_the_walks_own_load", 0, 1);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let env = raced("race_before_the_walks_own_load", b"t", options(mode, false));
        assert_eq!(paths(&env), (want.clone(), deps.clone()), "{mode:?}");
        let stats = env.stats();
        assert_eq!(stats.preload_dereferences, 4, "{mode:?}");
        assert_eq!(stats.preload_lookups, 4, "{mode:?}");
    }
}

/// Another thread's load of `CAB-Q` made in the walk's own lookup, before
/// the walk's own load of `env.path`'s `cab-q` takes the key (review round
/// 14, area x's probe; the scene `preload_revisits_after_load`): UnityPy's
/// row `b` finds the walk's copy and gives its 6 that row's path, as with
/// the other load before or after the walk (its flag was set by the other
/// thread's change and not logged, so the walk's own change put back
/// nothing). Three dereferences, as with no other load.
#[test]
fn another_threads_change_in_the_walks_lookup_comes_before_its_own() {
    let (want, deps) = unitypy("race_around_the_walks_own_load", 0, 1);
    assert_eq!(deps[1], json!(["@/cab-q", 6, "assets/b.prefab"]));
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let env = raced("race_around_the_walks_own_load", b"q", options(mode, false));
        assert_eq!(paths(&env), (want.clone(), deps.clone()), "{mode:?}");
        assert_eq!(env.stats().preload_dereferences, 3, "{mode:?}");
    }
}
