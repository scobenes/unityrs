//! Refuse every allocation: metadata, names and WAVs must be charged first.
mod support;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use unityrs_core::{
    budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation},
    limits::Meter,
};
use unityrs_testkit::counted::{self, Allocator, Tally};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicU64 = AtomicU64::new(0);
static BASE_HELD: AtomicU64 = AtomicU64::new(0);
static OVER: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
struct Watching;
impl Tally for Watching {
    fn grew(&self, size: usize, _: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        if ARMED.load(Ordering::Relaxed) {
            let above = now.saturating_sub(BASE.load(Ordering::Relaxed)) as u64;
            let charge = HELD
                .load(Ordering::Relaxed)
                .saturating_sub(BASE_HELD.load(Ordering::Relaxed));
            OVER.fetch_max(above.saturating_sub(charge), Ordering::Relaxed);
        }
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
    fn refuses(&self, _: usize) -> bool {
        if !counted::counted() || !ARMED.load(Ordering::Relaxed) {
            return false;
        }
        CALLS.fetch_add(1, Ordering::Relaxed) >= FAIL_AT.load(Ordering::Relaxed)
    }
}
#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);
#[derive(Debug)]
struct Home;
impl PermitHome for Home {
    fn release(&self, bytes: u64) {
        HELD.fetch_sub(bytes, Ordering::Relaxed);
    }
}
#[derive(Debug)]
struct Budget(Arc<Home>);
impl MemoryBudget for Budget {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        HELD.fetch_add(bytes, Ordering::Relaxed);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}
#[test]
fn every_allocation_is_charged_first_and_fallible() {
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let pcm = [0u8; 32];
    for data in [
        support::bank(2, &[(&pcm, 16, 1, 8), (&pcm, 8, 2, 9)]),
        support::bank(5, &[(&pcm, 8, 1, 8)]),
        support::bank(7, &[(&[0x12; 0x24 * 6 * 3], 192, 6, 8)]),
        support::bank(16, &[(&[0x12; 0x8c * 8 * 3], 768, 8, 8)]),
        b"OggSsynthetic".to_vec(),
    ] {
        let budget = Arc::new(Budget(Arc::new(Home)));
        let m = Meter::new(u64::MAX, budget, Reservation::Input);
        let run = || {
            let p = unityrs_audio::plan(&data, true, &m)?;
            unityrs_audio::decode(&p, b"clip", &m)
        };
        sweep(run, &m);
    }
    let mut table = b"VST1".to_vec();
    for n in [1u32, 42, 20000] {
        table.extend_from_slice(&n.to_le_bytes());
    }
    table.extend_from_slice(b"\x05vorbis");
    table.resize(20016, 0);
    let dir = unityrs_testkit::scratch::Scratch::new("vorbis-charges");
    let path = dir.path().join("own-table");
    std::fs::write(&path, &table).unwrap();
    let m = Meter::new(
        u64::MAX,
        Arc::new(Budget(Arc::new(Home))),
        Reservation::Input,
    );
    sweep(|| unityrs_audio::VorbisHeaders::from_bytes(&table, &m), &m);
    sweep(|| unityrs_audio::VorbisHeaders::load(&path, &m), &m);
    counted::stop_counting_this_thread();
}

fn sweep<T>(run: impl Fn() -> Result<T, unityrs_audio::Error>, m: &Arc<Meter>) {
    BASE_HELD.store(HELD.load(Ordering::Relaxed), Ordering::Relaxed);
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    FAIL_AT.store(usize::MAX, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    let files = run();
    ARMED.store(false, Ordering::Relaxed);
    let calls = CALLS.load(Ordering::Relaxed);
    assert_eq!(OVER.load(Ordering::Relaxed), 0);
    drop(files.unwrap());
    assert_eq!(m.used(), 0);
    for at in 0..calls {
        CALLS.store(0, Ordering::Relaxed);
        FAIL_AT.store(at, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = run();
        ARMED.store(false, Ordering::Relaxed);
        assert!(result.is_err(), "allocation {at}");
        drop(result);
        assert_eq!(m.used(), 0);
        assert_eq!(
            HELD.load(Ordering::Relaxed),
            BASE_HELD.load(Ordering::Relaxed)
        );
    }
}
