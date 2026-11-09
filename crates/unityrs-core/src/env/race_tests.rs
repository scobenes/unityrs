//! A container preload walk races a load another thread makes, and gives
//! what UnityPy's walk gives with each key the load changes changed at the
//! point of the walk at which it was published (`tools/fixtures/env.py`'s
//! race scenes, recorded by `env_race.py`; plan section 3.16, W1 and item
//! 6). Each point the walk reaches ([`Hook::Search`], [`Hook::Step`]), it
//! is stopped there while another thread makes the load, whole or stopped
//! itself right after it published its first CAB's key
//! ([`Hook::Published`]), the rest published after the walk; the load is
//! also made before and after the walk.
//! Every point under `Eager` and `Lazy`, by the Environment's hooks
//! ([`super::gate`]), never by repetition or timing (review round 14: a
//! load another thread made before one of the walk's own, of the same key,
//! left an index the walk had dereferenced off for good, and a change it
//! made between two rows was seen by a later lookup but put back no index,
//! so which file got a later row's path depended on the threads' timing).
//!
//! Loads that race leave what UnityPy leaves when they are made one after
//! the other in the order of their numbers (review round 16): a load
//! stopped after its first key while another loads and a container pass
//! runs, and a load stopped before its `files` entry while another loads
//! whole, or two more, one of them of its key
//! (`tools/fixtures/env_race.py`'s `env_threads.json`).

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread or a lock
// carries one.
#[cfg(test)]
use std::sync::{Arc, Mutex};

use serde_json::{Value as J, json};

#[cfg(test)]
use super::gate::Gate;
use super::registry::{Hook, HookFn};
use super::{Cab, CabResolver, EnvOptions, Environment, LoadMode, Source};
use crate::container::ContainerPath;
use crate::error::{Error, NotFound};
use crate::files::File;
use crate::limits::Meter;
use crate::raw::RawStr;
use bytes::Bytes;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The recorded race scenes (`env.json`'s `race_scenes`).
fn cases() -> Vec<J> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/d/env.json");
    let doc: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    doc["race_scenes"].as_array().unwrap().clone()
}

/// A scene's `(name, bytes)` pairs.
fn pairs(v: &J) -> Vec<(Vec<u8>, Bytes)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let name = p[0].as_str().unwrap().as_bytes().to_vec();
            (name, Bytes::from(unhex(p[1].as_str().unwrap())))
        })
        .collect()
}

/// UnityPy's `env.path`, as `tests/env_fixtures/run.rs`'s `SceneResolver`:
/// the file of the name asked, else the first whose name ends with it,
/// keyed `@/<name>`; `find_file` raises where nothing matches.
#[derive(Debug)]
struct Path(Vec<(Vec<u8>, Bytes)>);

impl CabResolver for Path {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        let (n, data) = self
            .0
            .iter()
            .find(|(n, _)| n == name)
            .or_else(|| self.0.iter().find(|(n, _)| n.ends_with(name)))?;
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

/// An Environment under `mode` with the case's files loaded.
fn loaded(case: &J, mode: LoadMode) -> Environment {
    let mut o = EnvOptions::unitypy();
    o.load = mode;
    o.resolver = Some(std::sync::Arc::new(Path(pairs(&case["resolver"]))));
    let env = Environment::new(o);
    for (name, data) in pairs(&case["files"]) {
        env.load(Source::Bytes {
            name: RawStr::new(name),
            data,
        })
        .unwrap();
    }
    env
}

/// The case's other load.
fn other(env: &Environment, case: &J) {
    let name = case["other"][0].as_str().unwrap();
    let data = unhex(case["other"][1].as_str().unwrap());
    env.load(Source::Bytes {
        name: RawStr::from(name),
        data: Bytes::from(data),
    })
    .unwrap();
}

/// `env.container`'s rows, as `env.py` records them.
fn rows(env: &Environment) -> J {
    let rows: Vec<J> = env
        .container()
        .unwrap()
        .map(|i| {
            let i = i.unwrap();
            json!([
                i.row.path.as_ref().map(|p| text(p.as_bytes())),
                i.row.preload_index,
                i.row.preload_size,
                i.row.file_id,
                i.row.path_id
            ])
        })
        .collect();
    json!(rows)
}

fn kind(f: &File) -> &'static str {
    match f {
        File::Serialized(_) => "serialized",
        File::Bundle(_) => "bundle",
        File::Raw(_) | File::Web(_) => "raw",
    }
}

