use std::sync::Arc;

use super::{Counting, DecodeBuf, MemoryBudget, OWNER_COST, Permit, Reservation, Unbounded};
use crate::error::LimitKind;
use crate::limits::{ChargeError, Meter, heap_cost};

fn meter(limit: u64) -> Arc<Meter> {
    Meter::new(limit, Arc::new(Unbounded), Reservation::Input)
}

#[test]
fn a_permit_lives_as_long_as_its_last_slice() {
    let budget = Arc::new(Counting::new(1000));
    let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    let mut out = DecodeBuf::new(&m, 60, 1 << 20, LimitKind::BlockOutput).unwrap();
    out.extend_from_slice(&[7; 60]).unwrap();
    assert_eq!(
        budget.used(),
        heap_cost(60),
        "the meter booked it with the budget"
    );
    let whole = out.into_bytes().unwrap();
    let slice = whole.slice(10..20);
    drop(whole);
    let held = heap_cost(60) + OWNER_COST;
    assert_eq!(budget.used(), held, "the buffer and its owner record");
    assert_eq!(m.used(), held);
    drop(slice);
    assert_eq!((budget.used(), m.used()), (0, 0));
}

#[test]
fn a_decode_is_refused_before_it_outgrows_the_meter() {
    // A block declared at 10 bytes that keeps decoding: the meter holds
    // 200 bytes of buffer, and the owner record wrapping it at the end.
    let limit = 200 + OWNER_COST;
    let m = meter(limit);
    let mut out = DecodeBuf::new(&m, 10, 1 << 30, LimitKind::BlockOutput).unwrap();
    assert_eq!(m.used(), heap_cost(10));
    out.extend_from_slice(&[1; 10]).unwrap();
    let e = out.grow_zeroed(1000).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::InputMemory),
        "{e:?}"
    );
    assert_eq!(out.capacity(), 10, "never allocated past the meter");
    assert_eq!(out.len(), 10);
    assert_eq!(m.used(), heap_cost(10), "a refused growth charges nothing");
    // Growth the meter can pay for goes through, charged as it grows.
    out.extend_from_slice(&[2; 80]).unwrap();
    assert!(m.used() <= limit);
    assert_eq!(m.used(), out.charged());
    let bytes = out.into_bytes().unwrap();
    assert_eq!(
        m.used(),
        heap_cost(90) + OWNER_COST,
        "the charge equals the buffer kept and its owner record"
    );
    drop(bytes);
    assert_eq!(m.used(), 0);
}

#[test]
fn new_charges_before_it_allocates() {
    let m = meter(100);
    let _held = m.charge(90).unwrap();
    let e = DecodeBuf::new(&m, 64 << 20, 1 << 30, LimitKind::BlockOutput).unwrap_err();
    assert!(matches!(e, ChargeError::Limit(_)), "{e:?}");
    assert_eq!(m.used(), 90);
}

#[test]
fn each_block_of_a_run_has_its_own_cap() {
    let m = meter(1 << 20);
    let mut out = DecodeBuf::new(&m, 0, 16, LimitKind::BlockOutput).unwrap();
    out.extend_from_slice(&[0; 16]).unwrap();
    let e = out.extend_from_slice(&[0]).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::BlockOutput && l.max == 16 && l.value == 17),
        "{e:?}"
    );
    // The run's next block may write 16 more, and no further.
    out.begin_block(16, LimitKind::BlockOutput);
    out.extend_from_slice(&[1; 16]).unwrap();
    let before = out.capacity();
    let e = out.grow_zeroed(1).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::BlockOutput),
        "{e:?}"
    );
    assert_eq!(out.capacity(), before, "the cap is checked before growing");
    assert_eq!(out.len(), 32);
}

#[test]
fn a_dropped_decode_returns_its_charge() {
    let m = meter(1 << 20);
    let mut out = DecodeBuf::new(&m, 0, 1 << 20, LimitKind::BlockOutput).unwrap();
    out.extend_from_slice(&[0; 1000]).unwrap();
    assert!(m.used() >= 1000);
    // The codec fails here (a corrupt stream): the buffer is dropped.
    drop(out);
    assert_eq!(m.used(), 0);
}

#[test]
fn a_full_meter_still_fits_data_that_fits() {
    // 100 bytes in two writes (60 then 40) under a meter that holds 100
    // bytes' allocation and no doubling past it.
    let m = meter(heap_cost(60) + heap_cost(100));
    let mut out = DecodeBuf::new(&m, 60, 1 << 20, LimitKind::BlockOutput).unwrap();
    out.extend_from_slice(&[1; 60]).unwrap();
    out.extend_from_slice(&[2; 40]).unwrap();
    assert_eq!(out.len(), 100);
    // Wrapping it needs the owner record, which this meter has no room for.
    assert!(out.into_bytes().is_err());
    assert_eq!(m.used(), 0);
}

