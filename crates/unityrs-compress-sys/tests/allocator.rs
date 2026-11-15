use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_compress_sys::{Error, LzmaError, lzma_compress, lzma_encoder_memory};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, counted};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static REFUSE: AtomicUsize = AtomicUsize::new(0);

struct Watching;

impl Tally for Watching {
    fn refuses(&self, _size: usize) -> bool {
        counted() && CALLS.fetch_add(1, Ordering::Relaxed) + 1 == REFUSE.load(Ordering::Relaxed)
    }
    fn grew(&self, size: usize, _zeroed: bool) {
        LIVE.fetch_add(size, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Allocator<Watching> = Allocator(Watching);

#[test]
fn every_native_allocation_can_fail_without_aborting_or_leaking() {
    count_this_thread();
    let allowance = lzma_encoder_memory().unwrap();
    CALLS.store(0, Ordering::Relaxed);
    let baseline = LIVE.load(Ordering::Relaxed);
    let mut produced = 0;
    lzma_compress(b"bundle payload", allowance, |part| {
        produced += part.len();
        Ok::<_, ()>(())
    })
    .unwrap();
    let allocations = CALLS.load(Ordering::Relaxed);
    assert!(allocations > 4);
    assert!(produced > 0);
    assert_eq!(LIVE.load(Ordering::Relaxed), baseline);
    for at in 1..=allocations {
        CALLS.store(0, Ordering::Relaxed);
        REFUSE.store(at, Ordering::Relaxed);
        let result = lzma_compress(b"bundle payload", allowance, |_| Ok::<_, ()>(()));
        REFUSE.store(0, Ordering::Relaxed);
        assert!(
            matches!(result, Err(LzmaError::Native(Error::Allocator(_)))),
            "allocation {at}: {result:?}"
        );
        assert_eq!(
            LIVE.load(Ordering::Relaxed),
            baseline,
            "allocation {at} leaked"
        );
    }
    let refused = lzma_compress(b"payload", 1, |_| Ok::<_, ()>(()));
    assert!(matches!(refused, Err(LzmaError::Native(Error::Allowance))));
    let sink = lzma_compress(b"payload", allowance, |_| Err("output refused"));
    assert!(matches!(sink, Err(LzmaError::Output("output refused"))));
    assert_eq!(LIVE.load(Ordering::Relaxed), baseline);
}
