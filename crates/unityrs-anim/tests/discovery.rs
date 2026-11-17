use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use unityrs_anim::{AnimError, ClipSelector, SelectionError, discover};
use unityrs_core::budget::{
    BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation, Unbounded,
};
use unityrs_core::env::{CabResolver, DerefError, EnvOptions, Environment, PPtr, Source};
use unityrs_core::error::{Error, LimitKind};
use unityrs_core::files::{File, ParentKind, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_scene::{GraphOptions, graph_from_scene_file};
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{Object, V, set_field};
use unityrs_testkit::scene3d::{self as s, Layout, anim as a};

fn load(
    env: &Environment,
    name: &'static [u8],
    externals: &[&str],
    objects: &[Object],
) -> Arc<SerializedFile> {
    load_layout(env, name, externals, objects, Layout::Unity2022)
}

fn load_layout(
    env: &Environment,
    name: &'static [u8],
    externals: &[&str],
    objects: &[Object],
    layout: Layout,
) -> Arc<SerializedFile> {
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(name),
            data: Bytes::from(s::file(layout, externals, objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("serialized fixture");
    };
    file
}

fn cross_file_rigs(
    env: &Environment,
) -> (
    Arc<SerializedFile>,
    Arc<SerializedFile>,
    Arc<SerializedFile>,
) {
    let clips = load(
        env,
        b"clips",
        &["base"],
        &[
            a::clip(-7),
            a::clip(-8),
            a::clip(-9),
            a::overrides(41, (1, 30), &[((0, -7), (0, -9))]),
        ],
    );
    let base = load(
        env,
        b"base",
        &["clips", "outside"],
        &[
            a::controller(
                30,
                &[(0, -7), (1, -7), (1, -8), (1, -8), (2, -7), (1, -9)],
                &[5, 0, 9, u32::MAX],
            ),
            a::clip(-7),
        ],
    );
    let rig = load(
        env,
        b"rig",
        &["base", "clips", "outside"],
        &[
            Object::new(
                1,
                1,
                s::game_object(b"same", &[(0, 2), (0, 3), (0, 4), (0, 3)]),
            ),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::legacy(3, (2, -7), &[(1, -7), (2, -7)]),
            a::animator(4, (0, 40)),
            a::overrides(40, (2, 41), &[((2, -9), (3, -8)), ((2, -8), (0, 0))]),
            Object::new(5, 1, s::game_object(b"same", &[(0, 6), (0, 7)])),
            Object::new(6, 4, s::transform((0, 5), (0, 0), &[], false)),
            a::legacy(7, (2, -7), &[]),
        ],
    );
    (clips, base, rig)
}

#[test]
fn ordered_scoped_slots_and_direct_selection() {
    let env = Environment::new(EnvOptions::unitypy());
    let (clips, base, rig) = cross_file_rigs(&env);
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &rig, &GraphOptions::default(), &meter).unwrap();
    let baseline = meter.used();
    let found = discover(&env, &graph, &meter).unwrap();
    let rigs = found.rigs();
    assert_eq!(
        rigs.iter()
            .map(|rig| rig.component.path_id())
            .collect::<Vec<_>>(),
        [3, 4, 7]
    );
    let legacy = &rigs[0];
    assert_eq!(
        legacy
            .slots()
            .iter()
            .map(|slot| slot.source.field)
            .collect::<Vec<_>>(),
        ["m_Animation", "m_Animations", "m_Animations"]
    );
    assert_eq!(legacy.clips().count(), 2);
    let controller = &rigs[1];
    let slots = controller.slots();
    assert_eq!(slots.len(), 6);
    assert_eq!(slots[0].clip().unwrap().path_id(), -7);
    assert!(Arc::ptr_eq(slots[0].clip().unwrap().file(), &base));
    assert_eq!(slots[2].clip().unwrap().path_id(), -8);
    assert!(!slots[3].is_first());
    assert_eq!(controller.clips().count(), 2);
    for (i, field, owner) in [
        (1, "m_OverrideClip", 40),
        (4, "m_AnimationClips", 30),
        (5, "m_OverrideClip", 40),
    ] {
        let err = slots[i].clip().unwrap_err();
        assert!(matches!(
            &err.cause,
            AnimError::OutsideView(DerefError::NotFound { .. })
        ));
        assert_eq!(err.source.field, field);
        assert_eq!(err.source.owner.path_id(), owner);
        assert_eq!(slots[i].source.ordinal, i);
        assert!(slots[i].replacement.is_none());
    }
    let leaves = controller.controller().unwrap().leaves();
    assert_eq!(leaves[0].slot.as_ref().unwrap(), &5);
    assert!(matches!(leaves[2].slot, Err(AnimError::InvalidClipSlot(9))));
    assert_eq!(leaves.len(), 3);
    let root = &graph.nodes()[legacy.node].transform;
    let expected = clips.object(-7).unwrap();
    let selected = found
        .select(
            root,
            &legacy.component,
            ClipSelector::Clip(&expected),
            &meter,
        )
        .unwrap();
    assert_eq!(selected.source.field, "m_Animation");
    let alias = found
        .select(
            &graph.nodes()[controller.node].transform,
            &controller.component,
            ClipSelector::Source(&slots[3].source),
            &meter,
        )
        .unwrap();
    assert_eq!(alias.source.ordinal, 3);
    let error = found
        .select(
            &graph.nodes()[controller.node].transform,
            &controller.component,
            ClipSelector::Source(&slots[1].source),
            &meter,
        )
        .unwrap_err();
    assert!(matches!(error, SelectionError::Unresolved(err)
        if std::ptr::eq(err, slots[1].clip().unwrap_err())));
    assert!(
        found
            .select(
                root,
                &rigs[2].component,
                ClipSelector::Clip(&expected),
                &meter
            )
            .is_err()
    );
    assert_eq!(rigs[2].clips().count(), 1);
    assert!(meter.used() > baseline);
    drop(found);
    assert_eq!(meter.used(), baseline);
}

#[test]
fn ambiguous_outer_override_does_not_name_the_inner_replacement() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load_layout(
        &env,
        b"overrides",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            a::overrides(4, (0, 5), &[((0, 8), (0, 9)), ((0, 8), (0, 0))]),
            a::overrides(5, (0, 6), &[((0, 7), (0, 8))]),
            a::controller(6, &[(0, 7)], &[0]),
            a::clip(7),
            a::clip(8),
            a::clip(9),
        ],
        Layout::Unity6000,
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let rig = &catalog.rigs()[0];
    let [slot] = rig.slots() else {
        panic!("one controller clip slot")
    };
    let failure = slot.clip().unwrap_err();
    let [nested] = rig.issues() else {
        panic!("one nested override diagnostic")
    };
    assert!(matches!(nested.cause, AnimError::NestedOverrideController));
    assert_eq!(nested.source.owner.path_id(), 4);
    assert_eq!(nested.source.field, "m_Controller");
    assert_eq!(nested.source.pointer.unwrap().path_id, 5);
    assert!(matches!(failure.cause, AnimError::AmbiguousOverride));
    assert_eq!(failure.source.owner.path_id(), 4);
    assert!(slot.replacement.is_none());
}

