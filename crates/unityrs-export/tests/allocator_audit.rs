//! Direct provenance and absorbed refusals must stop before another read.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use unityrs_core::{
    Bytes, Error, FileContext, RawStr,
    budget::{AllocError, BudgetExceeded, Counting, DecodeBuf, MemoryBudget, Permit, Reservation},
    codec::{BlockCodec, Codec, Decoded},
    detect::DetectError,
    env::{DerefError, EnvOptions, Environment, Source},
    limits::{ChargeError, Meter},
    typetree::TreeError,
    walk::{ParseError, ReadDetail, ReadError, ReadErrorKind},
};
use unityrs_export::ngui::{self, Admit, Class, Data, Failure, Hooks, Registration, Staged};
use unityrs_testkit::{
    bundle::{BundleSpec, EntrySpec, TestCodec, build},
    env::{Obj, bundle, serialized},
};

#[derive(Debug)]
struct Audit {
    pool: Counting,
    allocator: AtomicU64,
}
impl Audit {
    fn new() -> Self {
        Self {
            pool: Counting::new(u64::MAX),
            allocator: AtomicU64::new(0),
        }
    }
}
impl MemoryBudget for Audit {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        self.pool.try_reserve(bytes, what)
    }
    fn allocator_refused(&self, _: AllocError) {
        self.allocator.fetch_add(1, Ordering::Relaxed);
    }
}
impl Hooks for Audit {
    fn allocator_refusals(&self) -> u64 {
        self.allocator.load(Ordering::Relaxed)
    }
    fn refusals(&self) -> Option<u64> {
        Some(0)
    }
}
fn host(c: Class) {
    assert!(matches!(c, Class::Run(r) if r.reason == "host_memory"));
}

#[test]
fn allocator_provenance_routes_through_every_error_wrapper() {
    for what in [
        Reservation::Input,
        Reservation::Dependency,
        Reservation::Provider,
    ] {
        let a = AllocError {
            requested: 8192,
            what,
        };
        host(ngui::classify::charge(&ChargeError::from(a)));
        host(ngui::classify::tree(&TreeError::from(a)));
        host(ngui::classify::parse(&ParseError::from(a)));
        host(ngui::classify::parse(&ParseError::Read(ReadError {
            kind: ReadErrorKind::Other,
            at: 0,
            detail: ReadDetail::from(a),
        })));
        host(ngui::classify::deref(&DerefError::from(a)));
        for error in [
            Error::from(a),
            Error::from(DetectError::from(a)),
            Error::Entry {
                name: RawStr::from_static(b"entry"),
                source: Arc::new(Error::from(a)),
            },
            Error::AssetBundle(Arc::new(ParseError::from(a))),
            Error::Deref(Arc::new(DerefError::from(a))),
        ] {
            host(ngui::classify::error(&error));
            host(ngui::classify::entry_error(
                &error,
                &RawStr::from_static(b"entry"),
            ));
        }
    }
}

/// Native codec refusal stand-in: only the nested bundle's data uses it.
/// Its header and all precommit registration use stored blocks.
#[derive(Debug)]
struct Refuse;
impl BlockCodec for Refuse {
    fn decode(&self, _: &Bytes, _: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
        Err(out.meter().allocator_refused(4096).into())
    }
}
fn scene() -> Bytes {
    let inner = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Raw(42),
        entries: vec![EntrySpec {
            name: b"CAB-N".to_vec(),
            data: serialized(&[], &[Obj::named(1, "n", &[])]),
            flags: 0,
        }],
        ..BundleSpec::default()
    });
    bundle(&[("INNER", inner), ("INNER", vec![0; 50])]).into()
}
fn options(audit: &Arc<Audit>) -> EnvOptions {
    let mut context = FileContext::unitypy();
    context.budget = audit.clone();
    let mut codecs = unityrs_core::codec::Codecs::unitypy();
    codecs.set(Codec::Other(42), Arc::new(Refuse)).unwrap();
    context.codecs = Arc::new(codecs);
    EnvOptions::batch(Arc::new(context))
}

