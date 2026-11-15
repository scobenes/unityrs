//! Shared inline/streamed texture data, independently of either exporter.

use std::cell::Cell;
use std::sync::Arc;

use unityrs_core::budget::{Counting, Reservation, Unbounded};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::{BundleFile, File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext};
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{file, mono_script};
use unityrs_texture::fetch::{self, Error, Hooks, Span, Stream, python_slice};

fn opened(rows: &[(&str, Vec<u8>)]) -> (Arc<BundleFile>, Arc<SerializedFile>, Arc<Meter>) {
    let meter = Meter::unlimited();
    let root = BundleFile::open(
        "test.bundle".into(),
        Bytes::from(bundle(rows)),
        None,
        &meter,
        &Arc::new(FileContext::default()),
    )
    .unwrap();
    let File::Serialized(from) = root.entry(0).unwrap() else {
        panic!("first row must be serialized");
    };
    (root, from, meter)
}

fn stream(path: &str, offset: i128, size: i128) -> Stream {
    Stream {
        path: path.into(),
        offset,
        size,
    }
}

#[test]
fn inline_and_missing_stream_are_zero_copy_and_never_resolve() {
    let (_root, from, meter) = opened(&[(
        "CAB-test",
        file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
    )]);
    let env = Environment::new(EnvOptions::default());
    let inline = Bytes::from_static(b"inline");
    let missing = stream("missing.resS", 0, 10);
    let got = fetch::fetch(&env, &from, &inline, Some(&missing), &meter).unwrap();
    assert_eq!(got.as_ptr(), inline.as_ptr());
    assert!(
        fetch::fetch(&env, &from, &Bytes::new(), None, &meter)
            .unwrap()
            .is_empty()
    );
    assert!(
        fetch::fetch(&env, &from, &Bytes::new(), Some(&stream("", 0, 10)), &meter)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn own_rows_beat_registry_shadows_candidates_are_ordered_and_bundles_pass() {
    let (_root, from, meter) = opened(&[
        (
            "CAB-test",
            file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
        ),
        ("sheet.resource", b"second candidate".to_vec()),
        ("sheet.resS", b"fourth candidate".to_vec()),
        ("same.resS", b"first row".to_vec()),
        ("SAME.resS", b"middle row".to_vec()),
        ("same.resS", b"last row".to_vec()),
        (
            "same.resS",
            bundle(&[("same.resS", b"nested shadow".to_vec())]),
        ),
    ]);
    let env = Environment::new(EnvOptions::default());
    env.load(Source::Bytes {
        name: "same.resS".into(),
        data: Bytes::from_static(b"registry shadow"),
    })
    .unwrap();
    let get = |path| {
        fetch::fetch(
            &env,
            &from,
            &Bytes::new(),
            Some(&stream(path, 0, 100)),
            &meter,
        )
        .unwrap()
    };
    assert_eq!(get("archive:/sheet.dat"), b"second candidate"[..]);
    assert_eq!(get("archive:/same.resS"), b"last row"[..]);
}

#[test]
fn streamed_spans_keep_signed_and_wide_python_read_rules() {
    let (_root, from, meter) = opened(&[
        (
            "CAB-test",
            file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
        ),
        ("r.resS", b"01234567".to_vec()),
    ]);
    let env = Environment::new(EnvOptions::default());
    for (offset, size, want) in [
        (2, 3, "234"),
        (6, 9, "67"),
        (-4, -1, "4567"),
        (0, -2, "012345"),
        (-4, 0, ""),
        (2, i128::MAX, "234567"),
        (i128::MIN, 3, ""),
        (i128::MAX, 1, ""),
    ] {
        let got = fetch::fetch(
            &env,
            &from,
            &Bytes::new(),
            Some(&stream("r.resS", offset, size)),
            &meter,
        )
        .unwrap();
        assert_eq!(got.as_ref(), want.as_bytes(), "{offset}, {size}");
    }
    assert_eq!(stream("r", -4, -1).span(), Span::Slice(-4, i128::MAX));
    assert_eq!(stream("r", 5, 0).span(), Span::Range(0, 0));
    assert_eq!(python_slice(8, 6, 2), 6..6);
}

#[test]
fn fallback_resources_keep_resource_failures_distinct_from_own_data() {
    let (_root, from, meter) = opened(&[(
        "CAB-test",
        file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
    )]);
    let env = Environment::new(EnvOptions::default());
    env.load(Source::Bytes {
        name: "dep.resS".into(),
        data: Bytes::from_static(b"01234567"),
    })
    .unwrap();
    let got = fetch::fetch(
        &env,
        &from,
        &Bytes::new(),
        Some(&stream("dep.resS", -3, -1)),
        &meter,
    )
    .unwrap();
    assert_eq!(got, b"567"[..]);
    assert!(matches!(
        fetch::fetch(
            &env,
            &from,
            &Bytes::new(),
            Some(&stream("absent", 0, 1)),
            &meter
        ),
        Err(Error::Resource { .. })
    ));
    assert!(matches!(
        fetch::fetch(
            &env,
            &from,
            &Bytes::new(),
            Some(&stream("CAB-test", 0, 1)),
            &meter
        ),
        Err(Error::Serialized)
    ));
}

#[test]
fn inner_container_wins_and_outer_candidates_precede_inner_later_candidates() {
    let meter = Meter::unlimited();
    let root = BundleFile::open(
        "test.bundle".into(),
        Bytes::from(bundle(&[
            ("r.resS", b"outer".to_vec()),
            ("s.dat", b"outer first candidate".to_vec()),
            (
                "nested",
                bundle(&[
                    (
                        "CAB-test",
                        file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
                    ),
                    ("r.resS", b"inner".to_vec()),
                    ("s.resource", b"inner second candidate".to_vec()),
                ]),
            ),
        ])),
        None,
        &meter,
        &Arc::new(FileContext::default()),
    )
    .unwrap();
    let File::Bundle(inner) = root.entry(2).unwrap() else {
        panic!("nested bundle");
    };
    let File::Serialized(from) = inner.entry(0).unwrap() else {
        panic!("serialized row");
    };
    let env = Environment::new(EnvOptions::default());
    let get = |path| {
        fetch::fetch(
            &env,
            &from,
            &Bytes::new(),
            Some(&stream(path, 0, 100)),
            &meter,
        )
        .unwrap()
    };
    assert_eq!(get("r.resS"), b"inner"[..]);
    assert_eq!(get("s.dat"), b"outer first candidate"[..]);
}

#[derive(Default)]
struct Observed {
    probes: Cell<usize>,
    environment: Cell<usize>,
}

impl Hooks for Observed {
    type Error = Error;
    fn probe<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.probes.set(self.probes.get() + 1);
        f()
    }
    fn env_call<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.environment.set(self.environment.get() + 1);
        f()
    }
    fn row_error(&self, error: Error) -> Error {
        error
    }
}

