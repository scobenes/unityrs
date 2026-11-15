//! Packed-input work and decoder-internal Brotli limits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use unityrs_core::{
    BundleFile, Bytes, Error, FileContext, LimitKind, RawStr, WebFile,
    budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation},
    codec::Lzma,
    limits::Meter,
};
use unityrs_testkit::web;

#[derive(Debug, Default)]
struct Peak {
    held: Arc<Held>,
}
#[derive(Debug, Default)]
struct Held {
    now: AtomicU64,
    peak: AtomicU64,
}
impl PermitHome for Held {
    fn release(&self, bytes: u64) {
        self.now.fetch_sub(bytes, Ordering::AcqRel);
    }
}
impl MemoryBudget for Peak {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        let now = self.held.now.fetch_add(bytes, Ordering::AcqRel) + bytes;
        self.held.peak.fetch_max(now, Ordering::AcqRel);
        Ok(Permit::new(bytes, self.held.clone()))
    }
}
fn setup(inflate: u64, work: u64) -> (Arc<FileContext>, Arc<Meter>, Arc<Peak>) {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_inflate = inflate;
    ctx.limits.max_decode_work = work;
    let budget = Arc::new(Peak::default());
    let meter = Meter::for_input(&ctx.limits, budget.clone(), Reservation::Input);
    (Arc::new(ctx), meter, budget)
}

fn padded_legacy(padding: usize) -> Vec<u8> {
    let mut data = web::legacy(b"UnityWeb", 5, &[("raw", b"payload")]);
    let header = u32::from_be_bytes(data[54..58].try_into().unwrap()) as usize;
    data.resize(data.len() + padding, 0);
    let packed = u32::try_from(data.len() - header).unwrap();
    data[66..70].copy_from_slice(&packed.to_be_bytes());
    data
}

#[test]
fn legacy_lzma_trailing_input_is_refused_before_it_is_copied() {
    let data = padded_legacy(1 << 20);
    let (ctx, meter, budget) = setup(1 << 20, 1024);
    let error = BundleFile::open(RawStr::from("x"), data.into(), None, &meter, &ctx).unwrap_err();
    assert!(matches!(error, Error::Limit(l) if l.kind() == LimitKind::DecodeWork));
    assert!(meter.decoded() <= 1024);
    assert!(budget.held.peak.load(Ordering::Acquire) < 65536);
    assert_eq!(meter.used(), 0);
}

#[test]
fn legacy_packed_work_is_credit_for_output_not_a_second_charge() {
    let data = padded_legacy(4096);
    let packed = u32::from_be_bytes(data[66..70].try_into().unwrap());
    let (ctx, meter, _) = setup(1 << 20, 65536);
    let file = BundleFile::open(RawStr::from("x"), data.into(), None, &meter, &ctx).unwrap();
    assert_eq!(file.entry_bytes(0).unwrap().as_ref(), b"payload");
    // The four NUL-terminated strings searched are 9, 6, 11 and 4 bytes.
    assert_eq!(meter.decoded(), u64::from(packed) + 30);
    drop(file);
    assert_eq!(meter.used(), 0);
}

#[test]
fn legacy_decoder_budget_refusals_keep_copied_work_and_exhaust_retries() {
    #[derive(Debug, Default)]
    struct RefuseDecoder(Peak);
    impl MemoryBudget for RefuseDecoder {
        fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
            // Legacy decoding starts with the minimum 4 KiB dictionary.
            if bytes == Lzma::decoder_charge(4096) {
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
            self.0.try_reserve(bytes, what)
        }
    }

    let data = Bytes::from(padded_legacy(65536));
    let packed = u64::from(u32::from_be_bytes(data[66..70].try_into().unwrap()));
    // The packed input is copied except for its eight stored-size bytes.
    let copied = packed - 8;
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_decode_work = 3 * packed;
    let budget = Arc::new(RefuseDecoder::default());
    let meter = Meter::for_input(&ctx.limits, budget.clone(), Reservation::Input);
    let ctx = Arc::new(ctx);
    for _ in 0..2 {
        let before = meter.decoded();
        let error =
            BundleFile::open(RawStr::from("x"), data.clone(), None, &meter, &ctx).unwrap_err();
        assert!(
            matches!(error, Error::Budget(ref b) if b.requested == Lzma::decoder_charge(4096)),
            "{error:?}"
        );
        assert!(meter.decoded() - before >= copied);
        assert_eq!(meter.used(), 0);
        assert_eq!(budget.0.held.now.load(Ordering::Acquire), 0);
    }
    // Header searches also cost work; a third packed copy no longer fits.
    for _ in 0..3 {
        let before = meter.decoded();
        let error =
            BundleFile::open(RawStr::from("x"), data.clone(), None, &meter, &ctx).unwrap_err();
        assert!(matches!(error, Error::Limit(l) if l.kind() == LimitKind::DecodeWork));
        assert!(meter.decoded() - before < copied);
        assert!(meter.decoded() <= ctx.limits.max_decode_work);
        assert_eq!(meter.used(), 0);
        assert_eq!(budget.0.held.now.load(Ordering::Acquire), 0);
    }
}