#[test]
fn a_decode_repeated_for_every_read_stops_at_the_work_bound() {
    // Round 3: a block that fails after decoding 1 MiB, measured again
    // by every entry's read. Each attempt returns its memory; the work
    // is counted and stops the fourth attempt before it allocates.
    let limits = crate::limits::Limits {
        max_decode_work: 3 << 20,
        ..crate::limits::Limits::default()
    };
    let m = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
    for _ in 0..3 {
        let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        out.grow_zeroed(1 << 20).unwrap();
        drop(out); // the codec failed
    }
    assert_eq!(m.used(), 0);
    let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
    let e = out.grow_zeroed(1 << 20).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork),
        "{e:?}"
    );
    assert_eq!(out.capacity(), 0, "refused before it allocated");
    // A path read or a writer's output is not decode work.
    let mut path = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::InputBytes).unwrap();
    path.grow_zeroed(1 << 20).unwrap();
}

#[test]
fn every_byte_written_counts_whatever_the_buffer_already_holds() {
    // Round 6 (Codex M, claude6 M1): work was counted only on a write
    // that grew the buffer, and only that write's own bytes, so a buffer
    // sized up front (W1), one scratch buffer reused across decodes (W2)
    // and output written in small pieces (W3) decoded without bound.
    let mib = 1usize << 20;
    let work = |max: u64| {
        let limits = crate::limits::Limits {
            max_decode_work: max,
            ..crate::limits::Limits::default()
        };
        Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input)
    };
    let is_work =
        |e: &ChargeError| matches!(e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork);
    // W1: pre-sized from blocks-info, ten blocks of 1 MiB.
    let m = work(3 << 20);
    let mut done = 0;
    for _ in 0..10 {
        let mut out = DecodeBuf::new(&m, 1 << 20, 1 << 30, LimitKind::BlockOutput).unwrap();
        out.begin_block(1 << 30, LimitKind::BlockOutput);
        match out.grow_zeroed(mib) {
            Ok(_) => done += 1,
            Err(e) => assert!(is_work(&e), "{e:?}"),
        }
    }
    assert_eq!((done, m.decoded()), (3, 3 << 20), "W1");
    // W2: one scratch buffer, truncated and reused for every decode.
    let m = work(3 << 20);
    let mut scratch = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
    let mut done = 0;
    for _ in 0..100 {
        scratch.truncate(0);
        scratch.begin_block(1 << 30, LimitKind::BlockOutput);
        match scratch.grow_zeroed(mib) {
            Ok(_) => done += 1,
            Err(e) => assert!(is_work(&e), "{e:?}"),
        }
    }
    assert_eq!((done, m.decoded()), (3, 3 << 20), "W2");
    // W3: 64 MiB written in 4 KiB pieces counts all 64 MiB.
    let m = work(u64::MAX);
    let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
    for _ in 0..(64 << 20) / 4096 {
        out.extend_from_slice(&[0; 4096]).unwrap();
    }
    assert_eq!(m.decoded(), 64 << 20, "W3");
    // Room made ahead is memory, not work: only what is written counts.
    let m = work(u64::MAX);
    let mut out = DecodeBuf::new(&m, 1 << 20, 1 << 30, LimitKind::BlockOutput).unwrap();
    out.reserve(1 << 20).unwrap();
    assert_eq!(m.decoded(), 0);
    out.extend_from_slice(&[1; 10]).unwrap();
    assert_eq!(m.decoded(), 10);
}

