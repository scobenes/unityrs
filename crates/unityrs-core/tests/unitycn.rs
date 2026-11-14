//! Synthetic UnityCN acceptance tests. All keys here are made up.
use std::sync::Arc;
use unityrs_core::budget::Reservation;
use unityrs_core::limits::Meter;
use unityrs_core::{BundleFile, Bytes, Error, FileContext, RawStr};
use unityrs_testkit::bundle::{BundleSpec, build};

#[test]
fn key_validation_is_local_and_preserves_the_previous_key() {
    let mut ctx = FileContext::default();
    ctx.set_assetbundle_decrypt_key(b"synthetic-key-01")
        .unwrap();
    for size in [0, 15, 17, 32] {
        let err = ctx.set_assetbundle_decrypt_key(vec![0; size]).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "AssetBundle Key length is wrong. It should be 16 bytes and now is {size} bytes."
            )
        );
    }
    assert!(!format!("{ctx:?}").contains("synthetic-key-01"));
    let bundle = support::bundle(b"2022.3.0f1", 7, 0x1000, false, false, false);
    let bundle = open(bundle, &Arc::new(ctx)).unwrap();
    assert_eq!(bundle.entry_bytes(0).unwrap(), support::plaintext());
}

#[test]
fn missing_key_is_a_lookup_error_after_reading_the_header() {
    let data = build(&BundleSpec {
        encrypted: true,
        ..BundleSpec::default()
    });
    let ctx = Arc::new(FileContext::default());
    let meter = Meter::new(
        ctx.limits.max_input_memory,
        ctx.budget.clone(),
        Reservation::Input,
    );
    let got = BundleFile::open(
        RawStr::from("synthetic.bundle"),
        Bytes::from(data),
        None,
        &meter,
        &ctx,
    );
    assert!(matches!(got, Err(Error::UnityCnKeyMissing)));
    assert_eq!(meter.used(), 0);
}

#[path = "support/unitycn.rs"]
mod support;
use unityrs_core::Unsupported;
use unityrs_core::codec::{BlockDecryptor, unitycn::UnityCnDecryptor};
use unityrs_core::stream::{Endian, Reader};

fn context(key: &[u8]) -> Arc<FileContext> {
    let mut ctx = FileContext::default();
    ctx.set_assetbundle_decrypt_key(key).unwrap();
    Arc::new(ctx)
}

fn open(data: Vec<u8>, ctx: &Arc<FileContext>) -> Result<Arc<BundleFile>, Error> {
    let meter = Meter::new(
        ctx.limits.max_input_memory,
        ctx.budget.clone(),
        Reservation::Input,
    );
    BundleFile::open(
        RawStr::from("synthetic.bundle"),
        Bytes::from(data),
        None,
        &meter,
        ctx,
    )
}