#[test]
fn negative_file_id_cannot_select_the_first_of_two_externals() {
    let env = Environment::new(EnvOptions::unitypy());
    load(&env, b"first", &[], &[a::clip(42)]);
    load(&env, b"second", &[], &[a::clip(42)]);
    let file = load(
        &env,
        b"rig",
        &["first", "second"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            a::overrides(4, (0, 5), &[((0, 6), (-1, 42))]),
            a::controller(5, &[(0, 6)], &[0]),
            a::clip(6),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let failure = catalog.rigs()[0].slots()[0].clip().unwrap_err();
    assert!(matches!(failure.cause, AnimError::NegativeFileId(-1)));
    assert_eq!(failure.source.pointer.unwrap().file_id, -1);
}

#[test]
fn repeated_clips_charge_for_each_bundle_sibling_scan() {
    for siblings in [0, 1024] {
        let env = Environment::new(EnvOptions::batch(Arc::new(FileContext::unitypy())));
        let rig = s::file(
            Layout::Unity2022,
            &["clips"],
            &[
                Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
                Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
                a::legacy(3, (1, 42), &[(1, 42); 3]),
            ],
        )
        .unwrap();
        let names: Vec<_> = (0..siblings).map(|i| format!("{i}.resS")).collect();
        let mut entries = vec![("rig", rig)];
        entries.extend(names.iter().map(|name| (name.as_str(), vec![0])));
        entries.push((
            "clips",
            s::file(Layout::Unity2022, &[], &[a::clip(42)]).unwrap(),
        ));
        let loaded = env
            .load(Source::Bytes {
                name: RawStr::from_static(b"rig.bundle"),
                data: Bytes::from(bundle(&entries)),
            })
            .unwrap();
        let File::Serialized(file) = loaded.container_entry(0).unwrap() else {
            panic!("rig fixture");
        };
        let graph =
            graph_from_scene_file(&env, &file, &GraphOptions::default(), &Meter::unlimited())
                .unwrap();
        let meter =
            Meter::with_decode_work(u64::MAX, 3000, Arc::new(Unbounded), Reservation::Input);
        let result = discover(&env, &graph, &meter);
        if siblings == 0 {
            let catalog = result.unwrap();
            let rig = &catalog.rigs()[0];
            assert_eq!(rig.clips().count(), 1);
            assert_eq!(rig.slots().len(), 4);
        } else {
            assert!(matches!(
                result,
                Err(AnimError::Deref(DerefError::Load(Error::Limit(error))))
                    if error.kind() == LimitKind::DecodeWork
            ));
        }
    }
}

#[test]
fn discovery_charges_the_sibling_scan_when_the_registry_retries() {
    #[derive(Debug, Default)]
    struct RefuseOnce(AtomicBool);

    impl MemoryBudget for RefuseOnce {
        fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
            if what == Reservation::Dependency && self.0.swap(false, Ordering::Relaxed) {
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
            Ok(Permit::detached(bytes))
        }
    }

    let budget = Arc::new(RefuseOnce::default());
    let mut context = FileContext::unitypy();
    context.budget = budget.clone();
    let env = Environment::new(EnvOptions::batch(Arc::new(context)));
    let clip = s::file(Layout::Unity2022, &[], &[a::clip(42)]).unwrap();
    drop(
        env.load_dependency(Source::Bytes {
            name: RawStr::from_static(b"clips.bundle"),
            data: Bytes::from(bundle(&[("clips", clip)])),
        })
        .unwrap(),
    );
    let rig = s::file(
        Layout::Unity2022,
        &["clips"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::legacy(3, (1, 42), &[]),
        ],
    )
    .unwrap();
    let container = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"rig.bundle"),
            data: Bytes::from(bundle(&[("rig", rig), ("padding.resS", vec![0])])),
        })
        .unwrap();
    let File::Serialized(file) = container.container_entry(0).unwrap() else {
        panic!("serialized rig");
    };
    let graph =
        graph_from_scene_file(&env, &file, &GraphOptions::default(), &Meter::unlimited()).unwrap();
    budget.0.store(true, Ordering::Relaxed);

    // Node and components, two field lookups, pointer read and resolution,
    // external path and sibling names, clip interning, then first occurrences.
    let work = 3 + 4 + 5 + (5 + 4 + 13) + 2 + (1 + 4 + 2);
    let meter = Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input);
    let catalog = discover(&env, &graph, &meter).unwrap();
    assert!(!budget.0.load(Ordering::Relaxed));
    assert_eq!(catalog.rigs()[0].slots()[0].clip().unwrap().path_id(), 42);
    assert_eq!(meter.decoded(), work);
}

