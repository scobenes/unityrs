//! Decoder reservations and archive member lifetimes, measured on the heap.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use unityrs_core::{
    Bytes, FileContext, LimitKind, RawStr,
    budget::{Counting, Reservation, Unbounded},
    env::{EnvOptions, Environment, Source},
    limits::{Limits, Meter},
    zip::ZipArchive,
};
use unityrs_testkit::{
    counted::{Allocator, Tally, count_this_thread, others_are_not_counted},
    zip::archive,
};

struct Watching;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
impl Tally for Watching {
    fn grew(&self, size: usize, _: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(now, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}
#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);
fn rise(f: impl FnOnce()) -> usize {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    f();
    PEAK.load(Ordering::Relaxed) - base
}
fn load(env: &Environment, data: Bytes) {
    env.load_zip_file(Source::Bytes {
        name: RawStr::from("test.zip"),
        data,
    })
    .unwrap();
}

#[test]
fn zip_decoder_reserves_before_allocation_and_single_use_members_are_released() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    // Characterize flate2 1.1.10 / zlib-rs 0.6.8, including its 32 KiB
    // window: the production reservation must cover the measured peak.
    let mut measured = 0;
    let peak = rise(|| {
        let base = LIVE.load(Ordering::Relaxed);
        let mut dec = flate2::Decompress::new(false);
        dec.decompress(
            &[1, 0, 0, 255, 255],
            &mut [0; 1],
            flate2::FlushDecompress::None,
        )
        .unwrap();
        measured = LIVE.load(Ordering::Relaxed) - base;
        drop(dec);
        assert_eq!(LIVE.load(Ordering::Relaxed), base);
    });
    eprintln!("decoder heap: {measured} bytes, peak {peak} bytes");
    assert!(measured >= 32768, "backend bypassed counting: {measured}");
    assert!(peak <= 65536, "decoder heap grew to {peak}");

    for budget_refuses in [false, true] {
        let mut limits = Limits::default();
        if !budget_refuses {
            limits.max_input_memory = 16 * 1024;
        }
        let budget = Arc::new(Counting::new(if budget_refuses {
            16 * 1024
        } else {
            u64::MAX
        }));
        let meter = Meter::for_input(&limits, budget.clone(), Reservation::Input);
        let zip = ZipArchive::open(archive(&[("a", b"", true)]).into(), &meter).unwrap();
        let used = budget.used();
        let peak = rise(|| {
            let error = zip.read(0, &limits).unwrap_err();
            assert!(
                matches!(error, unityrs_core::Error::Budget(_))
                    || matches!(error, unityrs_core::Error::Limit(ref e) if e.kind() == LimitKind::InputMemory)
            );
        });
        assert!(peak < 32768, "refusal first allocated the backend: {peak}");
        assert_eq!(budget.used(), used);
        drop(zip);
        assert_eq!(budget.used(), 0);
    }

    let part = vec![0; 32 << 20];
    let data: Bytes = archive(&[("a.split0", &part, false), ("a.split1", &part, false)]).into();
    drop(part);
    let env = Environment::new(EnvOptions::unitypy());
    let peak = rise(|| load(&env, data));
    eprintln!("split parts peak growth: {peak} bytes");
    assert!(peak < 130 << 20, "single-use split parts retained: {peak}");
    drop(env);

    let raw = vec![0; 40 << 20];
    let first = archive(&[("first", &raw, false)]);
    let second = archive(&[("second", &raw, false)]);
    let data: Bytes = archive(&[("one", &first, false), ("two", &second, false)]).into();
    drop((raw, first, second));
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_memory = 140 << 20;
    ctx.budget = Arc::new(Unbounded);
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(ctx);
    let env = Environment::new(options);
    let peak = rise(|| load(&env, data));
    eprintln!("nested ZIP peak growth: {peak} bytes");
    assert!(peak < 130 << 20, "single-use nested zips retained: {peak}");
    assert_eq!(env.files().count(), 2);
}
