//! CAB operations pay for copied names and snapshots before performing work.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::files::{
    BundleFile, BundleSaveOptions, File, SaveError, SaveLayout, WebFile, WebSaveOptions,
};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::{Bytes, FileContext, LimitKind, RawStr};

const WORK: u64 = 1 << 20;

#[test]
fn serialized_cab_nul_path_refuses_before_registering_any_state() {
    for asset_name in ["CAB-asset", "CAB-asset\0suffix"] {
        let meter = Meter::unlimited();
        let asset = unityrs_testkit::env::serialized(
            &[],
            &[unityrs_testkit::env::Obj::named(7, "asset", &[])],
        );
        let parent = web(&[(asset_name, &asset)], &meter);
        let File::Serialized(sf) = parent.entry(0).unwrap() else {
            panic!("SerializedFile")
        };
        let name = RawStr::from("valid\0name.resS");
        let before = meter.used();
        assert!(matches!(
            sf.get_writeable_cab_with_guid(Some(&name), [b'x'; 16]),
            Err(SaveError::Range("CAB external path"))
        ));
        assert_eq!(meter.used(), before);
        assert!(sf.writeable_externals().unwrap().is_empty());
        assert!(parent.writeable_cabs().unwrap().is_empty());
    }
}

#[derive(Debug, Default)]
struct Attempts(AtomicUsize);
impl MemoryBudget for Attempts {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(Permit::detached(bytes))
    }
}
fn metered() -> (Arc<Meter>, Arc<Attempts>) {
    let budget = Arc::new(Attempts::default());
    (
        Meter::with_decode_work(1 << 20, WORK, budget.clone(), Reservation::Input),
        budget,
    )
}
fn web(rows: &[(&str, &[u8])], meter: &Arc<Meter>) -> Arc<WebFile> {
    WebFile::open(
        RawStr::from_static(b"cab.web"),
        Bytes::from(unityrs_testkit::web::plain("UnityWebData1.0", rows)),
        None,
        meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap()
}
fn exhaust(meter: &Meter) {
    meter.count_decode(WORK - meter.decoded()).unwrap();
}
fn work_error<T>(result: Result<T, SaveError>) {
    let Some(SaveError::Charge(ChargeError::Limit(error))) = result.err() else {
        panic!("expected DecodeWork limit refusal")
    };
    assert_eq!(error.kind(), LimitKind::DecodeWork);
}

#[test]
fn first_cab_name_copy_refuses_work_before_allocating() {
    let (meter, budget) = metered();
    let web = web(&[], &meter);
    let name = RawStr::from("long-".repeat(1024).as_str());
    exhaust(&meter);
    budget.0.store(0, Ordering::Relaxed);
    work_error(web.get_writeable_cab(Some(&name)));
    assert_eq!(
        budget.0.load(Ordering::Relaxed),
        0,
        "name copying cannot start before work is granted"
    );
}

#[test]
fn serialized_archive_path_copy_refuses_work_before_allocating() {
    let (meter, budget) = metered();
    let asset =
        unityrs_testkit::env::serialized(&[], &[unityrs_testkit::env::Obj::named(7, "asset", &[])]);
    let parent = web(&[("CAB-asset", &asset)], &meter);
    let File::Serialized(sf) = parent.entry(0).unwrap() else {
        panic!("SerializedFile")
    };
    exhaust(&meter);
    budget.0.store(0, Ordering::Relaxed);
    work_error(sf.get_writeable_cab_with_guid(None, *b"123-123456789abc"));
    assert_eq!(
        budget.0.load(Ordering::Relaxed),
        0,
        "archive path copying cannot start before work is granted"
    );
}

#[test]
fn cab_and_pending_external_snapshots_pay_work_before_cloning() {
    let (meter, budget) = metered();
    let asset =
        unityrs_testkit::env::serialized(&[], &[unityrs_testkit::env::Obj::named(7, "asset", &[])]);
    let parent = web(&[("CAB-asset", &asset)], &meter);
    let File::Serialized(sf) = parent.entry(0).unwrap() else {
        panic!("SerializedFile")
    };
    sf.get_writeable_cab_with_guid(None, *b"123-123456789abc")
        .unwrap();
    exhaust(&meter);
    budget.0.store(0, Ordering::Relaxed);
    work_error(parent.writeable_cabs());
    work_error(sf.writeable_externals());
    assert_eq!(budget.0.load(Ordering::Relaxed), 0);
}

#[test]
fn invalid_web_cab_utf8_refuses_semantic_save() {
    let meter = Meter::unlimited();
    let web = web(&[], &meter);
    let name = RawStr::from(&b"invalid-\xff.resS"[..]);
    if web.get_writeable_cab(Some(&name)).is_err() {
        return;
    }
    let before = meter.used();
    assert!(
        web.save(WebSaveOptions::default()).is_err(),
        "semantic save must not return an unreadable WebFile"
    );
    assert_eq!(
        meter.used(),
        before,
        "invalid metadata must not retain output storage"
    );
}

#[test]
fn bundle_cab_nul_refuses_semantic_save() {
    let meter = Meter::unlimited();
    let spec = unityrs_testkit::bundle::BundleSpec {
        entries: vec![],
        ..unityrs_testkit::bundle::BundleSpec::default()
    };
    let bundle = BundleFile::open(
        RawStr::from_static(b"cab.bundle"),
        Bytes::from(unityrs_testkit::bundle::build(&spec)),
        None,
        &meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    let name = RawStr::from("invalid\0name.resS");
    if bundle.get_writeable_cab(Some(&name)).is_err() {
        return;
    }
    assert!(
        bundle
            .save(BundleSaveOptions {
                layout: SaveLayout::Semantic,
                ..BundleSaveOptions::default()
            })
            .is_err(),
        "semantic cstr names must not contain NUL"
    );
}

#[test]
fn web_cab_embedded_nul_name_round_trips_as_length_prefixed_utf8() {
    let meter = Meter::unlimited();
    let parent = web(&[], &meter);
    let name = RawStr::from("valid\0name.resS");
    let cab = parent.get_writeable_cab(Some(&name)).unwrap();
    cab.write_bytes(b"nul-name-payload").unwrap();
    let saved = parent.save(WebSaveOptions::default()).unwrap();
    let back = WebFile::open(
        RawStr::from_static(b"saved.web"),
        saved,
        None,
        &meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    assert_eq!(back.files()[0].name, name);
    assert_eq!(back.entry_bytes(0).unwrap().as_ref(), b"nul-name-payload");
}

#[test]
fn default_guid_has_uuid_urn_suffix_shape_and_a_stable_node() {
    let meter = Meter::unlimited();
    let asset =
        unityrs_testkit::env::serialized(&[], &[unityrs_testkit::env::Obj::named(7, "asset", &[])]);
    let parent = web(&[("CAB-asset", &asset)], &meter);
    let File::Serialized(sf) = parent.entry(0).unwrap() else {
        panic!("SerializedFile")
    };
    for name in ["first.resS", "second.resS"] {
        sf.get_writeable_cab(Some(&RawStr::from(name)))
            .unwrap()
            .unwrap();
    }
    let pending = sf.writeable_externals().unwrap();
    assert_eq!(pending.len(), 2);
    let mut node = None;
    for external in pending.as_slice() {
        let guid: &[u8] = external.guid.as_ref().unwrap();
        assert_eq!(guid.len(), 16);
        assert_eq!(guid[3], b'-');
        assert!(
            guid.iter()
                .enumerate()
                .all(|(i, b)| i == 3 || b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        );
        if let Some(node) = node {
            assert_eq!(
                &guid[4..],
                node,
                "default UUID URN suffixes share their node"
            );
        } else {
            node = Some(&guid[4..]);
        }
    }
}

#[test]
fn memory_refusal_keeps_cab_creation_and_appends_retryable() {
    let (meter, _) = metered();
    let parent = web(&[], &meter);
    let held = meter.charge(meter.limit() - meter.used()).unwrap();
    assert!(matches!(
        parent.get_writeable_cab(None),
        Err(SaveError::Charge(_))
    ));
    assert!(parent.writeable_cabs().unwrap().is_empty());
    drop(held);
    let cab = parent.get_writeable_cab(None).unwrap();
    let held = meter.charge(meter.limit() - meter.used()).unwrap();
    assert!(matches!(
        cab.write_bytes(b"retry"),
        Err(SaveError::Charge(_))
    ));
    drop(held);
    assert_eq!(cab.write_bytes(b"retry").unwrap(), 0);
    assert_eq!(cab.save().unwrap().as_ref(), b"retry");
    let before = meter.used();
    let snapshot = parent.writeable_cabs().unwrap();
    assert!(meter.used() > before, "snapshot storage owns a charge");
    drop(snapshot);
    assert_eq!(meter.used(), before);
}

#[test]
fn failed_serialized_creation_adds_no_external_then_retry_deduplicates() {
    let (meter, _) = metered();
    let asset =
        unityrs_testkit::env::serialized(&[], &[unityrs_testkit::env::Obj::named(7, "asset", &[])]);
    let parent = web(&[("CAB-asset", &asset)], &meter);
    let File::Serialized(sf) = parent.entry(0).unwrap() else {
        panic!("SerializedFile")
    };
    let held = meter.charge(meter.limit() - meter.used()).unwrap();
    assert!(
        sf.get_writeable_cab_with_guid(None, *b"123-123456789abc")
            .is_err()
    );
    drop(held);
    assert!(sf.writeable_externals().unwrap().is_empty());
    let cab = sf
        .get_writeable_cab_with_guid(None, *b"123-123456789abc")
        .unwrap()
        .unwrap();
    cab.write_bytes(b"resource").unwrap();
    sf.get_writeable_cab_with_guid(None, *b"abc-fedcba987654")
        .unwrap();
    let pending = sf.writeable_externals().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending.as_slice()[0].path.as_bytes(),
        b"archive:/CAB-asset/CAB-UnityPy_Mod.resS"
    );
    assert_eq!(
        pending.as_slice()[0].guid.as_ref().unwrap().as_ref(),
        b"123-123456789abc"
    );
}