#[test]
fn postcommit_settle_stays_successful_but_registration_discards_it_before_next_read() {
    let data = scene();
    let name = RawStr::from_static(b"nested.bundle");
    let audit = Arc::new(Audit::new());
    let opts = options(&audit);
    let env = Environment::new(opts);
    let file = env
        .load_dependency(Source::Bytes {
            name: name.clone(),
            data: data.clone(),
        })
        .expect("postcommit refusal cannot fail the load");
    assert_eq!(audit.allocator_refusals(), 1, "no pool eviction or retry");
    assert_eq!(env.stats().dependencies_evicted, 0);
    drop(file);
    drop(env);
    assert_eq!(audit.pool.used(), 0);

    // A fresh attempt: its own open is fine; its successful commit absorbs
    // the same failure. The audit must beat loaded(), bits.load(), and reach.
    let audit = Arc::new(Audit::new());
    let opts = options(&audit);
    let meter = Meter::new(u64::MAX, audit.clone(), Reservation::Input);
    let admit = Admit {
        env: &opts,
        meter: &meter,
        hooks: audit.as_ref(),
        max_evictions: u64::MAX,
    };
    let registration = Registration::open(
        Staged {
            id: "x",
            name: &name,
            data: Data::Bytes(&data),
        },
        &[],
        admit,
    )
    .unwrap();
    let mut reads = 0;
    let result = registration.finish(&NoBits, &mut |_, _| {
        reads += 1;
        Ok(())
    });
    assert!(matches!(result, Err(Failure::Run(r)) if r.reason == "host_memory"));
    assert_eq!(
        reads, 0,
        "no callback or subsequent read after settle refusal"
    );
    assert_eq!(audit.allocator_refusals(), 1);
    assert_eq!(audit.pool.used(), 0, "the attempt Environment was dropped");
}
struct NoBits;
impl ngui::PeekBits for NoBits {
    fn load(&self, _: &str, _: &Arc<Meter>, _: &dyn Hooks) -> Result<ngui::Bits, Failure> {
        panic!("no read after allocator refusal")
    }
}

#[test]
fn peek_allocator_failure_stops_before_reading_the_next_entry() {
    #[derive(Debug)]
    struct PeekRefuse {
        calls: Arc<AtomicU64>,
        notify: bool,
    }
    impl BlockCodec for PeekRefuse {
        fn decode(&self, _: &Bytes, _: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let a = if self.notify {
                out.meter().allocator_refused(4096)
            } else {
                AllocError {
                    requested: 4096,
                    what: Reservation::Input,
                }
            };
            Err(a.into())
        }
    }
    for notify in [false, true] {
        let audit = Arc::new(Audit::new());
        let calls = Arc::new(AtomicU64::new(0));
        let mut codecs = unityrs_core::codec::Codecs::unitypy();
        codecs
            .set(
                Codec::Other(42),
                Arc::new(PeekRefuse {
                    calls: calls.clone(),
                    notify,
                }),
            )
            .unwrap();
        let mut context = FileContext::unitypy();
        context.codecs = Arc::new(codecs);
        context.budget = audit.clone();
        let data: Bytes = build(&BundleSpec {
            info_codec: TestCodec::None,
            data_codec: TestCodec::Raw(42),
            block_size: 32,
            entries: [b"first", b"later"]
                .into_iter()
                .map(|name| EntrySpec {
                    name: name.to_vec(),
                    data: vec![0; 32],
                    flags: 0,
                })
                .collect(),
            ..BundleSpec::default()
        })
        .into();
        let meter = Meter::new(u64::MAX, audit.clone(), Reservation::Input);
        let result = ngui::peek(
            &RawStr::from_static(b"peek.bundle"),
            Data::Bytes(&data),
            &Arc::new(context),
            &meter,
            audit.as_ref(),
        );
        assert!(matches!(result, Err(Failure::Run(r)) if r.reason == "host_memory"));
        assert_eq!(calls.load(Ordering::Relaxed), 1, "no later entry read");
        assert_eq!(audit.pool.used(), 0);
    }
}
