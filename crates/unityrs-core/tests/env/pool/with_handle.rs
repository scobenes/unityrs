use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::Reservation;
use unityrs_core::env::{EnvOptions, Environment, LoadMode, Source};
use unityrs_core::{Bytes, Error, RawStr};
use unityrs_testkit::env::{Obj, serialized};

use super::{RefuseOnce, RefusingFetch};
use crate::{ctx_with, named_bundle, src};

/// Count the load's dependency reservations with the pool granting all of
/// them, then refuse each one in a fresh environment. A refusal that the
/// reader absorbs needs no retry. A retry reopens the source and evicts the
/// cached dependency. A second refusal is returned to the caller.
#[test]
fn load_dependency_with_handle_retries_each_pool_refusal() {
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let run = |reservation| {
            let budget = Arc::new(RefuseOnce::default());
            let mut options = EnvOptions::batch(ctx_with(budget.clone()));
            options.load = mode;
            let env = Environment::new(options);
            let prior = serialized(&[], &[Obj::named(1, "prior", &[])]);
            drop(env.load_dependency(src("prior.assets", &prior)).unwrap());

            let fetch = Arc::new(RefusingFetch {
                refusals: AtomicU64::new(0),
                runs: AtomicU64::new(0),
                data: Bytes::from(named_bundle("CAB-Q", 64)),
            });
            let before = budget.asked();
            budget.arm(reservation);
            let (file, handle) = env
                .load_dependency_with_handle(Source::Fetch {
                    name: RawStr::from("q.bundle"),
                    fetch: fetch.clone(),
                })
                .unwrap_or_else(|error| panic!("{mode:?}, reservation {reservation}: {error}"));
            let asked = budget.asked() - before;
            assert_eq!(budget.refused(), u64::from(reservation != 0));
            budget.arm(0);

            let runs = fetch.runs.load(Ordering::Acquire);
            assert!((1..=2).contains(&runs));
            assert_eq!(env.stats().dependencies_evicted, runs - 1);
            assert_eq!(env.files().count(), 0);
            let meter = file.meter().clone();
            drop(file);
            if reservation == 0 {
                // Replace every CAB name from the first load. Its handle
                // still reopens the original dependency on the same account.
                drop(
                    env.load_dependency(src("shadow.bundle", &fetch.data))
                        .unwrap(),
                );
            }
            assert!(
                handle
                    .read(|file| Ok(meter.same_input(file.meter())))
                    .unwrap()
            );
            assert_eq!(
                fetch.runs.load(Ordering::Acquire),
                runs + u64::from(reservation == 0)
            );
            (asked, runs)
        };

        let (reservations, runs) = run(0);
        assert!(reservations > 0);
        assert_eq!(runs, 1);
        for reservation in 1..=reservations {
            let (_, runs) = run(reservation);
            if reservation == 1 {
                assert_eq!(runs, 2, "{mode:?}: retry a refused open");
            }
        }

        let mut options = EnvOptions::unitypy();
        options.load = mode;
        let env = Environment::new(options);
        let fetch = RefusingFetch::new(2);
        let result = env.load_dependency_with_handle(Source::Fetch {
            name: RawStr::from("twice-refused.bundle"),
            fetch: fetch.clone(),
        });
        assert!(matches!(result, Err(Error::Budget(e)) if e.what == Reservation::Dependency));
        assert_eq!(fetch.runs.load(Ordering::Acquire), 2);
    }
}
