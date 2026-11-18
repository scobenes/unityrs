use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use unityrs_anim::discover;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::env::{EnvOptions, Environment, Fetch, Source};
use unityrs_core::error::Error;
use unityrs_core::files::{BundleFile, File};
use unityrs_core::limits::Meter;
use unityrs_core::provider::{ProviderChain, ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::Tree;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_scene::{GraphOptions, graph_from_scene_file};
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{Object, pptr, st, vector};
use unityrs_testkit::scene3d::{self as s, Layout, anim as a};

use super::load;

#[derive(Debug, Default)]
struct Usage(AtomicU64);
impl PermitHome for Usage {
    fn release(&self, bytes: u64) {
        self.0.fetch_sub(bytes, Ordering::Relaxed);
    }
}
#[derive(Debug)]
struct Pool {
    usage: Arc<Usage>,
    limit: u64,
}
impl Pool {
    fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            usage: Arc::default(),
            limit,
        })
    }
}
impl MemoryBudget for Pool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what != Reservation::Dependency {
            return Ok(Permit::detached(bytes));
        }
        self.usage
            .0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|&total| total <= self.limit)
            })
            .map_err(|_| BudgetExceeded {
                requested: bytes,
                what,
            })?;
        Ok(Permit::new(bytes, self.usage.clone()))
    }
}

#[test]
fn alternating_dependency_slots_share_two_retained_clips() {
    #[derive(Debug)]
    struct CountedBundle {
        data: Bytes,
        fetches: AtomicU64,
        blocked_by: Option<Arc<OnceLock<Weak<BundleFile>>>>,
    }
    impl Fetch for CountedBundle {
        fn fetch(&self) -> Result<Bytes, Error> {
            if self
                .blocked_by
                .as_ref()
                .and_then(|pin| pin.get())
                .is_some_and(|pin| pin.strong_count() > 0)
            {
                return Err(BudgetExceeded {
                    requested: self.data.len() as u64,
                    what: Reservation::Dependency,
                }
                .into());
            }
            self.fetches.fetch_add(1, Ordering::Relaxed);
            Ok(self.data.clone())
        }
    }

    let mut context = FileContext::unitypy();
    context.budget = Pool::new(512 * 1024);
    let mut options = EnvOptions::batch(Arc::new(context));
    let filler: Vec<_> = (0..96).map(|i| a::clip(100 + i)).collect();
    let clip_file = s::file(Layout::Unity2022, &[], &filler).unwrap();
    let calibration = Environment::new(options.clone());
    drop(
        calibration
            .load_dependency(Source::Bytes {
                name: RawStr::from_static(b"clips-a.bundle"),
                data: Bytes::from(bundle(&[("clips-a", clip_file.clone())])),
            })
            .unwrap(),
    );
    let meter = Meter::unlimited();
    calibration
        .get_cab(&RawStr::from_static(b"clips-a"), &meter)
        .unwrap()
        .unwrap();
    options.dep_cache_bytes = calibration.stats().dependency_bytes;
    drop(calibration);
    let env = Environment::new(options);
    let pin = Arc::new(OnceLock::new());
    let mut bundles = Vec::new();
    for (name, bundle_name) in [
        ("clips-b", b"clips-b.bundle".as_slice()),
        ("clips-a", b"clips-a.bundle".as_slice()),
    ] {
        let fetch = Arc::new(CountedBundle {
            data: Bytes::from(bundle(&[(name, clip_file.clone())])),
            fetches: AtomicU64::new(0),
            blocked_by: (name == "clips-b").then(|| pin.clone()),
        });
        let opened = env
            .load_dependency(Source::Fetch {
                name: RawStr::from_static(bundle_name),
                fetch: fetch.clone(),
            })
            .unwrap();
        if name == "clips-a" {
            let File::Bundle(opened) = opened else {
                panic!("clip bundle");
            };
            pin.set(Arc::downgrade(&opened)).unwrap();
        }
        bundles.push(fetch);
    }
    let slots = [(1, 100), (2, 100), (3, 100)];
    let file = load(
        &env,
        b"rig",
        &["clips-a", "clips-b", "archive:/CLIPS-A/CLIPS-A"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::legacy(3, (0, 0), &slots),
        ],
    );
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let before = env.stats().dependencies_evicted;
    let catalog = discover(&env, &graph, &meter).unwrap();
    assert!(env.stats().dependencies_evicted > before);
    let rig = &catalog.rigs()[0];
    assert_eq!(rig.clips().count(), 2);
    let slots = &rig.slots()[1..];
    assert_eq!(slots.len(), 3);
    let first = slots[0].clip().unwrap();
    let second = slots[1].clip().unwrap();
    assert_eq!(first.file().name().as_bytes(), b"clips-a");
    assert_eq!(second.file().name().as_bytes(), b"clips-b");
    for (slot, expected) in slots.iter().zip([first, second].into_iter().cycle()) {
        assert!(std::ptr::eq(slot.clip().unwrap(), expected));
    }
    // B reopens after evicting A. The last A slot reuses the first parsed A.
    assert_eq!(pin.get().unwrap().strong_count(), 0);
    assert_eq!(bundles[0].fetches.load(Ordering::Relaxed), 2);
    assert_eq!(bundles[1].fetches.load(Ordering::Relaxed), 1);
}

