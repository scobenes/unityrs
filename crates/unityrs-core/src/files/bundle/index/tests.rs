//! The two tables against brute-force models, over random block layouts and
//! random orders of reads and publishes.

use std::sync::Arc;

use bytes::Bytes;
use unityrs_testkit::Rng;

use super::{Ends, Published, RUN_LEAF, RUN_SHARE, RunIndex, search_steps};
use crate::budget::{Reservation, Unbounded};
use crate::limits::{Meter, MeteredVec};

#[test]
fn a_search_counts_the_probes_a_binary_search_makes_at_most() {
    let want = [
        (0, 0),
        (1, 1),
        (2, 2),
        (3, 3),
        (4, 3),
        (5, 4),
        (8, 4),
        (9, 5),
        (40_000, 17),
    ];
    for (n, steps) in want {
        assert_eq!(search_steps(n), steps, "{n} rows");
    }
}

/// Every search of the known prefix finds the block a scan of the prefix
/// sums finds: random lengths, zeros among them, learned in order, searched
/// at every length of the prefix from every start, for offsets on, between
/// and past the blocks' edges.
#[test]
fn the_prefix_finds_the_blocks_a_scan_finds() {
    let mut rng = Rng::new(11);
    for _ in 0..200 {
        let n = rng.below(40);
        let lens: Vec<u64> = (0..n)
            .map(|_| match rng.below(4) {
                0 => 0,
                k => u64::try_from(rng.below(5 * k)).unwrap(),
            })
            .collect();
        let ends: Vec<u64> = lens
            .iter()
            .scan(0, |acc, l| {
                *acc += l;
                Some(*acc)
            })
            .collect();
        let total = ends.last().copied().unwrap_or(0);
        let (table, _charge) = Ends::new(n, &Meter::unlimited()).unwrap();
        for known in 0..=n {
            assert_eq!(table.known(), known);
            for k in 0..=known {
                let start = k.checked_sub(1).map_or(0, |j| ends[j]);
                assert_eq!(table.start(k), start, "start of {k}");
            }
            for from in 0..=known {
                let most = search_steps(known - from);
                for at in 0..=total + 2 {
                    let past = (from..known).find(|&k| ends[k] > at).unwrap_or(known);
                    let reach = (from..known).find(|&k| ends[k] >= at).unwrap_or(known);
                    let (got, probes) = table.first_ending_past(from, known, at);
                    assert_eq!(got, past, "{lens:?}");
                    assert!(probes <= most, "{probes} probes of {} rows", known - from);
                    let (got, probes) = table.first_ending_at(from, known, at);
                    assert_eq!(got, reach, "{lens:?}");
                    assert!(probes <= most, "{probes} probes of {} rows", known - from);
                }
            }
            if known < n {
                // Out of order, a block is not learned.
                table.learn(known + 1, 7);
                assert_eq!(table.known(), known);
                table.learn(known, lens[known]);
                // Learned again, it holds what it held.
                table.learn(known, lens[known]);
            }
        }
    }
}

/// The span a test run of blocks `first..end` stands for, read back.
fn span_of(bytes: &Bytes) -> (usize, usize) {
    (usize::from(bytes[0]), usize::from(bytes[1]))
}

fn run(first: usize, end: usize) -> Bytes {
    Bytes::from(vec![
        u8::try_from(first).unwrap(),
        u8::try_from(end).unwrap(),
    ])
}

/// Blocks `f..e` cover `first..end`.
fn covers((f, e): (usize, usize), (first, end): (usize, usize)) -> bool {
    f <= first && end <= e
}

/// The run index against a model that keeps every run ever published: a
/// span is covered by a cached run exactly when some published run covered
/// it, the run a search finds covers it, a publish is answered by a covering
/// run exactly when one exists and otherwise drops exactly the runs it
/// covers, no cached run contains another, and the charge is the root leaf
/// and one share per run.
#[test]
fn the_run_index_covers_what_every_published_run_covered() {
    let mut rng = Rng::new(5);
    let meter = Meter::unlimited();
    for _ in 0..300 {
        let blocks = 1 + rng.below(30);
        let mut index = RunIndex::new(&meter);
        let mut model: Vec<(usize, usize)> = Vec::new();
        for _ in 0..rng.below(80) {
            let first = rng.below(blocks);
            let end = first + 1 + rng.below(blocks - first);
            let span = (first, end);
            let covering = model.iter().copied().find(|r| covers(*r, span));
            let found = index.covering(first, end).map(|(f, b)| (f, span_of(&b)));
            assert_eq!(found.is_some(), covering.is_some(), "{span:?} in {model:?}");
            if let Some((f, r)) = found {
                assert_eq!(f, r.0);
                assert!(covers(r, span), "{r:?} for {span:?}");
            }
            if rng.below(2) == 0 {
                continue;
            }
            let cached: Vec<(usize, usize)> = index.runs.iter().map(|(f, r)| (*f, r.end)).collect();
            let inside: Vec<(usize, usize)> = cached
                .iter()
                .copied()
                .filter(|r| covers(span, *r))
                .collect();
            assert_eq!(index.covered(first, end), inside.len());
            let mut dropped = MeteredVec::new(&meter);
            match index.publish(first, end, &run(first, end), &mut dropped) {
                Published::Covered(f, b) => {
                    assert!(covering.is_some(), "{span:?} answered by a run");
                    assert!(covers(span_of(&b), span) && f == span_of(&b).0);
                    assert!(dropped.is_empty());
                }
                Published::Cached => {
                    assert!(covering.is_none(), "{span:?} cached though covered");
                    let gone: Vec<(usize, usize)> =
                        dropped.as_slice().iter().map(span_of).collect();
                    assert_eq!(gone, inside, "the runs {span:?} covers");
                    model.push(span);
                }
                Published::Refused => panic!("an unlimited meter refused"),
            }
            let now: Vec<(usize, usize)> = index.runs.iter().map(|(f, r)| (*f, r.end)).collect();
            for pair in now.windows(2) {
                assert!(pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1, "{now:?}");
            }
            for (f, r) in &index.runs {
                assert_eq!(span_of(&r.bytes), (*f, r.end));
            }
            let len = u64::try_from(index.len()).unwrap();
            assert_eq!(index.charge.bytes(), RUN_LEAF + len * RUN_SHARE);
        }
    }
}

/// A refused growth of the index, or of the list of the runs a publish
/// would drop, publishes nothing and drops nothing.
#[test]
fn a_refused_publish_changes_nothing() {
    let full = Meter::new(0, Arc::new(Unbounded), Reservation::Input);
    let mut index = RunIndex::new(&full);
    let mut dropped = MeteredVec::new(&full);
    assert!(matches!(
        index.publish(0, 5, &run(0, 5), &mut dropped),
        Published::Refused
    ));
    assert_eq!((index.len(), index.charge.bytes()), (0, 0));

    let unlimited = Meter::unlimited();
    let mut index = RunIndex::new(&unlimited);
    let mut dropped = MeteredVec::new(&unlimited);
    assert!(matches!(
        index.publish(1, 2, &run(1, 2), &mut dropped),
        Published::Cached
    ));
    let charged = index.charge.bytes();
    let mut no_room = MeteredVec::new(&full);
    assert!(matches!(
        index.publish(0, 5, &run(0, 5), &mut no_room),
        Published::Refused
    ));
    assert_eq!(
        index.covering(1, 2).map(|(f, b)| (f, span_of(&b))),
        Some((1, (1, 2)))
    );
    assert_eq!((index.len(), index.charge.bytes()), (1, charged));
}