#[test]
fn override_can_find_sibling_clips_after_its_dependency_is_evicted() {
    #[derive(Debug)]
    struct Pressure(Bytes);
    impl CabResolver for Pressure {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            (name == b"pressure").then(|| Source::Bytes {
                name: RawStr::from_static(b"pressure"),
                data: self.0.clone(),
            })
        }
    }

    let controller = s::file(
        Layout::Unity2022,
        &["clips", "pressure"],
        &[
            a::overrides(4, (0, 5), &[((1, 6), (1, 7))]),
            a::controller(5, &[(1, 6), (2, 8)], &[0, 1]),
        ],
    )
    .unwrap();
    let clips = s::file(Layout::Unity2022, &[], &[a::clip(6), a::clip(7)]).unwrap();
    let pressure = s::file(Layout::Unity2022, &[], &[a::clip(8)]).unwrap();
    let mut options = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    options.dep_cache_bytes = 1;
    options.resolver = Some(Arc::new(Pressure(Bytes::from(pressure))));
    let env = Environment::new(options);
    let dependency = bundle(&[("controller", controller), ("clips", clips)]);
    drop(
        env.load_dependency(Source::Bytes {
            name: RawStr::from_static(b"controller.bundle"),
            data: Bytes::from(dependency),
        })
        .unwrap(),
    );
    let file = load(
        &env,
        b"rig",
        &["controller"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (1, 4)),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let before = env.stats().dependencies_evicted;
    let catalog = discover(&env, &graph, &meter).unwrap();
    assert!(env.stats().dependencies_evicted > before);
    let rig = &catalog.rigs()[0];
    assert!(rig.issues().is_empty(), "{:?}", rig.issues());
    assert_eq!(
        rig.clips()
            .map(|slot| slot.clip().unwrap().path_id())
            .collect::<Vec<_>>(),
        [7, 8]
    );
    assert_eq!(
        rig.slots()[0].replacement.as_ref().unwrap().owner.path_id(),
        4
    );
}

