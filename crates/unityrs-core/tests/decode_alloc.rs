//! A decode buffer pays before it allocates (plan section 3.6): a growth the
//! meter or the budget refuses must not allocate first and give the memory
//! back after, and neither may a buffer's first allocation.
//! A counting global allocator watches the heap of the test's own thread
//! (`unityrs_testkit::counted`): libtest's main thread allocates beside it.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::LimitKind;
use unityrs_core::budget::{Counting, DecodeBuf, Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter, heap_cost};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

struct Watching;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

impl Tally for Watching {
    fn grew(&self, size: usize, _zeroed: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(now, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);

/// Runs `f` and returns how far the heap rose above where it started.
fn rise(f: impl FnOnce()) -> usize {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    f();
    PEAK.load(Ordering::Relaxed) - base
}

fn bytes(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

#[test]
fn a_refused_growth_allocates_nothing() {
    const ASKED: usize = 64 << 20;
    const SMALL: usize = 1 << 20;
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));

    // 1. The budget refuses (a batch job's prepaid amount is spent).
    let budget = Arc::new(Counting::new(4096));
    let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    let mut out = DecodeBuf::new(&meter, 16, 1 << 30, LimitKind::BlockOutput).unwrap();
    out.extend_from_slice(&[1; 16]).unwrap();
    let grew = rise(|| {
        // An LZMA block declared small that decodes on: the codec asks for 64 MiB.
        let e = out.grow_zeroed(ASKED).unwrap_err();
        assert!(matches!(e, ChargeError::Budget(_)), "{e:?}");
    });
    assert!(
        grew < SMALL,
        "a budget refusal allocated {grew} bytes first"
    );

    // Growth the budget allows: the heap never rises past what was charged.
    let grew = rise(|| out.extend_from_slice(&[2; 2048]).unwrap());
    assert!(budget.used() <= 4096);
    assert!(bytes(grew) <= 4096, "growth past the budget: {grew} bytes");
    assert_eq!(out.len(), 16 + 2048);
    drop(out);
    assert_eq!(budget.used(), 0, "a dropped buffer returns its charge");

    // 2. The input's meter is at its limit (Codex, round 2: the growth used
    //    to allocate 64 MiB and then report the meter's refusal).
    let full = Meter::new(1 << 30, Arc::new(Unbounded), Reservation::Input);
    let mut out = DecodeBuf::new(&full, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
    let held = full.charge(full.limit()).unwrap();
    let grew = rise(|| {
        let e = out.grow_zeroed(ASKED).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind() == LimitKind::InputMemory),
            "{e:?}"
        );
    });
    assert!(grew < SMALL, "a meter refusal allocated {grew} bytes first");
    assert_eq!(out.capacity(), 0);

    // 3. A buffer's first allocation is charged too: `new` on a full meter.
    let grew = rise(|| {
        let e = DecodeBuf::new(&full, 256 << 20, 1 << 30, LimitKind::BlockOutput).unwrap_err();
        assert!(matches!(e, ChargeError::Limit(_)), "{e:?}");
    });
    assert!(grew < SMALL, "new() allocated {grew} bytes on a full meter");
    drop(held);
    assert_eq!(full.used(), 0);

    // 4. At every step of a growing decode the heap holds no more than is
    //    charged, and a growth's peak (old and new block at once) no more
    //    than the old and new charges, whether the meter books with an
    //    `Unbounded` budget or a `Counting` one (whose permits have a home,
    //    so the bookkeeping is measured too).
    for counting in [false, true] {
        let budget: Arc<dyn unityrs_core::budget::MemoryBudget> = if counting {
            Arc::new(Counting::new(u64::MAX))
        } else {
            Arc::new(Unbounded)
        };
        let meter = Meter::new(u64::MAX, budget, Reservation::Input);
        let mut out = DecodeBuf::new(&meter, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        let base = LIVE.load(Ordering::Relaxed);
        for step in [1_usize, 100, 10_000, 1 << 20, 3 << 20] {
            let old = meter.used();
            let grew = rise(|| {
                out.grow_zeroed(step).unwrap();
            });
            let held = LIVE.load(Ordering::Relaxed) - base;
            assert!(
                bytes(held) <= meter.used(),
                "heap {held} above the charge {}",
                meter.used()
            );
            assert!(
                bytes(grew) <= old + meter.used(),
                "a growth's peak {grew} above its charges"
            );
        }
        assert_eq!(meter.used(), heap_cost(bytes(out.capacity())));
    }
}
