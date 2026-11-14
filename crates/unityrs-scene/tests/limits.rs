use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation, Unbounded};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::File;
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::{Bytes, FileContext, LimitKind, RawStr};
use unityrs_scene::{GraphOptions, Issue, SceneError, graph_from_game_object};
use unityrs_testkit::ngui::{Object, V};
use unityrs_testkit::scene3d::{self as s, Layout};

fn objects(n: i64) -> Vec<Object> {
    let mut out = vec![];
    for i in 1..=n {
        out.push(Object::new(
            i,
            1,
            s::game_object(b"bounded", &[(0, 1000 + i)]),
        ));
        let children = if i == n { vec![] } else { vec![(0, 1001 + i)] };
        out.push(Object::new(
            1000 + i,
            4,
            s::transform(
                (0, i),
                (0, if i == 1 { 0 } else { 999 + i }),
                &children,
                false,
            ),
        ));
    }
    out
}
fn fixture(depth: u32, n: i64) -> (Environment, unityrs_core::ObjectRef) {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_tree_depth = depth;
    let mut opts = EnvOptions::unitypy();
    opts.context = Arc::new(ctx);
    let env = Environment::new(opts);
    let File::Serialized(f) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"limits"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], &objects(n)).unwrap()),
        })
        .unwrap()
    else {
        panic!("file")
    };
    (env, f.object(1).unwrap())
}

#[derive(Debug)]
struct RefuseAfter {
    calls: AtomicUsize,
    at: usize,
}
impl MemoryBudget for RefuseAfter {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if self.calls.fetch_add(1, Ordering::Relaxed) >= self.at {
            Err(BudgetExceeded {
                requested: bytes,
                what,
            })
        } else {
            Ok(Permit::detached(bytes))
        }
    }
}

#[test]
fn every_graph_reservation_can_be_refused_and_returns_its_charges() {
    let (env, root) = fixture(256, 8);
    let opts = GraphOptions::default();
    let budget = Arc::new(RefuseAfter {
        calls: AtomicUsize::new(0),
        at: usize::MAX,
    });
    let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    let graph = graph_from_game_object(&env, &root, &opts, &meter).unwrap();
    assert!(meter.used() > 0);
    let calls = budget.calls.load(Ordering::Relaxed);
    assert!(calls > 20, "only {calls} reservations exercised");
    drop(graph);
    assert_eq!(meter.used(), 0);
    for at in 0..calls {
        let b = Arc::new(RefuseAfter {
            calls: AtomicUsize::new(0),
            at,
        });
        let m = Meter::new(u64::MAX, b, Reservation::Dependency);
        let error = graph_from_game_object(&env, &root, &opts, &m).unwrap_err();
        assert!(
            matches!(error, SceneError::Charge(ChargeError::Budget(ref b)) if b.what == Reservation::Dependency),
            "at {at}: {error:?}"
        );
        drop(error);
        assert_eq!(m.used(), 0, "at {at}");
    }
}

#[test]
fn input_memory_and_work_refusals_are_errors_and_release_partial_graphs() {
    let (env, root) = fixture(256, 8);
    let opts = GraphOptions::default();
    let m = Meter::unlimited();
    drop(graph_from_game_object(&env, &root, &opts, &m).unwrap());
    let work = m.decoded();
    assert!(work > 8);
    for limit in [0, 1, 16, work / 2, work - 1, work, work + 1, work * 2] {
        let m = Meter::with_decode_work(u64::MAX, limit, Arc::new(Unbounded), Reservation::Input);
        let result = graph_from_game_object(&env, &root, &opts, &m);
        // Randomised hash probing counts its actual work, which can vary
        // by a few steps. Bounds far below/above the full walk pin refusal
        // and success; every refused near-boundary walk must name work.
        if limit < work / 2 {
            assert!(result.is_err(), "limit {limit}");
        }
        if limit == work * 2 {
            assert!(result.is_ok());
        }
        if let Err(e) = &result {
            assert!(
                matches!(e, SceneError::Charge(ChargeError::Limit(l)) if l.kind() == LimitKind::DecodeWork),
                "{e:?}"
            );
        }
        drop(result);
        assert_eq!(m.used(), 0);
    }
    for limit in [0, 1, 32, 128, 1024, 4096] {
        let m = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        let result = graph_from_game_object(&env, &root, &opts, &m);
        assert!(
            matches!(result, Err(SceneError::Charge(ChargeError::Limit(ref l))) if l.kind() == LimitKind::InputMemory)
        );
        drop(result);
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn caller_cannot_loosen_file_depth_and_zero_refuses_the_root() {
    let (env, root) = fixture(12, 40);
    let opts = GraphOptions {
        max_depth: Some(u32::MAX),
    };
    let g = graph_from_game_object(&env, &root, &opts, &Meter::unlimited()).unwrap();
    assert_eq!(g.nodes().len(), 12);
    assert!(matches!(
        g.issues(),
        [Issue::Depth {
            depth: 13,
            limit: 12,
            ..
        }]
    ));
    assert!(
        graph_from_game_object(
            &env,
            &root,
            &GraphOptions { max_depth: Some(0) },
            &Meter::unlimited()
        )
        .is_err()
    );
}

#[test]
fn malformed_descendant_and_component_keep_the_remaining_graph() {
    let mut objects = objects(3);
    objects[3].value = V::Struct("Transform".into(), vec![]);
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(f) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"malformed"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], &objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("file")
    };
    let g = graph_from_game_object(
        &env,
        &f.object(1).unwrap(),
        &GraphOptions::default(),
        &Meter::unlimited(),
    )
    .unwrap();
    assert_eq!(g.nodes().len(), 1);
    assert!(matches!(g.issues(), [Issue::Malformed { .. }]));
}

