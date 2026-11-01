use std::sync::Arc;

use super::{Charge, ChargeError, Limits, Meter, MeteredVec, heap_cost};
use crate::budget::{Counting, Reservation, Unbounded};
use crate::error::LimitKind;

#[test]
fn a_refused_charge_charges_nothing_and_drops_return_everything() {
    let meter = Meter::new(100, Arc::new(Unbounded), Reservation::Input);
    let a = meter.charge(60).unwrap();
    let e = meter.charge(41).unwrap_err();
    assert_eq!(
        e,
        ChargeError::Limit(crate::LimitError {
            kind: LimitKind::InputMemory,
            value: 101,
            max: 100
        })
    );
    assert_eq!(meter.used(), 60, "a refused charge adds nothing");
    let mut b = meter.charge(40).unwrap();
    assert!(b.grow(1).is_err());
    b.release(15);
    assert_eq!(meter.used(), 85);
    drop(a);
    assert_eq!(meter.used(), 25);
    drop(b);
    assert_eq!(meter.used(), 0);
}

#[test]
fn the_meter_books_with_the_budget_and_says_which_refused() {
    let budget = Arc::new(Counting::new(50));
    let meter = Meter::new(1000, budget.clone(), Reservation::Dependency);
    let a = meter.charge(40).unwrap();
    assert_eq!(budget.used(), 40);
    let e = meter.charge(20).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Budget(b) if b.what == Reservation::Dependency),
        "{e:?}"
    );
    assert_eq!(meter.used(), 40, "the budget's refusal is refunded");
    drop(a);
    assert_eq!((meter.used(), budget.used()), (0, 0));
}

#[test]
fn absorbed_charges_return_when_the_owner_drops() {
    let m1 = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
    let m2 = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
    let mut c = m1.charge(10).unwrap();
    c.absorb(m1.charge(5).unwrap());
    c.absorb(m2.charge(7).unwrap());
    assert_eq!(c.bytes(), 22);
    assert_eq!((m1.used(), m2.used()), (15, 7));
    drop(c);
    assert_eq!((m1.used(), m2.used()), (0, 0));
}

#[test]
fn a_metered_vec_charges_its_capacity_before_it_grows() {
    let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
    let mut v: MeteredVec<u64> = MeteredVec::new(&meter);
    for i in 0..1000 {
        v.push(i).unwrap();
    }
    let cap = u64::try_from(v.capacity()).unwrap();
    assert_eq!(
        meter.used(),
        heap_cost(cap * 8),
        "exactly the live allocation"
    );
    let (vec, charge) = v.into_parts();
    assert_eq!(vec.len(), 1000);
    drop(vec);
    drop(charge);
    assert_eq!(meter.used(), 0);
}

#[test]
fn a_refused_doubling_retries_at_the_size_needed() {
    // 100 elements fit the meter; doubling 60 to 120 does not.
    let fits = heap_cost(100) + heap_cost(60);
    let meter = Meter::new(fits, Arc::new(Unbounded), Reservation::Input);
    let mut v: MeteredVec<u8> = MeteredVec::with_capacity(&meter, 60).unwrap();
    v.extend_from_slice(&[1; 60]).unwrap();
    v.extend_from_slice(&[2; 40]).unwrap();
    assert_eq!(v.capacity(), 100);
    assert_eq!(meter.used(), heap_cost(100));
}

#[test]
fn heap_cost_covers_the_header_and_rounding() {
    assert_eq!(heap_cost(0), 0);
    assert_eq!(heap_cost(1), 32);
    assert_eq!(heap_cost(24), 32);
    assert_eq!(heap_cost(25), 48);
    assert_eq!(heap_cost(1000), 1008);
    assert_eq!(heap_cost(128 << 10), (128 << 10) + 4096);
    assert_eq!(heap_cost(u64::MAX), !4095);
    for n in 1..5000_u64 {
        assert!(heap_cost(n) >= n + 8, "{n}");
    }
}

