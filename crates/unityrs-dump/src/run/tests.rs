//! [`super`]'s tests: the ordered pool, the writer and the output guard.

use super::*;

#[test]
fn gzip_header_is_the_oracles() {
    let mut gz = gzip(Vec::new());
    gz.write_all(b"{}\n").unwrap();
    let bytes = gz.finish().unwrap();
    assert_eq!(hex(bytes.get(..10).unwrap()), "1f8b08000000000002ff");
}

#[test]
fn ordered_keeps_order_at_any_worker_count() {
    let items: Vec<u64> = (0..50).collect();
    for jobs in [1, 2, 3, 8] {
        let mut seen = Vec::new();
        // Later items finish first: each sleeps less.
        let f = |x: &u64| {
            std::thread::sleep(std::time::Duration::from_micros(500 * (50 - x)));
            x * 2
        };
        ordered(&items, jobs, &f, &mut |i, r| {
            seen.push((i, r));
            Ok(())
        })
        .unwrap();
        let want: Vec<(usize, u64)> = (0..50).map(|i| (i, 2 * i as u64)).collect();
        assert_eq!(seen, want, "jobs {jobs}");
    }
}

/// The sleeps above only make a reordering likely; here it is certain. On
/// `jobs` workers, `jobs` items run at once, and each waits for the next to
/// finish, so they finish last to first, and the sink still sees them first
/// to last. The wait only turns a hang into a failure.
#[test]
fn ordered_hands_back_in_item_order_items_that_finish_last_to_first() {
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};
    for jobs in [2, 4] {
        let done = (Mutex::new(Vec::new()), Condvar::new());
        let f = |k: &usize| -> Option<usize> {
            let (finished, cv) = &done;
            let mut finished = finished.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(600);
            while *k + 1 < jobs && !finished.contains(&(*k + 1)) {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return None;
                }
                finished = cv.wait_timeout(finished, left).unwrap().0;
            }
            finished.push(*k);
            cv.notify_all();
            Some(*k)
        };
        let mut seen = Vec::new();
        ordered(&(0..jobs).collect::<Vec<_>>(), jobs, &f, &mut |i, r| {
            seen.push((i, r));
            Ok(())
        })
        .unwrap();
        let finished = done.0.into_inner().unwrap();
        assert_eq!(finished, (0..jobs).rev().collect::<Vec<_>>(), "-j {jobs}");
        let want: Vec<_> = (0..jobs).map(|i| (i, Some(i))).collect();
        assert_eq!(seen, want, "-j {jobs}");
    }
}

/// `-j N` runs N inputs at once and no more, whatever the scheduling:
/// every item runs on one of the pool's workers (its index is below the
/// pool's size), and the pool holds `jobs` workers, or one per item when
/// there are fewer, however many items the window of `2 * jobs` queues.
/// And `jobs` items that wait for each other all start: a rendezvous,
/// whose wait only turns a hang into a failure.
#[test]
fn ordered_runs_jobs_items_at_once_and_no_more() {
    use std::collections::BTreeSet;
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};
    for (jobs, items) in [(1, 5), (3, 2), (3, 12), (4, 4), (4, 40), (8, 3)] {
        let workers = Mutex::new(BTreeSet::new());
        let f = |_: &usize| {
            let worker = (rayon::current_num_threads(), rayon::current_thread_index());
            workers.lock().unwrap().insert(worker);
        };
        let all: Vec<usize> = (0..items).collect();
        ordered(&all, jobs, &f, &mut |_, ()| Ok(())).unwrap();
        let size = jobs.min(items);
        for (pool, index) in workers.into_inner().unwrap() {
            assert_eq!(pool, size, "-j {jobs}, {items} items: a pool of {pool}");
            assert!(
                index.is_some_and(|i| i < size),
                "-j {jobs}, {items} items: an item ran on {index:?}, not one of {size} workers"
            );
        }
    }
    let jobs = 4;
    let started = (Mutex::new(0usize), Condvar::new());
    let f = |_: &usize| -> bool {
        let (count, cv) = &started;
        let mut n = count.lock().unwrap();
        *n += 1;
        cv.notify_all();
        let deadline = Instant::now() + Duration::from_secs(600);
        while *n < jobs {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            n = cv.wait_timeout(n, left).unwrap().0;
        }
        true
    };
    let mut met = Vec::new();
    ordered(&(0..jobs).collect::<Vec<_>>(), jobs, &f, &mut |_, ok| {
        met.push(ok);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        met, [true; 4],
        "{jobs} workers never ran {jobs} items at once"
    );
}

#[test]
fn ordered_stops_on_a_panic_and_names_the_input() {
    let items: Vec<u32> = (0..10).collect();
    for jobs in [1, 4] {
        let f = |x: &u32| {
            assert!(*x != 6, "boom at six");
            *x
        };
        let mut n = 0;
        let err = ordered(&items, jobs, &f, &mut |_, _| {
            n += 1;
            Ok(())
        })
        .unwrap_err();
        assert!(err.to_string().contains("input 7 of 10"), "{err}");
        assert!(err.to_string().contains("boom at six"), "{err}");
        assert!(n <= 6, "jobs {jobs}: {n} written past the panic");
    }
}

#[test]
fn ordered_stops_when_the_sink_refuses() {
    let items: Vec<u32> = (0..20).collect();
    let f = |x: &u32| *x;
    let mut got = Vec::new();
    let err = ordered(&items, 4, &f, &mut |i, r| {
        if i == 3 {
            return Err(RunError::Failed("stop".into()));
        }
        got.push(r);
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err, RunError::Failed("stop".into()));
    assert_eq!(got, [0, 1, 2]);
}

#[test]
fn the_repository_is_refused_and_a_temporary_folder_is_not() {
    let repo = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    assert!(outside_repo(&repo.join("out.jsonl")).is_err());
    assert!(outside_repo(&repo.join("no/such/dir/out.jsonl")).is_err());
    assert!(outside_repo(&std::env::temp_dir().join("unityrs-dump-out.jsonl")).is_ok());
}
