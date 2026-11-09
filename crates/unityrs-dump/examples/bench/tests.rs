//! The bench's own pieces: the tracked budget, the percentiles, the
//! arguments, and passes over the testkit's format matrix written to a
//! scratch folder with an index naming each file a bundle.

use std::path::PathBuf;
use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::limits::Meter;
use unityrs_dump::select::collect;
use unityrs_testkit::scratch::Scratch;

use crate::budget::{BudgetKind, Tracker};
use crate::summary::{at, spread};
use crate::work::Load;
use crate::{Args, run};

fn args(list: &[&str]) -> Result<Args, String> {
    Args::parse(&list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
}

/// Each input's tracker counts what its meter holds now and at most, and
/// adds to the process's, whose peak is of the sum: input A holds 100,
/// then 30; B holds 50, then 250 with A's 30 alive, so the process peaks
/// at 280 though A peaked at 100 and B at 250.
#[test]
fn a_tracker_keeps_each_inputs_peak_and_the_peak_of_their_sum() {
    let process = Arc::new(Tracker::default());
    let (a, b) = (Tracker::child(&process), Tracker::child(&process));
    let ma = Meter::new(u64::MAX, a.budget(), Reservation::Input);
    let mb = Meter::new(u64::MAX, b.budget(), Reservation::Input);
    let a1 = ma.charge(100).unwrap();
    let b1 = mb.charge(50).unwrap();
    assert_eq!((a.used(), b.used(), process.used()), (100, 50, 150));
    drop(a1);
    let a2 = ma.charge(30).unwrap();
    let mut b2 = mb.charge(150).unwrap();
    b2.grow(50).unwrap();
    assert_eq!((a.used(), b.used(), process.used()), (30, 250, 280));
    assert_eq!((a.peak(), b.peak(), process.peak()), (100, 250, 280));
    drop((a2, b1, b2));
    assert_eq!((a.used(), b.used(), process.used()), (0, 0, 0));
    assert_eq!((ma.used(), mb.used()), (0, 0));
    assert_eq!(process.peak(), 280);
}

#[test]
fn percentiles_take_the_lower_value() {
    let xs: Vec<u64> = (1..=100).collect();
    assert_eq!((at(&xs, 1.0), at(&xs, 0.99), at(&xs, 0.5)), (100, 99, 50));
    assert_eq!(at(&[], 0.5), 0);
    assert_eq!(spread(vec![3, 1, 2])["max"], 3);
}

#[test]
fn arguments() {
    let a = args(&["i.json"]).unwrap();
    assert_eq!(
        (a.jobs, a.budget, a.load, a.pin),
        (1, BudgetKind::Unbounded, Load::Eager, false)
    );
    let a = args(&[
        "-j", "8", "i.json", "--budget", "tracked", "--load", "lazy", "--kind", "bundle",
    ])
    .unwrap();
    assert_eq!(
        (a.jobs, a.budget, a.load),
        (8, BudgetKind::Tracked, Load::Lazy)
    );
    assert_eq!(a.selection().kinds.len(), 1);
    assert_eq!(
        args(&["i.json", "--sample", "20"])
            .unwrap()
            .selection()
            .sample,
        Some(20)
    );
    for bad in [
        &["-j", "0", "i"][..],
        &["i", "j"],
        &[],
        &["i", "--budget", "x"],
        &["i", "--frob"],
    ] {
        assert!(args(bad).is_err(), "{bad:?}");
    }
}

/// The matrix's files, written beside an index that names each a bundle,
/// and the paths of the dump's selection of them.
fn matrix_index(dir: &Scratch) -> (PathBuf, Vec<PathBuf>) {
    let mut files = Vec::new();
    for (name, bytes) in unityrs_testkit::bundle::matrix() {
        std::fs::write(dir.join(&name), bytes).unwrap();
        files.push(serde_json::json!({"path": name, "kind": "bundle"}));
    }
    let index = dir.join("index.json");
    std::fs::write(&index, serde_json::json!({ "files": files }).to_string()).unwrap();
    let paths = collect(&args(&[index.to_str().unwrap()]).unwrap().selection())
        .unwrap()
        .into_iter()
        .map(|i| i.path)
        .collect();
    (index, paths)
}

/// An eager pass decodes every block once into one run per bundle, so it
/// discards and decodes again nothing; a lazy pass reads the same objects;
/// every input's charges are returned; each input's peak is at least its
/// own bytes (read into a charged buffer), and the process's peak of live
/// charges at least the largest input's on one thread; every bundle's
/// engine version is counted once.
#[test]
fn passes_over_the_format_matrix() {
    let dir = Scratch::new("bench-matrix");
    let (index, paths) = matrix_index(&dir);
    let index = index.to_str().unwrap();
    let eager = run(
        &args(&[index, "--budget", "tracked", "--pin"]).unwrap(),
        &paths,
    );
    let lazy = run(
        &args(&[index, "--load", "lazy", "-j", "3"]).unwrap(),
        &paths,
    );
    assert!(eager.done.len() > 50, "{}", eager.done.len());
    let mut loaded = 0;
    for (i, (e, l)) in eager.done.iter().zip(&lazy.done).enumerate() {
        assert_eq!((e.leaked, l.leaked), (0, 0), "input {i}");
        if e.file_error {
            continue;
        }
        loaded += 1;
        assert_eq!((e.sums.discarded, e.sums.redecoded), (0, 0), "input {i}");
        assert_eq!(e.objects, l.objects, "input {i}");
        assert!(eager.meter_peaks[i].unwrap() >= e.bytes, "input {i}");
        let bundles = e
            .versions
            .iter()
            .filter(|((c, s), _)| *c == "bundle" && s != "big");
        assert_eq!(
            bundles.map(|(_, n)| n).sum::<u64>(),
            e.sums.bundles,
            "input {i}"
        );
        assert!(e.pin.is_some(), "input {i}");
    }
    assert!(loaded > 40, "{loaded}");
    let largest = eager.meter_peaks.iter().map(|p| p.unwrap()).max().unwrap();
    assert!(eager.process.peak() >= largest);
    assert!(lazy.meter_peaks.iter().all(Option::is_none), "untracked");
    let objects: u64 = eager.done.iter().map(|d| d.objects).sum();
    assert!(objects > 0);
}