fn path_of(env: &Environment, o: &crate::object::ObjectRef) -> J {
    match env.container_path(o) {
        Ok(None) => J::Null,
        Ok(Some(ContainerPath::Str(s))) => json!(text(s.as_bytes())),
        Ok(Some(ContainerPath::NotAString)) => json!("<not a str>"),
        Err(e) => json!(format!("core: {e}")),
    }
}

/// What `env.py` records after the container (its `paths`, `dep_paths`
/// and the state), with `container` the walk's rows.
fn record(env: &Environment, container: &J) -> J {
    let objects: Vec<_> = env.objects().map(Result::unwrap).collect();
    let paths: Vec<J> = objects.iter().map(|o| path_of(env, o)).collect();
    let mut dep_paths = Vec::new();
    for f in env.files().filter(|f| f.is_dependency) {
        let File::Serialized(sf) = &f.file else {
            panic!("a scene's resolver file is a SerializedFile");
        };
        let key = text(f.name.as_bytes());
        dep_paths.extend(
            sf.objects()
                .map(|o| json!([key, o.path_id(), path_of(env, &o)])),
        );
    }
    let files: Vec<J> = env
        .files()
        .map(|f| json!([text(f.name.as_bytes()), f.is_dependency, kind(&f.file)]))
        .collect();
    let cabs: Vec<J> = env
        .registry()
        .keys()
        .map(|k| json!(text(k.as_bytes())))
        .collect();
    let objects: Vec<J> = objects
        .iter()
        .map(|o| {
            let name = o.file().name().as_bytes();
            let name = &name[name.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1)..];
            json!([text(name), o.path_id(), o.class_id()])
        })
        .collect();
    json!({"container": container, "paths": paths, "dep_paths": dep_paths,
        "files": files, "cabs": cabs, "objects": objects})
}