#[test]
fn override_matches_base_clip_identity_after_its_bundle_is_evicted() {
    let mut options = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    options.dep_cache_bytes = 1;
    let env = Environment::new(options);
    let clips = s::file(Layout::Unity2022, &[], &[a::clip(6), a::clip(7)]).unwrap();
    let another = s::file(Layout::Unity2022, &[], &[a::clip(6)]).unwrap();
    let pressure = s::file(Layout::Unity2022, &[], &[a::clip(8)]).unwrap();
    for (name, entries) in [
        (
            b"clips.bundle".as_slice(),
            vec![("clips", clips), ("another", another)],
        ),
        (b"pressure.bundle".as_slice(), vec![("pressure", pressure)]),
    ] {
        drop(
            env.load_dependency(Source::Bytes {
                name: RawStr::from_static(name),
                data: Bytes::from(bundle(&entries)),
            })
            .unwrap(),
        );
    }
    let file = load(
        &env,
        b"rig",
        &["clips", "pressure", "another"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            a::overrides(4, (0, 5), &[((1, 6), (1, 7))]),
            a::controller(5, &[(1, 6), (2, 8), (1, 6), (3, 6)], &[0, 1, 2, 3]),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let before = env.stats().dependencies_evicted;
    let catalog = discover(&env, &graph, &meter).unwrap();
    assert!(env.stats().dependencies_evicted > before);
    let rig = &catalog.rigs()[0];
    assert!(rig.issues().is_empty(), "{:?}", rig.issues());
    let [first, pressure, duplicate, another] = rig.slots() else {
        panic!("four controller slots");
    };
    let clip = first.clip().unwrap();
    assert_eq!(clip.path_id(), 7);
    assert_eq!(pressure.clip().unwrap().path_id(), 8);
    assert!(std::ptr::eq(duplicate.clip().unwrap(), clip));
    assert!(!duplicate.is_first());
    assert_eq!(another.clip().unwrap().path_id(), 6);
    assert!(another.replacement.is_none());

    let ParentKind::Bundle { bundle, .. } = &clip.file().parent().unwrap().kind else {
        panic!("clip bundle");
    };
    env.get_cab(&RawStr::from("pressure"), &meter)
        .unwrap()
        .unwrap();
    assert!(bundle.upgrade().is_none());
    let reopened = env
        .deref(
            &file,
            PPtr {
                file_id: 1,
                path_id: 7,
            },
        )
        .unwrap();
    assert!(!Arc::ptr_eq(clip.file(), reopened.file()));
    let selected = catalog
        .select(
            &rig.root,
            &rig.component,
            ClipSelector::Clip(&reopened),
            &meter,
        )
        .unwrap();
    assert!(std::ptr::eq(selected, first));
}

mod dependency_budget {
    use super::*;

    #[derive(Debug, Default)]
    struct Usage(AtomicU64);
    impl PermitHome for Usage {
        fn release(&self, bytes: u64) {
            self.0.fetch_sub(bytes, Ordering::Relaxed);
        }
    }
    #[derive(Debug)]
    pub(super) struct Pool {
        usage: Arc<Usage>,
        limit: u64,
    }
    impl Pool {
        pub(super) fn new(limit: u64) -> Arc<Self> {
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
}

#[test]
fn alternating_dependency_slots_share_two_retained_clips() {
    let mut context = FileContext::unitypy();
    context.budget = dependency_budget::Pool::new(512 * 1024);
    let mut options = EnvOptions::batch(Arc::new(context));
    options.dep_cache_bytes = 1;
    let env = Environment::new(options);
    let filler: Vec<_> = (0..96).map(|i| a::clip(100 + i)).collect();
    for (name, bundle_name) in [
        ("clips-a", b"clips-a.bundle".as_slice()),
        ("clips-b", b"clips-b.bundle".as_slice()),
    ] {
        let clips = s::file(Layout::Unity2022, &[], &filler).unwrap();
        drop(
            env.load_dependency(Source::Bytes {
                name: RawStr::from_static(bundle_name),
                data: Bytes::from(bundle(&[(name, clips)])),
            })
            .unwrap(),
        );
    }
    let slots: Vec<_> = (0..128).map(|i| (1 + i % 2, 100)).collect();
    let file = load(
        &env,
        b"rig",
        &["clips-a", "clips-b"],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::legacy(3, (0, 0), &slots),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let before = env.stats().dependencies_evicted;
    let catalog = discover(&env, &graph, &meter).unwrap();
    assert!(env.stats().dependencies_evicted > before);
    let rig = &catalog.rigs()[0];
    assert_eq!(rig.clips().count(), 2);
    let slots = &rig.slots()[1..];
    assert_eq!(slots.len(), 128);
    let first = slots[0].clip().unwrap();
    let second = slots[1].clip().unwrap();
    assert_eq!(first.file().name().as_bytes(), b"clips-a");
    assert_eq!(second.file().name().as_bytes(), b"clips-b");
    for (slot, expected) in slots.iter().zip([first, second].into_iter().cycle()) {
        assert!(std::ptr::eq(slot.clip().unwrap(), expected));
    }
}

#[test]
fn unused_override_rows_leave_room_for_the_selected_clip() {
    let discover_with_cache = |cache_bytes, pool_bytes| {
        let mut context = FileContext::unitypy();
        context.budget = dependency_budget::Pool::new(pool_bytes);
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

#[test]
fn override_cycles_are_retained_as_rig_issues() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load(
        &env,
        b"cycle",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            a::overrides(4, (0, 5), &[]),
            a::overrides(5, (0, 4), &[]),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let issue = catalog.rigs()[0]
        .issues()
        .iter()
        .find(|issue| matches!(issue.cause, AnimError::ControllerCycle))
        .expect("cycle diagnostic");
    assert!(matches!(issue.cause, AnimError::ControllerCycle));
    assert_eq!(issue.source.owner.path_id(), 5);
    assert_eq!(issue.source.field, "m_Controller");
    assert_eq!(issue.source.pointer.unwrap().path_id, 4);
}

#[test]
fn malformed_controller_edge_retains_its_owner() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load(
        &env,
        b"bad-edge",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            set_field(
                a::overrides(4, (0, 0), &[]),
                "m_Controller",
                V::Int(1),
                None,
            ),
        ],
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let issue = &catalog.rigs()[0].issues()[0];
    assert_eq!(issue.source.owner.path_id(), 4);
    assert_eq!(issue.source.field, "m_Controller");
    assert!(matches!(issue.cause, AnimError::Malformed(_)));
}

#[test]
fn pre_52_controller_leaves_use_the_state_offset_table() {
    let env = Environment::new(EnvOptions::unitypy());
    let file = load_layout(
        &env,
        b"legacy-controller",
        &[],
        &[
            Object::new(1, 1, s::game_object(b"rig", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            a::animator(3, (0, 4)),
            a::controller_with_leaf_info(
                4,
                &[(0, -7), (0, -8), (0, -9)],
                &[102, 101, 999],
                Some((&[101, 102], 1)),
            ),
            a::clip(-7),
            a::clip(-8),
            a::clip(-9),
        ],
        Layout::Unity51,
    );
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    let catalog = discover(&env, &graph, &meter).unwrap();
    let rig = &catalog.rigs()[0];
    let controller = rig.controller().unwrap();
    assert_eq!(
        controller.strings(0, &meter).unwrap().as_slice(),
        &[b"".as_slice()]
    );
    let leaves = controller.leaves();
    assert_eq!(leaves[0].slot.as_ref().unwrap(), &2);
    assert_eq!(leaves[1].slot.as_ref().unwrap(), &1);
    assert!(matches!(
        leaves[2].slot,
        Err(AnimError::UnmappedControllerLeaf(999))
    ));
    assert_eq!(rig.slots()[0].clip().unwrap().path_id(), -7);
}
