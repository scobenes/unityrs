//! What the module docs' "Threads" paragraph says holds, pinned with the
//! Environment's hooks ([`super::gate`]): a call stopped midway (a load or
//! a reach after its first key, a container pass at its first lookup) shows
//! another thread each key, each `files` entry and each `path_dict` entry
//! either absent or whole, as it is once the call returns; and once the
//! call returns, all of them are there (review round 6: the paragraph said
//! concurrent calls act as if made one after another, which a load or a
//! reach seen midway does not, since its keys are published one at a time).

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread carries
// one.
use std::sync::Arc;

#[cfg(test)]
use super::gate::Gate;
use super::registry::Hook;
use super::{Cab, EnvOptions, Environment, LoadMode, RawCab, Source};
use crate::container::ContainerPath;
use crate::limits::Meter;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, bundle, preload_paths, res_bytes, serialized};

fn src(name: &str, data: &Bytes) -> Source {
    Source::Bytes {
        name: RawStr::from(name),
        data: data.clone(),
    }
}

fn cab(name: &str) -> Vec<u8> {
    serialized(&[], &[Obj::named(1, name, &[])])
}

/// What `name` finds, whole: a CAB's name and its objects' path ids and
/// bytes, a raw entry's name and bytes; `None` for nothing.
fn answer(env: &Environment, name: &str) -> Option<String> {
    let meter = Meter::unlimited();
    Some(match env.get_cab(&RawStr::from(name), &meter).unwrap()? {
        Cab::Serialized(sf) => {
            let objects: Vec<String> = sf
                .objects()
                .map(|o| format!("{}:{:?}", o.path_id(), o.raw_data()))
                .collect();
            format!("{} {objects:?}", sf.name())
        }
        Cab::Raw(RawCab::InBundle { bundle, dir_index }) => {
            let len = u64::try_from(bundle.directory()[dir_index].size).unwrap();
            let bytes = bundle.entry_range(dir_index, 0..len).unwrap();
            format!("raw {} {bytes:?}", bundle.directory()[dir_index].path)
        }
        Cab::Raw(RawCab::File(f)) => format!("raw file {}", f.name),
    })
}

/// The `files` keys, in order.
fn files(env: &Environment) -> Vec<String> {
    env.files()
        .map(|e| String::from_utf8_lossy(e.name.as_bytes()).into_owned())
        .collect()
}

/// Each of `names` answers as it does once the call returned, or not at
/// all: never half made.
fn absent_or_whole(env: &Environment, done: &[Option<String>], names: &[&str]) -> Vec<bool> {
    names
        .iter()
        .zip(done)
        .map(|(n, want)| {
            let got = answer(env, n);
            assert!(got.is_none() || got == *want, "{n} half made: {got:?}");
            got.is_some()
        })
        .collect()
}