#[test]
fn eager_asset_bundle_budget_refusal_retries_after_releasing_the_pinned_bundle() {
    #[derive(Debug, Default)]
    struct RefuseWhilePinned {
        pinned: OnceLock<Weak<BundleFile>>,
        refusals: AtomicU64,
    }
    impl MemoryBudget for RefuseWhilePinned {
        fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
            // The eager object read allocates a value array larger than the
            // serialized bundle. Let all smaller charges through.
            if what == Reservation::Dependency
                && bytes >= 128 * 1024
                && self.pinned.get().is_some_and(|pin| pin.strong_count() > 0)
            {
                self.refusals.fetch_add(1, Ordering::Relaxed);
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
            Ok(Permit::detached(bytes))
        }
    }

    let budget = Arc::new(RefuseWhilePinned::default());
    let mut context = FileContext::unitypy();
    context.budget = budget.clone();
    let mut options = EnvOptions::batch(Arc::new(context));
    options.dep_cache_bytes = 1;
    let env = Environment::new(options);
    let null = pptr("Object", (0, 0));
    let asset_bundle = Object::new(
        11,
        142,
        st(
            "AssetBundle",
            vec![
                ("m_PreloadTable", vector(null.clone(), vec![])),
                ("m_Container", vector(null.clone(), vec![])),
                ("m_Junk", vector(null.clone(), vec![null; 8192])),
            ],
        ),
    );
    for (name, objects) in [
        ("clips-b", vec![a::clip(10), asset_bundle]),
        ("clips-a", vec![a::clip(9)]),
    ] {
        let clips = s::file(Layout::Unity2022, &[], &objects).unwrap();
        let opened = env
            .load_dependency(Source::Bytes {
                name: RawStr::from(format!("{name}.bundle").into_bytes()),
                data: Bytes::from(bundle(&[(name, clips)])),
            })
            .unwrap();
        if name == "clips-a" {
            let File::Bundle(opened) = opened else {
                panic!("clip bundle");
            };
            budget.pinned.set(Arc::downgrade(&opened)).unwrap();
        }
    }
    let file = load(
        &env,
        b"rig",
        &["clips-a", "clips-b"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::legacy(3, (1, 9), &[(2, 10)]),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let [first, second] = catalog.rigs()[0].slots() else {
        panic!("two clip slots");
    };
    assert!(budget.refusals.load(Ordering::Relaxed) > 0);
    assert_eq!(budget.pinned.get().unwrap().strong_count(), 0);
    assert_eq!(first.clip().unwrap().path_id(), 9);
    assert_eq!(second.clip().unwrap().path_id(), 10);
}

#[test]
fn stripped_asset_bundle_provider_refusal_retries_after_releasing_the_pinned_bundle() {
    #[derive(Debug)]
    struct AssetBundleProvider {
        tree: Arc<Tree>,
        pinned: OnceLock<Weak<BundleFile>>,
        requests: AtomicU64,
    }
    impl TypeTreeProvider for AssetBundleProvider {
        fn name(&self) -> &'static str {
            "asset-bundle-fixture"
        }

        fn tree(&self, request: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
            assert_eq!(request.class_id, 142);
            self.requests.fetch_add(1, Ordering::Relaxed);
            if self.pinned.get().unwrap().strong_count() > 0 {
                let mut error = ProviderError::new(self.name(), "dependency pool full".into());
                error.memory = Some(BudgetExceeded {
                    requested: 1,
                    what: Reservation::Dependency,
                });
                return Err(error);
            }
            Ok(Some(self.tree.clone()))
        }
    }

    let null = pptr("Object", (0, 0));
    let objects = [
        a::clip(10),
        Object::new(
            11,
            142,
            st(
                "AssetBundle",
                vec![
                    ("m_PreloadTable", vector(null.clone(), vec![])),
                    ("m_Container", vector(null, vec![])),
                ],
            ),
        ),
    ];
    let typed_env = Environment::new(EnvOptions::unitypy());
    let typed = load(&typed_env, b"typed", &[], &objects);
    let tree = typed.object(11).unwrap().tree(None).unwrap().unwrap().tree;
    let provider = Arc::new(AssetBundleProvider {
        tree,
        pinned: OnceLock::new(),
        requests: AtomicU64::new(0),
    });
    let mut context = FileContext::unitypy();
    context.providers = Arc::new(ProviderChain::new(vec![provider.clone()]));
    let mut options = EnvOptions::batch(Arc::new(context));
    options.dep_cache_bytes = 1;
    let env = Environment::new(options);
    let stripped = unityrs_testkit::ngui::file(&[], &objects, false).unwrap();
    drop(
        env.load_dependency(Source::Bytes {
            name: RawStr::from_static(b"clips-b.bundle"),
            data: Bytes::from(bundle(&[("clips-b", stripped)])),
        })
        .unwrap(),
    );
    let clips = s::file(Layout::Unity2022, &[], &[a::clip(9)]).unwrap();
    let File::Bundle(pinned) = env
        .load_dependency(Source::Bytes {
            name: RawStr::from_static(b"clips-a.bundle"),
            data: Bytes::from(bundle(&[("clips-a", clips)])),
        })
        .unwrap()
    else {
        panic!("clip bundle");
    };
    provider.pinned.set(Arc::downgrade(&pinned)).unwrap();
    drop(pinned);

    let file = load(
        &env,
        b"rig",
        &["clips-a", "clips-b"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::legacy(3, (1, 9), &[(2, 10)]),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let [first, second] = catalog.rigs()[0].slots() else {
        panic!("two clip slots");
    };
    // The first lookup sees the pin. Discovery releases it and retries once.
    assert_eq!(provider.requests.load(Ordering::Relaxed), 2);
    assert_eq!(provider.pinned.get().unwrap().strong_count(), 0);
    assert_eq!(first.clip().unwrap().path_id(), 9);
    assert_eq!(second.clip().unwrap().path_id(), 10);
}

#[test]
fn unused_override_rows_leave_room_for_the_selected_clip() {
    let discover_with_cache = |cache_bytes, pool_bytes| {
        let mut context = FileContext::unitypy();
        context.budget = Pool::new(pool_bytes);
        let mut options = EnvOptions::batch(Arc::new(context));
        options.dep_cache_bytes = cache_bytes;
        let env = Environment::new(options);
        let names: Vec<_> = (0..24).map(|i| format!("clips-{i}")).collect();
        let externals: Vec<_> = names.iter().map(String::as_str).collect();
        let filler: Vec<_> = (0..96).map(|i| a::clip(100 + i)).collect();
        for name in &externals {
            let clips = s::file(Layout::Unity2022, &[], &filler).unwrap();
            drop(
                env.load_dependency(Source::Bytes {
                    name: RawStr::from(format!("{name}.bundle").into_bytes()),
                    data: Bytes::from(bundle(&[(name, clips)])),
                })
                .unwrap(),
            );
        }
        let mut rows = vec![((1, 100), (1, 101))];
        // The unused rows have the same path ID as the selected original.
        rows.extend((2..=24).map(|file_id| ((file_id, 100), (file_id, 102))));
        let file = load(
            &env,
            b"rig",
            &externals,
            &[
                Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
                Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
                a::animator(3, (0, 4)),
                a::overrides(4, (0, 5), &rows),
                a::controller(5, &[(1, 100)], &[0]),
            ],
        );
        let meter = Meter::unlimited();
        let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
        let before = env.stats().dependencies_evicted;
        let catalog = discover(&env, &graph, &meter).unwrap();
        if cache_bytes == 1 {
            assert!(env.stats().dependencies_evicted > before);
        }
        let rig = &catalog.rigs()[0];
        assert!(rig.issues().is_empty(), "{:?}", rig.issues());
        let [slot] = rig.slots() else {
            panic!("one clip slot")
        };
        let clip = slot.clip().unwrap();
        assert_eq!(clip.file().name().as_bytes(), b"clips-0");
        assert_eq!(clip.path_id(), 101);
        assert_eq!(slot.replacement.as_ref().unwrap().ordinal, 0);
    };
    discover_with_cache(u64::MAX, u64::MAX);
    discover_with_cache(1, 512 * 1024);
}
