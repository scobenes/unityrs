//! X12's live charge feed, independent of the progress sampler.
#![cfg(feature = "test-hooks")]
use std::sync::{Arc, Mutex};
use unityrs_batch::{budget::SealedBudget, run::hooks::memory};
use unityrs_core::budget::{MemoryBudget, Reservation};

static EVENTS: Mutex<Vec<(u64, bool)>> = Mutex::new(Vec::new());

#[test]
fn granted_charges_and_cross_thread_returns_are_observed_once() {
    memory::install(memory::Observer {
        charge: |bytes, held| EVENTS.lock().unwrap().push((bytes, held)),
        network: |_, _| {},
        point: |_| {},
    })
    .unwrap();
    let budget = Arc::new(SealedBudget::new(64, 64));
    let first = budget.try_reserve(48, Reservation::Input).unwrap();
    assert!(budget.try_reserve(17, Reservation::Input).is_err());
    let second = budget.try_reserve(16, Reservation::Dependency).unwrap();
    std::thread::spawn(move || drop((first, second)))
        .join()
        .unwrap();
    assert!(budget.snapshot().is_clear());
    assert_eq!(
        *EVENTS.lock().unwrap(),
        [(48, true), (16, true), (48, false), (16, false)]
    );
}
