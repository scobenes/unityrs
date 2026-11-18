use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use unityrs_anim::{AnimError, PathQuery, RigPaths, SlotCounts, discover, read_bindings};
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation, Unbounded};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::File;
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::{Bytes, LimitKind, ObjectRef, RawStr};
use unityrs_scene::{Graph, GraphOptions, graph_from_game_object};
use unityrs_testkit::ngui::Object;
use unityrs_testkit::scene3d::{self as s, Layout, anim as a};

fn fixture() -> (Environment, Graph, ObjectRef) {
    let env = Environment::new(EnvOptions::unitypy());
    let clip = a::bound_clip(
        9,
        Layout::Unity2022,
        &[a::Binding {
            class: 4,
            attribute: 1,
            ..Default::default()
        }; 6],
        &[(0, 0)],
    );
    let mut objects = vec![
        Object::new(1, 1, s::game_object(b"root", &[(0, 2), (0, 3)])),
        Object::new(
            2,
            4,
            s::transform((0, 1), (0, 0), &[(0, 11), (0, 21)], false),
        ),
        a::animator(3, (0, 5)),
        a::overrides(5, (0, 6), &[((0, 8), (0, 9))]),
        a::controller(6, &[(0, 8); 12], &[0, 3, 7]),
        a::clip(8),
        clip,
    ];
    for id in [10, 20] {
        objects.push(Object::new(id, 1, s::game_object(b"child", &[(0, id + 1)])));
        objects.push(Object::new(
            id + 1,
            4,
            s::transform((0, id), (0, 2), &[], false),
        ));
    }
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"meter"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], &objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("serialized fixture");
    };
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        &Meter::unlimited(),
    )
    .unwrap();
    (env, graph, file.object(9).unwrap())
}

fn walk(
    env: &Environment,
    graph: &Graph,
    clip: &ObjectRef,
    meter: &Arc<Meter>,
) -> Result<(), AnimError> {
    let catalog = discover(env, graph, meter)?;
    let paths = RigPaths::new(graph, 0, meter)?;
    paths.resolve(PathQuery::Hash(0), meter)?;
    let table = read_bindings(clip, meter)?;
    let slots = table.slots(
        SlotCounts {
            constant: 18,
            ..Default::default()
        },
        meter,
    )?;
    assert_eq!(catalog.rigs()[0].clips().count(), 1);
    assert_eq!(slots.len(), 6);
    assert!(meter.used() > 0);
    Ok(())
}

#[derive(Debug)]
struct RefuseAfter {
    seen: AtomicUsize,
    at: usize,
}

impl MemoryBudget for RefuseAfter {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if self.seen.fetch_add(1, Ordering::Relaxed) >= self.at {
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
fn reservation_and_work_refusals_release_partial_results() {
    let (env, graph, clip) = fixture();
    let budget = Arc::new(RefuseAfter {
        seen: AtomicUsize::new(0),
        at: usize::MAX,
    });
    let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Dependency);
    walk(&env, &graph, &clip, &meter).unwrap();
    assert_eq!(meter.used(), 0);
    let reserves = budget.seen.load(Ordering::Relaxed);
    assert!(reserves >= 10);
    for at in 0..reserves {
        let budget = Arc::new(RefuseAfter {
            seen: AtomicUsize::new(0),
            at,
        });
        let meter = Meter::new(u64::MAX, budget, Reservation::Dependency);
        let err = walk(&env, &graph, &clip, &meter).unwrap_err();
        assert!(
            matches!(err, AnimError::Charge(ChargeError::Budget(ref e)) if e.what == Reservation::Dependency),
            "reservation {at}: {err:?}"
        );
        assert_eq!(meter.used(), 0, "reservation {at}");
    }
    let work = meter.decoded();
    assert!(work > 100);
    for limit in [0, 1, work / 2, work - 1, work] {
        let meter =
            Meter::with_decode_work(u64::MAX, limit, Arc::new(Unbounded), Reservation::Input);
        let result = walk(&env, &graph, &clip, &meter);
        if limit == work {
            result.unwrap();
        } else {
            assert!(
                matches!(result, Err(AnimError::Charge(ChargeError::Limit(ref e))) if e.kind() == LimitKind::DecodeWork),
                "work {limit}: {result:?}"
            );
        }
        assert_eq!(meter.used(), 0);
    }
}

#[cfg(feature = "test-counters")]
mod allocator {
    use super::*;
    use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, counted};

    static AT: AtomicUsize = AtomicUsize::new(usize::MAX);
    static SEEN: AtomicUsize = AtomicUsize::new(0);

    struct Refusing;
    impl Tally for Refusing {
        fn refuses(&self, _: usize) -> bool {
            counted()
                && unityrs_core::testing::in_fallible_reserve()
                && SEEN.fetch_add(1, Ordering::Relaxed) >= AT.load(Ordering::Relaxed)
        }
        fn grew(&self, _: usize, _: bool) {}
        fn shrank(&self, _: usize) {}
    }
    #[global_allocator]
    static ALLOCATOR: Allocator<Refusing> = Allocator(Refusing);

    #[test]
    fn fallible_allocations_unwind_with_no_remaining_charge() {
        count_this_thread();
        let (env, graph, clip) = fixture();
        let meter = Meter::unlimited();
        walk(&env, &graph, &clip, &meter).unwrap();
        SEEN.store(0, Ordering::Relaxed);
        walk(&env, &graph, &clip, &meter).unwrap();
        let allocations = SEEN.load(Ordering::Relaxed);
        assert!(allocations >= 10);
        let file_charge = clip.file().meter().used();
        for at in 0..allocations {
            SEEN.store(0, Ordering::Relaxed);
            AT.store(at, Ordering::Relaxed);
            let result = walk(&env, &graph, &clip, &meter);
            AT.store(usize::MAX, Ordering::Relaxed);
            assert!(result.is_err(), "allocator refusal {at}");
            drop(result);
            assert_eq!(meter.used(), 0, "allocator refusal {at}");
            assert_eq!(
                clip.file().meter().used(),
                file_charge,
                "allocator refusal {at}"
            );
        }
    }
}