#[test]
fn heap_cost_is_glibcs_mmapped_chunk_at_every_residue() {
    // glibc: nb = (n + 23) & !15, and an mmapped chunk is
    // ALIGN_UP(nb + 8, 4096). Round 3's probe: n = 200,684 takes 204,800
    // bytes, a page more than n + 16 rounded up.
    assert_eq!(heap_cost(200_684), 204_800);
    assert_eq!(heap_cost(4_095_984), 4_100_096);
    // A request whose padded chunk reaches the threshold may be mmapped.
    assert_eq!(heap_cost(131_049), 135_168);
    for k in [32_u64, 33, 49, 1024, 1025] {
        for r in 0..=40 {
            let n = 4096 * k - r;
            let nb = (n + 23) & !15;
            // Only a padded chunk at the threshold or above may be mmapped.
            let worst = if nb < 128 << 10 {
                nb
            } else {
                (nb + 8 + 4095) & !4095
            };
            assert!(heap_cost(n) >= worst, "{n}");
        }
    }
}

#[test]
fn a_reopen_has_its_own_work_and_shares_the_inputs_memory() {
    // Round 5: the Environment restarted the one meter's count when it
    // reopened an evicted dependency, so a nested bundle of the old open
    // that a resource read still held went on decoding on the restarted
    // allowance, and every reopen gave it a fresh one. A reopen is a new
    // meter on the same memory account; the old open keeps its count.
    let limits = Limits {
        max_input_memory: 1000,
        max_decode_work: 100,
        ..Limits::default()
    };
    let old = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Dependency);
    let held = old.charge(600).unwrap();
    old.count_decode(100).unwrap();
    // Work at the bound is within it (round 3: it was refused as `100 > 100`);
    // the next byte is not.
    assert!(old.work_used_up().is_none());
    assert!(old.count_decode(1).is_err());
    let new = old.reopen();
    assert!(new.same_input(&old));
    assert_eq!(new.reservation(), Reservation::Dependency);
    new.count_decode(100).unwrap();
    let e = old.count_decode(1).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork),
        "the old open stays used up: {e:?}"
    );
    assert_eq!((old.decoded(), new.decoded()), (100, 100));
    // Both opens charge one memory bound.
    assert_eq!(new.used(), 600);
    let e = new.charge(401).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::InputMemory && l.value == 1001),
        "{e:?}"
    );
    let more = new.charge(400).unwrap();
    assert_eq!((old.used(), new.used()), (1000, 1000));
    drop((held, more));
    assert_eq!(old.used(), 0);
    let other = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
    assert!(!other.same_input(&old));
}

#[test]
fn work_done_is_counted_past_the_bound_and_refuses_what_follows() {
    let limits = Limits {
        max_decode_work: 100,
        ..Limits::default()
    };
    let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
    meter.count_decode(40).unwrap();
    assert!(meter.work_used_up().is_none());
    // A computation that ended unkept was granted 90 bytes: counted as done.
    meter.add_work(90);
    assert_eq!(meter.decoded(), 130);
    let used_up = meter.work_used_up().unwrap();
    assert_eq!(
        (used_up.kind, used_up.value, used_up.max),
        (LimitKind::DecodeWork, 130, 100)
    );
    assert!(meter.count_decode(1).is_err());
    assert_eq!(meter.granted(), 0, "work is not memory");
    let c = meter.charge(7).unwrap();
    drop(c);
    assert_eq!(
        (meter.granted(), meter.used()),
        (7, 0),
        "grants are never returned"
    );
}

#[test]
fn decode_work_is_counted_and_never_returned() {
    let limits = Limits {
        max_decode_work: 100,
        ..Limits::default()
    };
    let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
    meter.count_decode(60).unwrap();
    meter.count_decode(40).unwrap();
    let e = meter.count_decode(1).unwrap_err();
    assert!(
        matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork && l.value == 101 && l.max == 100),
        "{e:?}"
    );
    assert_eq!(meter.decoded(), 100, "a refusal counts nothing");
    assert_eq!(meter.used(), 0, "work is not memory");
}

