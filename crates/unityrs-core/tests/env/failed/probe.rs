//! The probe: while a dependency's load runs, another thread's lookups
//! find none of its keys and none of the values it replaces (plan
//! section 3.16, item 6). A module of `failed.rs`, whose scenes and
//! helpers it uses; split off so no file passes 800 lines.

use std::thread::ThreadId;

use super::*;

/// `B`'s names: two new keys and the two `A` holds, whose values `B`
/// replaces.
const B_NAMES: [&str; 4] = ["CAB-B", "CAB-S", "CAB-S.resS", "CAB-B2"];

/// What another thread finds during `B`'s load: the registry's keys, and
/// each of `B`'s names' `get_cab`.
fn probed(env: &Environment) -> Vec<String> {
    let mut out = vec![format!("keys {:?}", keys(env))];
    for name in B_NAMES {
        let found = cab(env.get_cab(&key(name), &unlimited()));
        out.push(format!("get_cab {name}: {found}"));
    }
    out
}

/// A budget that, at each dependency reservation the loading thread makes,
/// has another thread look up what [`probed`] does, and keeps what it saw:
/// lookups made while the load runs, between any two of its charges and
/// after the last one before its commit.
#[derive(Default)]
struct Probe {
    env: Mutex<Option<Environment>>,
    loader: Mutex<Option<ThreadId>>,
    seen: Mutex<Vec<Vec<String>>>,
}

impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Probe")
    }
}

impl MemoryBudget for Probe {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let loading = *self.loader.lock().unwrap() == Some(std::thread::current().id());
        if what == Reservation::Dependency && loading {
            let env = self.env.lock().unwrap().clone();
            if let Some(env) = env {
                let seen =
                    guard::finishes("a lookup on another thread during the load", move || {
                        probed(&env)
                    });
                self.seen.lock().unwrap().push(seen);
            }
        }
        Unbounded.try_reserve(bytes, what)
    }
}

/// While `B`'s load runs, no lookup on another thread finds one of its
/// keys: at each of its dependency reservations, the last of them the
/// LRU's entry just before the commit, another thread's key listing, and
/// its `get_cab` of each of `B`'s names, two of them keys `A` holds whose
/// values `B` replaces, are the ones before the load (plan section 3.16,
/// item 6; CR1's review, round 2: the probe saw the keys, not the values).
#[test]
fn no_lookup_on_another_thread_finds_a_key_of_a_load_before_its_commit() {
    for (what, mut options, _) in options(LoadMode::Lazy) {
        let probe = Arc::new(Probe::default());
        options.context = ctx_with(probe.clone());
        let env = Environment::new(options);
        drop(env.load_dependency(a()).unwrap());
        // `A`'s slots reached, so the commit's `settle` has none to reach
        // (it would reserve after the first key is published).
        for name in NAMES {
            drop(env.get_cab(&key(name), &unlimited()).unwrap());
        }
        let before = probed(&env);
        *probe.env.lock().unwrap() = Some(env.clone());
        *probe.loader.lock().unwrap() = Some(std::thread::current().id());
        drop(env.load_dependency(b()).unwrap());
        *probe.loader.lock().unwrap() = None;
        *probe.env.lock().unwrap() = None;
        let seen = std::mem::take(&mut *probe.seen.lock().unwrap());
        assert!(
            seen.len() as u64 > 4 * B_ENTRIES,
            "{what}: {} probes",
            seen.len()
        );
        for (i, found) in seen.iter().enumerate() {
            assert_eq!(found, &before, "{what}: at reservation {}", i + 1);
        }
        let after = probed(&env);
        assert_ne!(after[0], before[0], "{what}: the load published its keys");
        assert_ne!(after[2], before[2], "{what}: the load replaced CAB-S");
    }
}
