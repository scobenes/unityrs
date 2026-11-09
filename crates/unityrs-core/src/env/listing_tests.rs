//! `files` is a dict by rank (`listing.rs`'s `Files`; review round 16):
//! whatever order loads list their entries in, from whichever thread, it
//! holds what UnityPy's `env.files` holds when the same loads are made one
//! after the other in the order of their numbers, which the registry's
//! keys follow too (plan section 3.16, R1).

use std::sync::Arc;

use super::registry::open_source;
use super::{EnvOptions, Environment, Source};
use crate::files::File;
use crate::limits::Meter;
use crate::options::FileContext;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, serialized};

/// The listings: each load's number and the key it lists. Two loads list
/// `x` and two `y`, and `X` lowers to `x`, so a key is listed again by a
/// later load and by an earlier one, and two keys lower to one name.
const LISTINGS: [(u64, &str); 5] = [(0, "x"), (1, "y"), (2, "X"), (3, "x"), (4, "y")];

/// UnityPy's `env.files` after the listings of `loads`, made in the order
/// of their numbers (a Python dict: a key at its first assignment, with
/// its last's value): each key and the load whose file it holds.
fn dict(loads: &[u64]) -> Vec<(&'static str, u64)> {
    let mut sorted = loads.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<(&'static str, u64)> = Vec::new();
    for load in sorted {
        let key = LISTINGS.iter().find(|l| l.0 == load).unwrap().1;
        match out.iter_mut().find(|e| e.0 == key) {
            Some(e) => e.1 = load,
            None => out.push((key, load)),
        }
    }
    out
}

/// Every order of `0..n`, by Heap's algorithm.
fn orders(n: usize) -> Vec<Vec<usize>> {
    fn heap(k: usize, a: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if k <= 1 {
            out.push(a.clone());
            return;
        }
        for i in 0..k - 1 {
            heap(k - 1, a, out);
            let j = if k.is_multiple_of(2) { i } else { 0 };
            a.swap(j, k - 1);
        }
        heap(k - 1, a, out);
    }
    let mut out = Vec::new();
    heap(n, &mut (0..n).collect(), &mut out);
    out
}

/// Every order the five listings can arrive in, each checked after every
/// listing against UnityPy's dict of the listings made so far: `files`
/// lists each key at its place with its file, `objects` steps into the
/// files in that order (each holds one object, of path id its load + 1),
/// and rule 4's lookup by a lowered name answers the first key that lowers
/// to it (`classes/PPtr.py:72-75`). Each listing comes bounded, so a cursor
/// that does not advance fails instead of looping.
#[test]
fn files_holds_unitypys_dict_whatever_order_the_loads_list_in() {
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::unlimited();
    let files: Vec<File> = LISTINGS
        .iter()
        .map(|&(load, _)| {
            let id = i64::try_from(load).unwrap() + 1;
            let source = Source::Bytes {
                name: RawStr::from(format!("f{load}").as_str()),
                data: Bytes::from(serialized(&[], &[Obj::named(id, "o", &[])])),
            };
            open_source(&RawStr::from("f"), &source, &meter, &ctx).unwrap()
        })
        .collect();
    let load_of = |f: &File| -> u64 {
        let File::Serialized(sf) = f else {
            panic!("a listed file is a SerializedFile");
        };
        let at = files
            .iter()
            .position(|g| matches!(g, File::Serialized(g) if Arc::ptr_eq(g, sf)))
            .expect("a listed file");
        LISTINGS[at].0
    };
    let all = orders(LISTINGS.len());
    assert_eq!(all.len(), 120);
    for order in &all {
        let env = Environment::new(EnvOptions::unitypy());
        let mut loads = Vec::new();
        for &i in order {
            let (load, key) = LISTINGS[i];
            env.list(RawStr::from(key), files[i].clone(), false, load, &meter)
                .unwrap();
            loads.push(load);
            let want = dict(&loads);
            let what = format!("order {order:?}, after {loads:?}");
            let got: Vec<(String, u64)> = env
                .files()
                .take(want.len() + 1)
                .map(|e| {
                    (
                        String::from_utf8_lossy(e.name.as_bytes()).into_owned(),
                        load_of(&e.file),
                    )
                })
                .collect();
            let want_files: Vec<(String, u64)> =
                want.iter().map(|&(k, l)| (k.to_owned(), l)).collect();
            assert_eq!(got, want_files, "{what}: files");
            let ids: Vec<i64> = env
                .objects()
                .take(want.len() + 1)
                .map(|o| o.unwrap().path_id())
                .collect();
            let want_ids: Vec<i64> = want
                .iter()
                .map(|&(_, l)| i64::try_from(l).unwrap() + 1)
                .collect();
            assert_eq!(ids, want_ids, "{what}: objects");
            for name in ["x", "y"] {
                let want_match = want
                    .iter()
                    .find(|(k, _)| k.to_lowercase() == name)
                    .map(|&(_, l)| l);
                let got_match = env.files_match(&RawStr::from(name)).map(|f| load_of(&f));
                assert_eq!(got_match, want_match, "{what}: rule 4's {name}");
            }
        }
    }
}