#[test]
fn a_charge_keeps_one_record_per_meter_and_per_home() {
    // Round 3: under a Counting budget every growth left a Permit in its
    // owner's bookkeeping, uncharged.
    let budget = Arc::new(Counting::new(u64::MAX));
    let m1 = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    let m2 = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    let mut owner = Charge::zero(&m1);
    for i in 0..10_000_u64 {
        let mut c = Charge::zero(&m1);
        c.grow(1 + i % 7).unwrap();
        owner.absorb(c);
        owner.absorb(Charge::zero(&m1));
        owner.grow(3).unwrap();
        owner.absorb(m2.charge(2).unwrap());
    }
    assert_eq!(owner.records(), 1, "one record, for the other meter");
    assert_eq!(budget.used(), owner.bytes());
    let mut v: MeteredVec<u64> = MeteredVec::new(&m1);
    for i in 0..100_000 {
        v.push(i).unwrap();
    }
    assert_eq!(v.charge().records(), 0);
    drop((owner, v));
    assert_eq!((budget.used(), m1.used(), m2.used()), (0, 0, 0));
}

#[test]
fn a_boxed_slice_is_charged_at_its_exact_size() {
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let mut v: MeteredVec<u32> = MeteredVec::new(&meter);
    for i in 0..1000 {
        v.push(i).unwrap();
    }
    assert!(v.capacity() > 1000);
    let (b, charge) = v.into_boxed_slice().unwrap();
    assert_eq!(b.len(), 1000);
    assert_eq!(meter.used(), heap_cost(4000));
    assert_eq!(charge.bytes(), heap_cost(4000));
    drop((b, charge));
    // A meter with no room for the exact copy refuses and keeps nothing.
    let tight = Meter::new(heap_cost(8 * 4), Arc::new(Unbounded), Reservation::Input);
    let mut v: MeteredVec<u32> = MeteredVec::with_capacity(&tight, 8).unwrap();
    v.push(1).unwrap();
    assert!(v.into_boxed_slice().is_err());
    assert_eq!(tight.used(), 0);
}

#[test]
fn the_defaults() {
    let l = Limits::default();
    assert_eq!(l.max_input_memory, 8 << 30);
    assert_eq!(l.max_input_bytes, 4 << 30);
    assert_eq!(l.max_block_output, 1 << 30);
}

/// Every refusal of the dependency pool the core meets is counted on its
/// thread ([`super::pool_refusals`]), the Environment's one retry of a pool
/// refusal telling by the count a refusal it already retried from a later
/// one (`env::registry::with_room`; review round 8): a budget's refusal of
/// a dependency's meter, and an allocation the allocator refuses after a
/// dependency's meter granted it ([`super::reserve_exact`]), but no
/// refusal of another reservation.
#[test]
fn every_refusal_of_the_dependency_pool_is_counted() {
    let pool = Arc::new(Counting::new(0));
    let refused = |what: Reservation| {
        let before = super::pool_refusals();
        let meter = Meter::new(100, pool.clone(), what);
        assert!(matches!(meter.charge(1), Err(ChargeError::Budget(_))));
        super::pool_refusals() - before
    };
    assert_eq!(refused(Reservation::Dependency), 1);
    assert_eq!(refused(Reservation::Input), 0);
    // A request past `isize::MAX` bytes, which no allocator meets, granted
    // first by a meter with no bound.
    let huge = |what: Reservation| {
        let before = super::pool_refusals();
        let meter = Meter::new(u64::MAX, Arc::new(Unbounded), what);
        let mut v: Vec<u64> = Vec::new();
        let e = super::reserve_exact(&mut v, usize::MAX / 4, 1, &meter).unwrap_err();
        assert_eq!(e.what, what);
        super::pool_refusals() - before
    };
    assert_eq!(huge(Reservation::Dependency), 1);
    assert_eq!(huge(Reservation::Input), 0);
}