#[test]
fn a_refused_growth_counts_no_work_and_the_read_succeeds_once_there_is_room() {
    // Round 4: decode work was counted before the growth was granted, so
    // refusals that decoded nothing used up the allowance, and the
    // transient refusal became a permanent `DecodeWork`.
    let limits = crate::limits::Limits {
        max_decode_work: 4 << 20,
        ..crate::limits::Limits::default()
    };
    let pool = Arc::new(Counting::new(2 << 20));
    let m = Meter::for_input(&limits, pool.clone(), Reservation::Dependency);
    // Another job holds all of the pool but 64 KiB.
    let other = Meter::new(u64::MAX, pool.clone(), Reservation::Input);
    let held = other.charge((2 << 20) - (64 << 10)).unwrap();
    for _ in 0..10 {
        // Ten 1 MiB refusals: more than twice the work bound.
        let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        let e = out.grow_zeroed(1 << 20).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Budget(b) if b.what == Reservation::Dependency),
            "{e:?}"
        );
        assert!(e.is_transient());
    }
    assert_eq!(m.decoded(), 0, "a refused growth decodes nothing");
    drop(held);
    // The pool has room again: the same decode succeeds, and counts.
    let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
    out.grow_zeroed(1 << 20).unwrap();
    assert_eq!(m.decoded(), 1 << 20);
    drop(out);
    assert_eq!((m.used(), pool.used()), (0, 0));
}

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code: this file's `#[cfg(test)] mod tests;` is in
// another.
#[cfg(test)]
#[test]
fn decodes_on_several_threads_never_pass_the_work_bound_together() {
    // Round 5 (Codex M1): the bound was checked before a growth and
    // counted after it, so threads that checked at once all passed and
    // all counted. This budget holds each thread at its first
    // reservation until all eight have arrived (a test budget may block;
    // a real one must not), so every thread has checked the bound before
    // any of them counts: eight 1 MiB growths against 4 MiB of work.
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Debug)]
    struct Gate {
        barrier: std::sync::Barrier,
        arrived: AtomicUsize,
        threads: usize,
    }
    impl MemoryBudget for Gate {
        fn try_reserve(
            &self,
            bytes: u64,
            _what: Reservation,
        ) -> Result<Permit, super::BudgetExceeded> {
            if self.arrived.fetch_add(1, Ordering::AcqRel) < self.threads {
                self.barrier.wait();
            }
            Ok(Permit::detached(bytes))
        }
    }
    const THREADS: usize = 8;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let limits = crate::limits::Limits {
            max_decode_work: 4 << 20,
            ..crate::limits::Limits::default()
        };
        let gate = Arc::new(Gate {
            barrier: std::sync::Barrier::new(THREADS),
            arrived: AtomicUsize::new(0),
            threads: THREADS,
        });
        let m = Meter::for_input(&limits, gate, Reservation::Input);
        let outcomes: Vec<_> = (0..THREADS)
            .map(|_| {
                let m = m.clone();
                std::thread::spawn(move || {
                    let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput)?;
                    out.grow_zeroed(1 << 20)?;
                    Ok::<_, ChargeError>(out)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        let granted = outcomes.iter().filter(|o| o.is_ok()).count();
        let refused = outcomes
            .iter()
            .filter(|o| matches!(o, Err(ChargeError::Limit(l)) if l.kind == LimitKind::DecodeWork))
            .count();
        let held = m.used();
        let decoded = m.decoded();
        drop(outcomes);
        let _ = tx.send((granted, refused, decoded, held, m.used()));
    });
    let (granted, refused, decoded, held, after) =
        unityrs_testkit::guard::recv(&rx, "the growths to finish");
    assert_eq!(decoded, 4 << 20, "the work bound held");
    assert_eq!((granted, refused), (4, 4));
    assert_eq!(
        held,
        4 * heap_cost(1 << 20),
        "a refused growth kept no charge"
    );
    assert_eq!(after, 0);
}

#[test]
fn permits_merge_by_home() {
    let a = Counting::new(1000);
    let b = Counting::new(1000);
    // A charge's first permit is detached and empty: it takes the home.
    let mut p = Permit::detached(0);
    for _ in 0..100 {
        p.absorb(a.try_reserve(1, Reservation::Input).unwrap());
        p.absorb(Permit::detached(0));
        p.absorb(b.try_reserve(2, Reservation::Input).unwrap());
    }
    assert_eq!(p.records(), 1, "one record for the second home");
    assert_eq!((p.bytes(), a.used(), b.used()), (300, 100, 200));
    drop(p);
    assert_eq!((a.used(), b.used()), (0, 0));
}

#[test]
fn absorbed_permits_from_another_home_still_return() {
    let a = Counting::new(100);
    let b = Counting::new(100);
    let mut p = a.try_reserve(10, Reservation::Input).unwrap();
    p.absorb(a.try_reserve(5, Reservation::Input).unwrap());
    p.absorb(b.try_reserve(7, Reservation::Input).unwrap());
    p.absorb(Permit::detached(3));
    assert_eq!(p.bytes(), 25);
    assert_eq!((a.used(), b.used()), (15, 7));
    p.shrink_to(12);
    assert_eq!(p.bytes(), 12);
    assert_eq!((a.used(), b.used()), (12, 0));
    drop(p);
    assert_eq!((a.used(), b.used()), (0, 0));
}
