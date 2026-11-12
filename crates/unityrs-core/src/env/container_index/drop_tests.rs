//! A container pass's charges are released after the memory each pays for
//! (plan section 3.15), which their fields' order alone keeps: a walk's
//! `Targets` drops `known`, `answers` and `next` before its walk, whose
//! watches end before `known`'s table's charge is released (review round
//! 5), and a bundle's `SiblingNames` drops its names before their table's
//! charge (fix pass 17's sweep: no test failed with either charge declared
//! first, or with the walk declared before `known`). The heap is counted
//! on the test's thread ([`crate::env::heap`]).

use std::collections::HashMap;
use std::sync::Arc;

use super::{Answer, NONE, Siblings, Targets};
use crate::env::heap::{self, Noted};
use crate::env::registry::open_bare;
use crate::env::watch::{FILES, REGISTRY};
use crate::env::{EnvOptions, Environment, Source};
use crate::files::File;
use crate::limits::{Charge, Meter, MeteredVec};
use crate::options::FileContext;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, bundle, serialized};

/// A walk's `Targets` of 64 externals, each kept with its flag and linked,
/// with the walk's charge and `known`'s table's on meters of their own:
/// when it drops, each of the two is released after every block the
/// `Targets` held is freed (`known` with its flags' handles, `answers`,
/// `next`, then the walk's entries and its own blocks).
#[test]
fn a_walks_targets_release_each_charge_after_what_it_paid_for() {
    heap::count();
    let env = Environment::new(EnvOptions::unitypy());
    let (walk_meter, walk_noted) = Noted::meter();
    let (known_meter, known_noted) = Noted::meter();
    let other = Meter::unlimited();
    let mut t = Targets {
        known: HashMap::new(),
        answers: MeteredVec::new(&other),
        next: None,
        walk: env
            .inner
            .registry
            .watches()
            .start(&walk_meter, [&env.inner.registry, &env.inner.files]),
        charge: Charge::zero(&known_meter),
    };
    for i in 0..64_usize {
        let h = t.walk.hash(&i.to_le_bytes());
        let flag = t.walk.watch(&[(REGISTRY, h), (FILES, h)]).unwrap();
        let id = flag.id();
        t.answers.push(Answer { head: NONE }).unwrap();
        t.keep(i32::try_from(i).unwrap(), None, true, Some(flag))
            .unwrap();
        t.link(i, id, 64, &other).unwrap();
    }
    let (walk_from, known_from) = (walk_noted.releases(), known_noted.releases());
    let before = heap::live();
    drop(t);
    let after = heap::live();
    assert!(before > after, "the drop freed nothing");
    heap::released_last(&walk_noted, walk_from, after, "the walk's charge");
    heap::released_last(&known_noted, known_from, after, "known's table's charge");
}

/// A bundle's sibling names, rule 4's index of its 32 entries, built by a
/// lookup, then dropped alone: the charge for their table, on the bundle's
/// meter, is released after the table and every name in it are freed.
#[test]
fn a_bundles_sibling_names_release_their_tables_charge_last() {
    heap::count();
    let ctx = Arc::new(FileContext::unitypy());
    let (meter, noted) = Noted::meter();
    let entries: Vec<(String, Vec<u8>)> = (0..32)
        .map(|i| {
            let file = serialized(&[], &[Obj::named(1, "o", &[])]);
            (format!("CAB-{i:02}"), file)
        })
        .collect();
    let named: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    let source = Source::Bytes {
        name: RawStr::from("b.bundle"),
        data: Bytes::from(bundle(&named)),
    };
    let File::Bundle(b) = open_bare(&RawStr::from("b"), &source, &meter, &ctx).unwrap() else {
        panic!("the testkit's bundle opens as a BundleFile");
    };
    let mut siblings = Siblings::default();
    assert_eq!(siblings.find(&b, &RawStr::from("cab-07")).unwrap(), Some(7));
    let (_, index) = siblings.bundles.drain().next().unwrap();
    drop(siblings);
    let from = noted.releases();
    let before = heap::live();
    drop(index);
    let after = heap::live();
    assert!(before > after, "the drop freed nothing");
    heap::released_last(&noted, from, after, "the names' table's charge");
    drop(b);
}