#[derive(Debug)]
struct RefusingProvider(unityrs_core::provider::ProviderError);
impl unityrs_core::provider::TypeTreeProvider for RefusingProvider {
    fn name(&self) -> &'static str {
        "refusal"
    }
    fn tree(
        &self,
        _: &unityrs_core::provider::TreeRequest<'_>,
    ) -> Result<Option<Arc<unityrs_core::typetree::Tree>>, unityrs_core::provider::ProviderError>
    {
        Err(self.0.clone())
    }
}

#[test]
fn provider_memory_and_work_refusals_fail_instead_of_becoming_component_issues() {
    use unityrs_core::provider::{ProviderChain, ProviderError};
    for memory in [false, true] {
        let mut refusal = ProviderError::new("refusal", Arc::from("synthetic refusal"));
        refusal.limit =
            (!memory).then(|| unityrs_core::LimitError::refused(LimitKind::DecodeWork, 2, 1));
        refusal.memory = memory.then_some(BudgetExceeded {
            requested: 128,
            what: Reservation::Provider,
        });
        let mut ctx = FileContext::unitypy();
        ctx.providers = Arc::new(ProviderChain::new(vec![Arc::new(RefusingProvider(
            refusal,
        ))]));
        let mut opts = EnvOptions::unitypy();
        opts.context = Arc::new(ctx);
        let env = Environment::new(opts);
        env.load(Source::Bytes {
            name: RawStr::from_static(b"stripped"),
            data: Bytes::from(
                unityrs_testkit::ngui::file(
                    &[],
                    &[Object::new(2, 23, s::mesh_renderer((0, 1), &[], (0, 0)))],
                    false,
                )
                .unwrap(),
            ),
        })
        .unwrap();
        let File::Serialized(f) = env
            .load(Source::Bytes {
                name: RawStr::from_static(b"root"),
                data: Bytes::from(
                    s::file(
                        Layout::Unity2022,
                        &["stripped"],
                        &[
                            Object::new(1, 1, s::game_object(b"root", &[(0, 10), (1, 2)])),
                            Object::new(10, 4, s::transform((0, 1), (0, 0), &[], false)),
                        ],
                    )
                    .unwrap(),
                ),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let meter = Meter::unlimited();
        let result = graph_from_game_object(
            &env,
            &f.object(1).unwrap(),
            &GraphOptions::default(),
            &meter,
        );
        assert!(
            matches!(&result, Err(SceneError::Parse(unityrs_core::walk::ParseError::Provider(p))) if p.memory.is_some() == memory),
            "{result:?}"
        );
        drop(result);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn gzip_dependency_inflate_refusal_is_fatal_and_releases_the_partial_graph() {
    use unityrs_core::Error;
    use unityrs_core::detect::DetectError;
    use unityrs_core::env::{CabResolver, DerefError};

    #[derive(Debug)]
    struct GzipDependency(Bytes);
    impl CabResolver for GzipDependency {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            (name == b"dependency").then(|| Source::Bytes {
                name: RawStr::from_static(b"dependency"),
                data: self.0.clone(),
            })
        }
    }
    // A valid gzip member containing 256 zero bytes, padded with allowed
    // trailing zeros to reach detection's 128-byte minimum.
    let mut gzip = vec![
        31, 139, 8, 0, 0, 0, 0, 0, 0, 255, 99, 96, 24, 217, 0, 0, 88, 133, 150, 13, 0, 1, 0, 0,
    ];
    gzip.resize(128, 0);
    for max_inflate in [128, 256] {
        let mut context = FileContext::unitypy();
        context.limits.max_inflate = max_inflate;
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(context);
        options.resolver = Some(Arc::new(GzipDependency(Bytes::copy_from_slice(&gzip))));
        let env = Environment::new(options);
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from_static(b"root"),
                data: Bytes::from(
                    s::file(
                        Layout::Unity2022,
                        &["dependency"],
                        &[
                            Object::new(1, 1, s::game_object(b"root", &[(0, 10), (0, 20)])),
                            Object::new(10, 4, s::transform((0, 1), (0, 0), &[], false)),
                            Object::new(20, 33, s::mesh_filter((0, 1), (1, 9))),
                        ],
                    )
                    .unwrap(),
                ),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let meter = Meter::unlimited();
        let result = graph_from_game_object(
            &env,
            &file.object(1).unwrap(),
            &GraphOptions::default(),
            &meter,
        );
        if max_inflate == 128 {
            assert!(
                matches!(&result,
                    Err(SceneError::Deref(DerefError::Load(Error::Detect(DetectError::InflateLimit(l)))))
                    if l.kind() == LimitKind::Inflate && l.value() == 256 && l.max() == 128
                ),
                "{result:?}"
            );
        } else {
            // Detection succeeds at the exact bound; a raw dependency is
            // an ordinary unresolved pointer, which keeps the graph.
            let graph = result.as_ref().unwrap();
            assert_eq!(graph.nodes().len(), 1);
            assert!(matches!(
                graph.issues(),
                [Issue::Unresolved {
                    reason: DerefError::RawTarget { .. },
                    ..
                }]
            ));
        }
        drop(result);
        assert_eq!(meter.used(), 0);
    }
}