/// UnityPy's record at `at` (`"before"`, `[row, index]`, `"after"`), less
/// its `at` (and, for a load of more than one key, its `first_key`).
fn recorded(case: &J, at: &J) -> J {
    let mut point = case["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["at"] == *at)
        .unwrap_or_else(|| panic!("{}: UnityPy has no point {at}", case["name"]))
        .clone();
    point.as_object_mut().unwrap().remove("at");
    point
}

/// What a run whose load is stopped right after it published its first key
/// is compared with at `point` (a recorded point less its `at`): UnityPy's
/// walk with that key changed there and the load's other keys after the
/// walk (`first_key`, recorded for a load of more than one key), else the
/// point itself.
fn first_key(point: &J) -> J {
    match point.get("first_key") {
        Some(split) => split.clone(),
        None => compared(point, false),
    }
}

/// The points the walk reaches, in order, by a walk no load races.
#[cfg(test)]
fn points(case: &J, mode: LoadMode) -> Vec<Hook> {
    let env = loaded(case, mode);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    *env.inner.registry.hook.lock().unwrap() = Some(HookFn(Arc::new(move |h| {
        if matches!(h, Hook::Search { .. } | Hook::Step { .. }) {
            log.lock().unwrap().push(h);
        }
    })));
    rows(&env);
    *env.inner.registry.hook.lock().unwrap() = None;
    seen.lock().unwrap().clone()
}

/// What a run is compared on: everything, or, when the load's `files`
/// entry is made after the walk (the load stopped), what does not list it.
fn compared(r: &J, whole: bool) -> J {
    if whole {
        let mut r = r.clone();
        if let Some(r) = r.as_object_mut() {
            r.remove("first_key");
        }
        return r;
    }
    json!({"paths": r["paths"], "dep_paths": r["dep_paths"], "cabs": r["cabs"],
        "container": r["container"]})
}

/// What run `got` is compared with at UnityPy's `point`: the load whole,
/// or stopped right after its first key ([`first_key`]).
fn wanted(point: &J, whole: bool) -> J {
    if whole {
        compared(point, true)
    } else {
        first_key(point)
    }
}

/// Before and after the walk, and at each point it reaches while another
/// thread makes the load, whole or stopped after it published its first
/// CAB's key: the paths, the dependencies' paths and the CABs UnityPy's
/// walk gives with the load made there (when stopped, its first key made
/// there and the rest after the walk), and, when the load is made whole,
/// its `files`, objects and rows. Every run is checked, and every failure
/// listed: one whose answer is UnityPy's at no point of the walk (no order
/// of the two calls gives it) and one whose answer is UnityPy's at another
/// point (the walk did not see a load that had ended, or saw one later).
#[cfg(test)]
#[test]
fn a_walk_gives_unitypys_answer_with_another_threads_load_at_each_point() {
    let mut runs = 0;
    let mut failures = Vec::new();
    for case in cases() {
        let name = case["name"].as_str().unwrap().to_owned();
        let at_all = |got: &J, whole: bool| {
            case["points"].as_array().unwrap().iter().any(|p| {
                let mut p = p.clone();
                p.as_object_mut().unwrap().remove("at");
                wanted(&p, whole) == compared(got, whole)
            })
        };
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let env = loaded(&case, mode);
            other(&env, &case);
            let got = record(&env, &rows(&env));
            assert_eq!(
                got,
                recorded(&case, &json!("before")),
                "{name} {mode:?} before"
            );
            let env = loaded(&case, mode);
            let container = rows(&env);
            other(&env, &case);
            let got = record(&env, &container);
            assert_eq!(
                got,
                recorded(&case, &json!("after")),
                "{name} {mode:?} after"
            );
            let points = points(&case, mode);
            assert!(
                points.iter().any(|h| matches!(h, Hook::Step { .. })),
                "{name}: the walk took no step"
            );
            for h in points {
                let (Hook::Search { row, index } | Hook::Step { row, index }) = h else {
                    unreachable!("points are searches and steps");
                };
                let want = recorded(&case, &json!([row, index]));
                for stopped in [false, true] {
                    let what = format!("{name} {mode:?} at {h:?}, the load stopped {stopped}");
                    let env = loaded(&case, mode);
                    let gate = Gate::install(&env);
                    let walk = {
                        let env = env.clone();
                        gate.run(Some((h, 1)), move || rows(&env))
                    };
                    walk.stopped();
                    let load = {
                        let env = env.clone();
                        let case = case.clone();
                        let at = stopped.then_some((Hook::Published, 1));
                        gate.run(at, move || other(&env, &case))
                    };
                    let container = if stopped {
                        load.stopped();
                        let container = walk.finish();
                        load.finish();
                        container
                    } else {
                        load.finish();
                        walk.finish()
                    };
                    let got = record(&env, &container);
                    let whole = !stopped;
                    if compared(&got, whole) != wanted(&want, whole) {
                        let how = if at_all(&got, whole) {
                            "UnityPy's at another point"
                        } else {
                            "UnityPy's at no point"
                        };
                        let want = wanted(&want, whole);
                        failures.push(format!(
                            "{what}: {how}: paths {} dep_paths {}, want {} {}",
                            got["paths"], got["dep_paths"], want["paths"], want["dep_paths"]
                        ));
                    }
                    runs += 1;
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(runs >= 60, "{runs} runs");
}

/// A load's change to a key and the new container index generation it
/// starts are one step, under the registry's lock (plan section 3.16, P1
/// with W1; review round 15: the generation was bumped after the lock, so
/// a thread that found the new key could still read the old generation's
/// built index): with a pass built, another thread's load of the scene's
/// other file is stopped right after it published its CAB's key
/// ([`Hook::Published`]); the CAB the key now finds is the other file's,
/// and its object 7's container path is UnityPy's once the load is made
/// (the `"after"` point), which walks that file's own table.
#[cfg(test)]
#[test]
fn a_change_is_found_with_its_new_container_index_generation() {
    let case = cases()
        .into_iter()
        .find(|c| c["name"] == "race_replaced_by_a_table_of_its_own")
        .unwrap();
    let after = recorded(&case, &json!("after"));
    let at = after["objects"]
        .as_array()
        .unwrap()
        .iter()
        .position(|o| o[0] == "CAB-A" && o[1] == 7)
        .unwrap();
    let want = &after["paths"][at];
    assert!(want.is_string(), "UnityPy gives 7 no path: {want}");
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let env = loaded(&case, mode);
        rows(&env);
        let gate = Gate::install(&env);
        let load = {
            let env = env.clone();
            let case = case.clone();
            gate.run(Some((Hook::Published, 1)), move || other(&env, &case))
        };
        load.stopped();
        let found = env
            .get_cab(&RawStr::from("CAB-A"), &Meter::unlimited())
            .unwrap();
        let Some(Cab::Serialized(sf)) = found else {
            panic!("{mode:?}: CAB-A is not a SerializedFile");
        };
        let seven = sf
            .objects()
            .find(|o| o.path_id() == 7)
            .unwrap_or_else(|| panic!("{mode:?}: the CAB found is not the other file's"));
        assert_eq!(path_of(&env, &seven), *want, "{mode:?}");
        load.finish();
    }
}

/// UnityPy's runs of the orders two threads make loads in
/// (`env_threads.json`, `tools/fixtures/env_race.py`).
fn threads() -> J {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/d/env_threads.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A recorded run less what names it.
fn without(case: &J, keys: &[&str]) -> J {
    let mut case = case.clone();
    for k in keys {
        case.as_object_mut().unwrap().remove(*k);
    }
    case
}

/// The options a race runs under: UnityPy's, `Lazy`, and the batch
/// engine's (`Lazy`, resolver loads kept out of `files`).
fn race_options() -> [(&'static str, EnvOptions); 3] {
    let mut lazy = EnvOptions::unitypy();
    lazy.load = LoadMode::Lazy;
    let batch = EnvOptions::batch(Arc::new(crate::options::FileContext::unitypy()));
    [
        ("eager", EnvOptions::unitypy()),
        ("lazy", lazy),
        ("batch", batch),
    ]
}

/// Review round 16's first race (scene `nested_between_loads`):
/// `c.bundle`'s load is stopped right after it published its first key
/// ([`Hook::Published`]), `b.bundle` loads whole and a container pass is
/// built, then `c.bundle`'s load goes on. Its `n` ranks between the key's
/// position (`a.bundle`'s) and its value (`b.bundle`'s), so it changes no
/// key; under `Lazy` it lists its dropped slot, whose reach registers
/// `CAB-Z`, and starts a new container index generation in that step, so
/// the next container pass walks `CAB-Z`'s table: the container, every
/// object's path and the state are UnityPy's for the same loads with
/// `c.bundle`'s registrations past its first made after `b.bundle`'s load
/// and a container listing (`env_race.py`'s `held_case`), under every
/// option set. The listing started no generation, so the pass built before
/// it was kept and object 7 of `CAB-Z` had no path under `Lazy`.
/// `held_case` is UnityPy's answer only because no later file registers a
/// key the held load registers past its first (it asserts so): were
/// `c.bundle`'s held registration a `SerializedFile` of a key `b.bundle`
/// also registers, UnityPy there would keep `c.bundle`'s as the value, and
/// the core, as the loads' numbers order them, `b.bundle`'s.
#[cfg(test)]
#[test]
fn a_load_between_a_keys_position_and_value_starts_a_container_index_generation() {
    let doc = threads();
    let case = doc["held"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "nested_between_loads")
        .expect("the scene is recorded")
        .clone();
    let held = usize::try_from(case["held"].as_u64().unwrap()).unwrap();
    let want = without(&case, &["name", "held", "before"]);
    let paths = want["paths"].as_array().unwrap();
    assert!(
        paths.contains(&json!("assets/z.prefab")),
        "UnityPy gives CAB-Z's objects no path: {paths:?}"
    );
    let scene = unityrs_testkit::env::nested_between_loads();
    let src = |i: usize| Source::Bytes {
        name: RawStr::new(scene.files[i].0.clone()),
        data: Bytes::from(scene.files[i].1.clone()),
    };
    for (what, options) in race_options() {
        let env = Environment::new(options);
        for i in 0..held {
            env.load(src(i)).unwrap();
        }
        let gate = Gate::install(&env);
        let load = {
            let (env, source) = (env.clone(), src(held));
            gate.run(Some((Hook::Published, 1)), move || {
                drop(env.load(source).unwrap());
            })
        };
        load.stopped();
        for i in held + 1..scene.files.len() {
            env.load(src(i)).unwrap();
        }
        assert_eq!(rows(&env), case["before"], "{what}: the pass before");
        load.finish();
        let got = record(&env, &rows(&env));
        for part in ["paths", "files"] {
            assert_eq!(got[part], want[part], "{what}: {part}");
        }
        assert_eq!(got, want, "{what}");
    }
}

/// Review round 16's second race (scene `listings_raced`): the first file's
/// load is stopped after its registration, before its `files` entry
/// ([`Hook::Listing`]), or right after its key is published, before the
/// number its entry is listed at is read ([`Hook::Published`]), while the
/// second file loads whole; then the first lists. `files` is a dict by the loads' numbers, as the registry is, so
/// the container, every object's path and the state are UnityPy's for the
/// two loads made in the order of their numbers, first file first, whose
/// other order UnityPy answers otherwise: under the files' own names (the
/// first's key first) and both under `x` (the second's file the value),
/// under every option set, and with the first file found by `find_file` in
/// the resolver's `env.path` (a dependency listed under its path), under
/// the option sets that list resolver loads. `files` took the order the
/// threads listed in, so the first load's file was listed last: `x` held
/// the first file in `files` and the second in the registry, and `p` came
/// after `q`.
#[cfg(test)]
#[test]
fn two_loads_that_race_their_listings_leave_unitypys_order_of_their_numbers() {
    let doc = threads();
    let scene = unityrs_testkit::env::listings_raced();
    let run = |names: &J, find: bool, order: [u64; 2]| -> J {
        let found = doc["orders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["names"] == *names && c["find"] == find && c["order"] == json!(order))
            .unwrap_or_else(|| panic!("UnityPy has no run of {names}, find {find}, in {order:?}"));
        assert_eq!(found["picks"], json!([0, 1]), "the scene's two files");
        without(found, &["name", "picks", "names", "find", "order"])
    };
    let mut cases = 0;
    for (names, find) in [(["p", "q"], false), (["x", "x"], false), (["p", "q"], true)] {
        let names_j = json!(names);
        let want = run(&names_j, find, [0, 1]);
        assert_ne!(
            want,
            run(&names_j, find, [1, 0]),
            "{names:?}, find {find}: UnityPy's orders agree"
        );
        let src = |i: usize| Source::Bytes {
            name: RawStr::from(names[i]),
            data: Bytes::from(scene.files[i].1.clone()),
        };
        let options: Vec<_> = if find {
            // `env.path` holds the first file under its name.
            let path = Arc::new(Path(vec![(
                names[0].as_bytes().to_vec(),
                Bytes::from(scene.files[0].1.clone()),
            )]));
            let [eager, lazy, _] = race_options();
            [eager, lazy]
                .into_iter()
                .map(|(what, mut o)| {
                    o.resolver = Some(path.clone());
                    (what, o)
                })
                .collect()
        } else {
            race_options().into()
        };
        for ((what, options), at) in options
            .into_iter()
            .flat_map(|o| [(o.clone(), Hook::Listing), (o, Hook::Published)])
        {
            cases += 1;
            let what = format!("{what}, stopped at {at:?}");
            let env = Environment::new(options);
            let gate = Gate::install(&env);
            let load = {
                let (env, source) = (env.clone(), src(0));
                gate.run(Some((at, 1)), move || {
                    if find {
                        let name = RawStr::from(names[0]);
                        let found = env.find_file(&name, true, &Meter::unlimited());
                        assert!(found.unwrap().is_some(), "find_file found nothing");
                    } else {
                        drop(env.load(source).unwrap());
                    }
                })
            };
            load.stopped();
            env.load(src(1)).unwrap();
            load.finish();
            let got = record(&env, &rows(&env));
            for part in ["files", "objects"] {
                assert_eq!(
                    got[part], want[part],
                    "{names:?}, find {find}, {what}: {part}"
                );
            }
            assert_eq!(got, want, "{names:?}, find {find}, {what}");
        }
    }
    assert_eq!(cases, (2 * 3 + 2) * 2);
}

/// Three loads of `listings_raced`'s files (`env_threads.json`'s run whose
/// names are `x`, `y` and `x`: `p` under `x` and `y`, then `q` under `x`):
/// the first is stopped before its `files` entry ([`Hook::Listing`]) or
/// right after its key is published ([`Hook::Published`]) while the other
/// two load whole, then the first lists. The third listed `x` first, at its
/// own number, so the first's listing moves `x` to the first's number and
/// leaves it the third's file: the container, every object's path and the
/// state are UnityPy's for the three made in the order of their numbers,
/// which differ from UnityPy's for the order they listed in, under every
/// option set (review round 16's follow-up: the race of two loads never
/// moved a key, so only a hand-written model checked the move).
#[cfg(test)]
#[test]
fn a_key_a_later_load_listed_first_moves_to_the_first_loads_number() {
    let doc = threads();
    let scene = unityrs_testkit::env::listings_raced();
    let names = json!(["x", "y", "x"]);
    let run = |order: [u64; 3]| -> &J {
        doc["orders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["names"] == names && c["order"] == json!(order))
            .unwrap_or_else(|| panic!("UnityPy has no run of {names} in {order:?}"))
    };
    let found = run([0, 1, 2]);
    let want = without(found, &["name", "picks", "names", "find", "order"]);
    assert_ne!(
        want,
        without(run([1, 2, 0]), &["name", "picks", "names", "find", "order"]),
        "UnityPy's orders agree"
    );
    assert_eq!(found["find"], json!(false));
    let picks: Vec<usize> = found["picks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| usize::try_from(p.as_u64().unwrap()).unwrap())
        .collect();
    let src = |i: usize| Source::Bytes {
        name: RawStr::from(names[i].as_str().unwrap()),
        data: Bytes::from(scene.files[picks[i]].1.clone()),
    };
    let mut cases = 0;
    for ((what, options), at) in race_options()
        .into_iter()
        .flat_map(|o| [(o.clone(), Hook::Listing), (o, Hook::Published)])
    {
        cases += 1;
        let what = format!("{what}, stopped at {at:?}");
        let env = Environment::new(options);
        let gate = Gate::install(&env);
        let load = {
            let (env, source) = (env.clone(), src(0));
            gate.run(Some((at, 1)), move || {
                drop(env.load(source).unwrap());
            })
        };
        load.stopped();
        env.load(src(1)).unwrap();
        env.load(src(2)).unwrap();
        load.finish();
        let got = record(&env, &rows(&env));
        for part in ["files", "objects"] {
            assert_eq!(got[part], want[part], "{what}: {part}");
        }
        assert_eq!(got, want, "{what}");
    }
    assert_eq!(cases, 3 * 2);
}