#[test]
fn encrypted_lz4_bundles_round_trip_with_each_flag_and_alignment() {
    let inputs = support::inputs();
    let ctx = context(support::KEY);
    for case in inputs["bundles"].as_array().unwrap() {
        let bundle = open(support::unhex(case["hex"].as_str().unwrap()), &ctx).unwrap();
        assert!(bundle.header().encrypted);
        bundle.load_all().unwrap();
        assert_eq!(
            if case["path"].is_array() {
                let unityrs_core::File::Bundle(inner) = bundle.entry(0).unwrap() else {
                    panic!("nested fixture must contain a bundle");
                };
                inner.entry_bytes(0).unwrap()
            } else {
                bundle.entry_bytes(0).unwrap()
            },
            support::plaintext(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn wrong_key_and_damaged_signature_fail_at_open() {
    let data = support::bundle(b"2022.3.0f1", 7, 0x1000, false, false, false);
    assert!(matches!(
        open(data.clone(), &context(b"synthetic-key-02")),
        Err(Error::UnityCnSignature)
    ));
    // Successful validation does not poison another load's key state.
    assert!(open(data, &context(support::KEY)).is_ok());
    let mut header = support::header(&support::INDEX, &support::SUBSTITUTE);
    header[37] ^= 1;
    assert!(matches!(
        UnityCnDecryptor::read(&mut Reader::new(&header, Endian::Big), Some(support::KEY)),
        Err(Error::UnityCnSignature)
    ));
}

#[test]
fn boost_virtual_zero_and_tier_u_reads_are_distinguished() {
    let inputs = support::inputs();
    for (case, expected) in inputs["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .zip(["e7", "c7", "f7", "fe", "f5", "0000", "f0", ""])
    {
        let index = support::unhex(case["index"].as_str().unwrap())
            .try_into()
            .unwrap();
        let sub = support::unhex(case["substitute"].as_str().unwrap())
            .try_into()
            .unwrap();
        let header = support::header(&index, &sub);
        let decryptor =
            UnityCnDecryptor::read(&mut Reader::new(&header, Endian::Big), Some(support::KEY))
                .unwrap();
        let mut block = support::unhex(case["hex"].as_str().unwrap());
        let result = decryptor.decrypt(&mut block, 0);
        if case["tier_u"] == true {
            assert!(matches!(
                result,
                Err(Error::Unsupported(Unsupported::UnityCnWalk))
            ));
        } else {
            result.unwrap();
            assert_eq!(support::hex(&block), expected);
        }
    }
}

#[test]
fn vector_read_positions_and_short_reads_match_the_header_layout() {
    let full = support::header(&support::INDEX, &support::SUBSTITUTE);
    assert_eq!(full.len(), 70);
    for len in 4..=70 {
        let mut r = Reader::new(&full[..len], Endian::Big);
        let got = UnityCnDecryptor::read(&mut r, Some(support::KEY));
        // The last byte of each vector is skipped, so missing the final
        // skipped byte (69 bytes) still constructs the decryptor.
        assert_eq!(got.is_ok(), len >= 69, "length {len}: {got:?}");
        let mut r = Reader::new(&full[..len], Endian::Big);
        assert!(matches!(
            UnityCnDecryptor::read(&mut r, None),
            Err(Error::UnityCnKeyMissing)
        ));
    }
    let mut r = Reader::new(&full, Endian::Big);
    UnityCnDecryptor::read(&mut r, Some(support::KEY)).unwrap();
    assert_eq!(r.position(), 70);
}

#[test]
fn decryption_returns_its_charges_and_retries_after_budget_refusal() {
    use unityrs_core::budget::{Counting, MemoryBudget};
    let budget = Arc::new(Counting::new(1 << 20));
    let mut ctx = FileContext::default();
    ctx.budget = budget.clone();
    ctx.set_assetbundle_decrypt_key(support::KEY).unwrap();
    let ctx = Arc::new(ctx);
    let meter = Meter::new(
        ctx.limits.max_input_memory,
        ctx.budget.clone(),
        Reservation::Input,
    );
    let data = support::bundle(b"2022.3.0f1", 7, 0x1000, false, false, false);
    let bundle = BundleFile::open(
        RawStr::from("synthetic.bundle"),
        Bytes::from(data),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    let held = budget.used();
    let rest = budget
        .try_reserve((1 << 20) - held, Reservation::Input)
        .unwrap();
    assert!(matches!(bundle.entry_bytes(0), Err(Error::Budget(_))));
    drop(rest);
    assert_eq!(budget.used(), held);
    assert_eq!(bundle.entry_bytes(0).unwrap(), support::plaintext());
    drop(bundle);
    assert_eq!(meter.used(), 0);
    assert_eq!(budget.used(), 0);
}

#[test]
fn encrypted_block_work_is_refused_before_the_walk() {
    use unityrs_core::LimitKind;
    let ctx = context(support::KEY);
    let data = support::bundle(b"2022.3.0f1", 7, 0x1000, false, false, false);
    let meter = Meter::new(
        ctx.limits.max_input_memory,
        ctx.budget.clone(),
        Reservation::Input,
    );
    let bundle = BundleFile::open(
        RawStr::from("synthetic.bundle"),
        Bytes::from(data.clone()),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    let opened_work = meter.decoded();
    drop(bundle);
    let meter = Meter::with_decode_work(
        ctx.limits.max_input_memory,
        opened_work,
        ctx.budget.clone(),
        Reservation::Input,
    );
    let bundle = BundleFile::open(
        RawStr::from("synthetic.bundle"),
        Bytes::from(data),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    assert!(
        matches!(bundle.entry_bytes(0), Err(Error::Limit(l)) if l.kind() == LimitKind::DecodeWork)
    );
    drop(bundle);
    assert_eq!(meter.used(), 0);
}

#[test]
fn inverse_covers_token_walks_at_wrapping_index_boundaries() {
    let header = support::header(&support::INDEX, &support::SUBSTITUTE);
    let decryptor =
        UnityCnDecryptor::read(&mut Reader::new(&header, Endian::Big), Some(support::KEY)).unwrap();
    for index in [0, 1, 15, 16, 255, 256, usize::MAX - 1, usize::MAX] {
        let mut plain = unityrs_testkit::bundle::compress(
            unityrs_testkit::bundle::TestCodec::Lz4,
            &support::plaintext(),
        );
        let expected = plain.clone();
        support::encrypt(&mut plain, index);
        decryptor.decrypt(&mut plain, index).unwrap();
        assert_eq!(plain, expected, "index {index}");
    }
}