#[test]
fn hooks_observe_the_environment_only_after_the_own_chain_misses() {
    let (_root, from, meter) = opened(&[
        (
            "CAB-test",
            file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
        ),
        ("r.resS", b"own".to_vec()),
    ]);
    let env = Environment::new(EnvOptions::default());
    let hooks = Observed::default();
    let got = fetch::fetch_with(
        &env,
        &from,
        &Bytes::new(),
        Some(&stream("r.resS", 0, 100)),
        &meter,
        &hooks,
    )
    .unwrap();
    assert_eq!(got, b"own"[..]);
    assert_eq!(hooks.environment.get(), 0);
    assert!(
        hooks.probes.get() >= 4,
        "join, simplify, entry, range each probed"
    );
    assert!(
        fetch::fetch_with(
            &env,
            &from,
            &Bytes::new(),
            Some(&stream("missing", 0, 100)),
            &meter,
            &hooks
        )
        .is_err()
    );
    assert_eq!(hooks.environment.get(), 1);
}

#[test]
fn lookup_work_and_name_storage_are_refused_before_any_resource_call() {
    let (_root, from, _meter) = opened(&[(
        "CAB-test",
        file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
    )]);
    let env = Environment::new(EnvOptions::default());
    let work = Meter::with_decode_work(u64::MAX, 0, Arc::new(Unbounded), Reservation::Input);
    let memory = Meter::new(u64::MAX, Arc::new(Counting::new(0)), Reservation::Input);
    for meter in [work, memory] {
        let hooks = Observed::default();
        let got = fetch::fetch_with(
            &env,
            &from,
            &Bytes::new(),
            Some(&stream("UPPER.resS", 0, 1)),
            &meter,
            &hooks,
        );
        assert!(matches!(got, Err(Error::Charge(_))));
        assert_eq!(hooks.environment.get(), 0);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn corrupt_own_rows_are_data_failures_and_do_not_fall_through() {
    let (_root, from, meter) = opened(&[
        (
            "CAB-test",
            file(&[], &[mono_script(1, "Synthetic")], true).unwrap(),
        ),
        ("r.resS", b"UnityFS\0broken bundle header".to_vec()),
    ]);
    let env = Environment::new(EnvOptions::default());
    let hooks = Observed::default();
    let result = fetch::fetch_with(
        &env,
        &from,
        &Bytes::new(),
        Some(&stream("r.resS", 0, 1)),
        &meter,
        &hooks,
    );
    assert!(matches!(result, Err(Error::Data(_))));
    assert_eq!(hooks.environment.get(), 0);
}
