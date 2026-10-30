//! `Lazy` against `Eager`, one lookup at a time, on every testkit scene
//! (plan 3.13, difference (b)'s invariant): each lookup runs alone, right
//! after the scene's loads, in a fresh Environment of each mode, so no
//! earlier step has reached a nested container for it.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::File;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{Scene, scenes};

use super::run::{Run, text};

/// The scenes whose entries or blocks fail (differences (a) and (c)).
const FAILING: &[&str] = &["bad_cab", "asset_bundle_modes", "last_block_fails"];

/// What a scene's files hold, read under `Eager`: every name a lookup by
/// name could ask (each directory entry's path, at every depth, and each
/// external's path), the paths of the entries that are themselves bundles
/// below a top-level file (whose own name is a key under `Lazy` alone), and
/// the paths of the entries inside those (which a lookup by name alone may
/// miss under `Lazy` until something reaches their container).
#[derive(Default)]
struct Names {
    all: Vec<String>,
    containers: BTreeSet<String>,
    nested: BTreeSet<String>,
}

impl Names {
    fn of(scene: &Scene) -> Self {
        let env = Environment::new(EnvOptions::unitypy());
        let mut names = Self::default();
        for (name, data) in scene.files.iter().chain(&scene.resolver) {
            let source = Source::Bytes {
                name: RawStr::new(name.clone()),
                data: Bytes::from(data.clone()),
            };
            if let Ok(f) = env.load(source) {
                names.walk(&f, 0);
            }
        }
        names
    }

    fn walk(&mut self, file: &File, depth: usize) {
        match file {
            File::Serialized(sf) => {
                for e in sf.externals() {
                    self.add(text(e.path.as_bytes()));
                }
            }
            File::Bundle(b) => {
                for (i, d) in b.directory().iter().enumerate() {
                    let name = text(d.path.as_bytes());
                    if depth > 0 {
                        self.nested.insert(name.clone());
                    }
                    let Ok(f) = b.entry(i) else { continue };
                    if matches!(f, File::Bundle(_)) {
                        self.containers.insert(name.clone());
                    }
                    self.add(name);
                    self.walk(&f, depth + 1);
                }
            }
            File::Raw(_) | File::Web(_) => {}
        }
    }

    fn add(&mut self, name: String) {
        if !self.all.contains(&name) {
            self.all.push(name);
        }
    }

    /// `name`'s registry key (`simplify_name`, which lower-cases a CAB's).
    fn key(name: &str) -> String {
        name.rsplit(['/', '\\'])
            .next()
            .unwrap_or(name)
            .to_lowercase()
    }
}

/// The keys of a step's state without the containers' own names.
fn keys_without(step: &J, containers: &BTreeSet<String>) -> Vec<String> {
    let skip: BTreeSet<String> = containers.iter().map(|c| Names::key(c)).collect();
    step["cabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap().to_owned())
        .filter(|k| !skip.contains(k))
        .collect()
}

/// Under `Lazy`, every lookup that starts from a file (a dereference, a
/// resource read), the container pass (`container`, `paths`, `dep_paths`)
/// and every read answer as under `Eager`, under `EnvOptions::unitypy()`
/// and `batch()`, on every scene whose entries and blocks all parse, each
/// lookup the first after the loads; `files` and `objects` are `Eager`'s at
/// every step. What (b) leaves: the keys are `Eager`'s once the nested
/// containers' own names are out of both, but for keys of entries inside
/// a nested container nothing has reached, and a `get_cab` of such an
/// entry's name may miss (review round 4: a read from inside a nested
/// container, and a container a later load's entry dropped unread, gave
/// none of what `Eager` gives).
#[test]
fn lazy_answers_every_lookup_as_eager_does() {
    let batch = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    for (name, scene) in scenes() {
        if FAILING.contains(&name) {
            continue;
        }
        let names = Names::of(&scene);
        let loads: Vec<J> = (0..scene.files.len()).map(|i| json!(["load", i])).collect();
        let lookups = [
            json!(["reads"]),
            json!(["derefs"]),
            json!(["resources"]),
            json!(["container"]),
            json!(["paths"]),
            json!(["dep_paths"]),
            json!(["get_cab", names.all]),
        ];
        for options in [EnvOptions::unitypy(), batch.clone()] {
            for lookup in &lookups {
                let mut ops = loads.clone();
                ops.push(lookup.clone());
                let eager = Run::with(scene.clone(), LoadMode::Eager, options.clone()).steps(&ops);
                let lazy = Run::with(scene.clone(), LoadMode::Lazy, options.clone()).steps(&ops);
                for (e, l) in eager.iter().zip(&lazy) {
                    let op = &e["op"];
                    for part in ["files", "objects"] {
                        assert_eq!(l[part], e[part], "{name} {op} {part}");
                    }
                    if op[0] == "get_cab" {
                        check_get_cab(name, &names, &e["result"], &l["result"]);
                    } else {
                        assert_eq!(l["result"], e["result"], "{name} {op}");
                    }
                    check_keys(name, op, &names, e, l);
                }
            }
        }
    }
}

/// `get_cab` of every name: `Eager`'s answer, or none for an entry inside
/// a nested container or for a nested container's own name.
fn check_get_cab(scene: &str, names: &Names, eager: &J, lazy: &J) {
    let pairs = eager
        .as_array()
        .unwrap()
        .iter()
        .zip(lazy.as_array().unwrap());
    for (name, (e, l)) in names.all.iter().zip(pairs) {
        let key = Names::key(name);
        let hole = l.is_null() && names.nested.iter().any(|n| Names::key(n) == key);
        // (b): a container's own name answers `None` under `Lazy`.
        let container = l.is_null() && names.containers.iter().any(|n| Names::key(n) == key);
        assert!(
            l == e || hole || container,
            "{scene} get_cab({name}): lazy {l}, eager {e}"
        );
    }
}

/// `Lazy`'s keys, the containers' own names out, are `Eager`'s in order,
/// but for keys of nested entries not yet registered.
fn check_keys(scene: &str, op: &J, names: &Names, eager: &J, lazy: &J) {
    let e = keys_without(eager, &names.containers);
    let l = keys_without(lazy, &names.containers);
    let nested: BTreeSet<String> = names.nested.iter().map(|n| Names::key(n)).collect();
    let mut theirs = e.iter();
    for k in &l {
        let found = theirs.by_ref().find(|t| *t == k || !nested.contains(*t));
        assert!(
            found.is_some_and(|t| t == k),
            "{scene} {op}: keys {l:?} of {e:?}"
        );
    }
    assert!(
        theirs.all(|t| nested.contains(t)),
        "{scene} {op}: keys {l:?} of {e:?}"
    );
}