#[test]
fn overlapping_legacy_entries_pay_for_each_packed_copy() {
    let inner = padded_legacy(65536);
    let names: Vec<_> = (0..8).map(|i| format!("inner{i}")).collect();
    let entries: Vec<_> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    let mut outer = web::plain("UnityWebData1.0", &entries);
    let offset = u32::try_from(outer.len()).unwrap();
    let mut at = 20;
    for name in &names {
        outer[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        outer[at + 4..at + 8].copy_from_slice(&u32::try_from(inner.len()).unwrap().to_le_bytes());
        at += 12 + name.len();
    }
    outer.extend_from_slice(&inner);
    let (ctx, meter, _) = setup(1 << 20, 3 * 65536);
    let web = WebFile::open(RawStr::from("outer"), outer.into(), None, &meter, &ctx).unwrap();
    let mut loaded = 0;
    for i in 0..names.len() {
        let before = meter.decoded();
        match web.entry(i) {
            Ok(_) => {
                loaded += 1;
                assert!(meter.decoded() - before >= 65536);
            }
            Err(e) => {
                assert!(matches!(e, Error::Limit(l) if l.kind() == LimitKind::DecodeWork));
                break;
            }
        }
    }
    assert!(
        (1..=2).contains(&loaded),
        "loaded {loaded} overlapping entries"
    );
    drop(web);
    assert_eq!(meter.used(), 0);
}

/// Brotli 1.2.1, quality 11, lgwin 24: a 20-byte empty WebFile header
/// followed by 12,000,000 zero bytes, with an RFC 7932 metadata marker.
fn brotli_bomb() -> Bytes {
    let mut data = vec![111, 61, 1];
    data.resize(83, 0);
    data[32..38].copy_from_slice(b"brotli");
    data.extend_from_slice(&[
        57, 177, 113, 139, 127, 156, 114, 9, 36, 220, 202, 40, 111, 187, 192, 83, 16, 81, 110, 43,
        4, 28, 210, 82, 223, 91, 59, 227, 149, 184, 141, 12, 2,
    ]);
    data.into()
}

#[test]
fn a_tiny_brotli_bomb_cannot_allocate_or_inflate_its_declared_window() {
    let bytes = brotli_bomb();
    assert_eq!(bytes.len(), 116);
    for (inflate, work, kind) in [
        (100, 1024, LimitKind::Inflate),
        (32 << 20, 1024, LimitKind::DecodeWork),
    ] {
        let (ctx, meter, budget) = setup(inflate, work);
        let error =
            WebFile::open(RawStr::from("x"), bytes.clone(), None, &meter, &ctx).unwrap_err();
        assert!(
            matches!(error, Error::Limit(l) if l.kind() == kind),
            "{error:?}"
        );
        let peak = budget.held.peak.load(Ordering::Acquire);
        assert!(peak < 65536, "decoder peak: {peak}");
        assert!(meter.decoded() <= 1024, "decoder work: {}", meter.decoded());
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn the_brotli_bomb_is_a_complete_stream_and_every_output_byte_is_work() {
    let bytes = brotli_bomb();
    let (ctx, meter, _) = setup(32 << 20, 20 << 20);
    let file = WebFile::open(RawStr::from("x"), bytes, None, &meter, &ctx).unwrap();
    assert_eq!(file.data().len(), 12_000_020);
    assert_eq!(&file.data()[..20], b"UnityWebData1.0\0\x14\0\0\0");
    assert!(file.data()[20..].iter().all(|b| *b == 0));
    assert!(meter.decoded() >= 12_000_020);
    drop(file);
    assert_eq!(meter.used(), 0);
}

/// The same encoder and header, lgwin 16, 200,000 zero bytes. Output
/// crosses three 64 KiB ring boundaries, without large compressed input.
fn wrapping_brotli() -> Bytes {
    let mut data = vec![172, 39];
    data.resize(82, 0);
    data[32..38].copy_from_slice(b"brotli");
    data.extend_from_slice(&[
        250, 255, 15, 248, 199, 41, 151, 64, 194, 173, 140, 242, 182, 11, 60, 5, 17, 229, 182, 66,
        192, 33, 45, 245, 189, 181, 51, 212, 251, 128, 200, 32, 212, 84, 67, 224, 159, 0, 136, 195,
        34, 0, 135, 130, 0,
    ]);
    data.into()
}

#[test]
fn brotli_ring_wraps_count_output_once_and_refused_inflation_counts_unflushed_bytes() {
    let bytes = wrapping_brotli();
    assert_eq!(bytes.len(), 127);
    let (ctx, meter, _) = setup(1 << 20, 1 << 20);
    let file = WebFile::open(RawStr::from("x"), bytes.clone(), None, &meter, &ctx).unwrap();
    assert_eq!(file.data().len(), 200_020);
    // Input, actual output (larger than initialized decoder storage), and
    // the 16-byte signature search. Flushing the ring adds no second count.
    assert_eq!(meter.decoded(), 127 + 200_020 + 16);
    drop(file);
    assert_eq!(meter.used(), 0);

    let (ctx, meter, _) = setup(100_000, 1 << 20);
    let error = WebFile::open(RawStr::from("x"), bytes, None, &meter, &ctx).unwrap_err();
    let Error::Limit(limit) = error else {
        panic!("{error:?}")
    };
    assert_eq!(limit.kind(), LimitKind::Inflate);
    // The second ring fill crossed the cap before it could be flushed.
    assert_eq!(limit.value(), 2 * 65536);
    assert_eq!(meter.decoded(), 127 + 2 * 65536);
    assert_eq!(meter.used(), 0);
}

#[test]
fn brotli_replenishes_ring_work_before_decoding_another_window() {
    let (ctx, meter, budget) = setup(1 << 20, 100_000);
    let error =
        WebFile::open(RawStr::from("x"), wrapping_brotli(), None, &meter, &ctx).unwrap_err();
    assert!(matches!(error, Error::Limit(l) if l.kind() == LimitKind::DecodeWork));
    assert!((65536..=100_000).contains(&meter.decoded()));
    assert!(budget.held.peak.load(Ordering::Acquire) < 256 * 1024);
    assert_eq!(meter.used(), 0);
}