/// A lazy reach of a nested container (`get_cab("INNER")`) stopped after it
/// registered its first entry, and an eager load stopped after its first
/// key: each key another thread looks up then is absent or whole, `files`
/// lists the load whole or not at all, and once the call returns every key
/// of it, and its `files` entry, is there.
#[test]
fn a_call_seen_midway_shows_each_key_whole_and_all_of_them_once_it_returns() {
    let inner = bundle(&[("CAB-I", cab("i")), ("CAB-I.resS", res_bytes(8, 1))]);
    let o = Bytes::from(bundle(&[("CAB-O", cab("o")), ("INNER", inner)]));
    let a = Bytes::from(bundle(&[("CAB-A", cab("a")), ("CAB-B", cab("b"))]));
    for mode in [LoadMode::Lazy, LoadMode::Eager] {
        let mut options = EnvOptions::unitypy();
        options.load = mode;
        // What the keys answer once every call returned (under `Lazy`,
        // `INNER` reached).
        let whole = Environment::new(options.clone());
        whole.load(src("o.bundle", &o)).unwrap();
        whole.load(src("a.bundle", &a)).unwrap();
        let meter = Meter::unlimited();
        drop(whole.get_cab(&RawStr::from("INNER"), &meter).unwrap());
        let names = ["CAB-I", "CAB-I.resS", "CAB-A", "CAB-B"];
        let done: Vec<_> = names.iter().map(|n| answer(&whole, n)).collect();
        assert!(done.iter().all(Option::is_some), "{mode:?}: {done:?}");

        let env = Environment::new(options);
        env.load(src("o.bundle", &o)).unwrap();
        let gate = Gate::install(&env);
        if mode == LoadMode::Lazy {
            let e = env.clone();
            let reach = gate.run(Some((Hook::Published, 1)), move || {
                drop(
                    e.get_cab(&RawStr::from("INNER"), &Meter::unlimited())
                        .unwrap(),
                );
            });
            reach.stopped();
            let seen = absent_or_whole(&env, &done[..2], &names[..2]);
            assert!(seen[0], "the reach stopped after CAB-I");
            reach.finish();
            assert_eq!(absent_or_whole(&env, &done[..2], &names[..2]), [true; 2]);
        }
        let e = env.clone();
        let data = a.clone();
        let load = gate.run(Some((Hook::Published, 1)), move || {
            drop(e.load(src("a.bundle", &data)).unwrap());
        });
        load.stopped();
        let seen = absent_or_whole(&env, &done[2..], &names[2..]);
        assert!(seen[0], "{mode:?}: the load stopped after CAB-A");
        let listed = files(&env);
        assert!(
            listed == ["o.bundle"] || listed == ["o.bundle", "a.bundle"],
            "{mode:?}: {listed:?}"
        );
        load.finish();
        assert_eq!(absent_or_whole(&env, &done[2..], &names[2..]), [true; 2]);
        assert_eq!(files(&env), ["o.bundle", "a.bundle"], "{mode:?}");
    }
}

/// Every object's `path_dict` entry, read without a pass: its path, or
/// `None`.
fn paths(env: &Environment) -> Vec<Option<String>> {
    let meter = Meter::unlimited();
    let mut out = Vec::new();
    for name in ["CAB-PP", "CAB-PQ"] {
        let Some(Cab::Serialized(sf)) = env.get_cab(&RawStr::from(name), &meter).unwrap() else {
            panic!("{name}");
        };
        for o in sf.objects() {
            let path = match sf
                .container()
                .unwrap()
                .path_of(o.path_id(), &meter)
                .unwrap()
            {
                Some(ContainerPath::Str(s)) => Some(String::from_utf8_lossy(s.as_bytes()).into()),
                Some(ContainerPath::NotAString) => Some("<not a str>".into()),
                None => None,
            };
            out.push(path);
        }
    }
    out
}

/// A container pass stopped at its first lookup of an external (scene
/// `preload_paths`): each `path_dict` entry another thread reads then is
/// absent or the one the pass leaves, and once the pass returns all are
/// there.
#[test]
fn a_pass_seen_midway_shows_each_path_whole_and_all_of_them_once_it_returns() {
    let data = Bytes::from(preload_paths().files[0].1.clone());
    for mode in [LoadMode::Lazy, LoadMode::Eager] {
        let mut options = EnvOptions::unitypy();
        options.load = mode;
        let whole = Environment::new(options.clone());
        whole.load(src("pp.bundle", &data)).unwrap();
        let _ = whole.container().unwrap().count();
        let done = paths(&whole);
        assert!(done.iter().any(Option::is_some), "{mode:?}: {done:?}");

        let env = Arc::new(Environment::new(options));
        env.load(src("pp.bundle", &data)).unwrap();
        let gate = Gate::install(&env);
        let e = env.clone();
        let pass = gate.run(Some((Hook::Watched, 1)), move || {
            let _ = e.container().unwrap().count();
        });
        pass.stopped();
        let midway = paths(&env);
        for (got, want) in midway.iter().zip(&done) {
            assert!(got.is_none() || got == want, "{mode:?}: {midway:?}");
        }
        assert_ne!(midway, done, "{mode:?}: the pass stopped before its end");
        pass.finish();
        assert_eq!(paths(&env), done, "{mode:?}");
    }
}
