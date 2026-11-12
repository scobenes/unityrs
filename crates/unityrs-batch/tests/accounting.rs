//! Refusal routing, permit homes, overflow, and the two accounting units.
use unityrs_batch::{budget::SealedBudget, work::Work};
use unityrs_core::budget::{MemoryBudget, Reservation};

#[test]
fn peek_budget_refuses_every_non_input_reservation_including_zero() {
    let b = SealedBudget::input_only(10);
    for bytes in [0, 1, u64::MAX] {
        for what in [Reservation::Dependency, Reservation::Provider] {
            assert!(b.try_reserve(bytes, what).is_err());
        }
    }
    let p = b.try_reserve(10, Reservation::Input).unwrap();
    assert!(b.try_reserve(0, Reservation::Input).is_ok());
    assert!(b.try_reserve(1, Reservation::Input).is_err());
    drop(p);
    assert!(b.snapshot().is_clear());
    assert_eq!(b.snapshot().refusals(), 7);
    assert!(
        SealedBudget::new(0, 0)
            .try_reserve(0, Reservation::Dependency)
            .is_ok()
    );
}

#[test]
fn reservations_keep_distinct_homes_and_peaks_after_release() {
    let b = SealedBudget::new(10, 20);
    let mut p = b.try_reserve(4, Reservation::Input).unwrap();
    p.absorb(b.try_reserve(6, Reservation::Input).unwrap());
    assert_eq!(p.records(), 0);
    p.absorb(b.try_reserve(20, Reservation::Dependency).unwrap());
    assert_eq!(p.records(), 1);
    assert!(b.try_reserve(1, Reservation::Input).is_err());
    assert!(b.try_reserve(1, Reservation::Dependency).is_err());
    assert!(b.try_reserve(0, Reservation::Provider).is_err());
    let live = b.snapshot();
    assert_eq!((live.input.used, live.dependency.used), (10, 20));
    assert_eq!(live.refusals(), 3);
    assert!(!live.is_clear());
    p.shrink_to(10);
    assert_eq!(b.snapshot().dependency.used, 0);
    drop(p);
    let end = b.snapshot();
    assert!(end.is_clear());
    assert_eq!((end.input.peak, end.dependency.peak), (10, 20));
}

#[test]
fn overflowing_reservations_are_refused_without_changing_use() {
    let b = SealedBudget::new(u64::MAX, 0);
    let p = b.try_reserve(u64::MAX, Reservation::Input).unwrap();
    assert!(b.try_reserve(1, Reservation::Input).is_err());
    assert_eq!(b.snapshot().input.used, u64::MAX);
    drop(p);
    assert!(b.snapshot().is_clear());
}

#[test]
fn work_counts_refused_increment_and_rounds_each_file_independently() {
    let mut w = Work::new(12289);
    w.pixels(1).unwrap();
    w.file(0).unwrap();
    w.file(4097).unwrap();
    assert_eq!((w.export_work(), w.spooled_part()), (12289, 12288));
    let e = w.file(1).unwrap_err();
    assert_eq!(e.value, 12289);
    assert_eq!(w.export_work(), 16385);
    assert_eq!(w.spooled_part(), 16384);
    w.failure_record(1).unwrap();
    assert_eq!((w.export_work(), w.spooled_part()), (16385, 20480));
    assert!(Work::new(u64::MAX).file(u64::MAX).is_err());
}
