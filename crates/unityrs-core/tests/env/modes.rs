//! The load modes' differences, (a) to (d) (plan section 3.13).

use std::sync::{Arc, Mutex, mpsc};

use unityrs_core::FileContext;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{Cab, EnvOptions, Environment, LoadMode, PPtr};
use unityrs_core::error::{Error, LimitKind};
use unityrs_core::files::File;
use unityrs_testkit::env::{
    Bundle, Obj, bad_cab, bundle, last_block_fails, nested_after_cab, nested_duplicates, serialized,
};

use crate::{ctx_with, env, file_of, key, keys, name_of, object, serialized_cab, src, unlimited};

/// (a): under `Eager` a CAB that fails to parse fails the bundle's load,
/// with the CABs before it registered and nothing in `files`; under `Lazy`
/// the load succeeds, the sibling reads, only the failing CAB's `get`
/// fails, and the bundle's `load_all` returns exactly `Eager`'s error.
#[test]
fn difference_a_a_failing_entry_fails_only_its_reads_under_lazy() {
    let scene = bad_cab();
    let eager = env(LoadMode::Eager);
    let eager_error = eager.load(file_of(&scene, 0)).unwrap_err();
    assert!(eager.files().next().is_none());
    assert_eq!(keys(&eager), ["cab-g", "cab-g.ress"]);

    let lazy = env(LoadMode::Lazy);
    let File::Bundle(b) = lazy.load(file_of(&scene, 0)).unwrap() else {
        panic!()
    };
    assert_eq!(keys(&lazy), ["cab-g", "cab-g.ress", "cab-bad"]);
    assert_eq!(name_of(&object(&serialized_cab(&lazy, "cab-g"), 1)), "g1");
    assert!(lazy.get_cab(&key("cab-bad"), &unlimited()).is_err());
    assert_eq!(
        b.load_all().unwrap_err().to_string(),
        eager_error.to_string()
    );
}

/// (a): an entry shadowed by a later one of the same path is never parsed
/// under `Lazy`; `Eager` parses it first and fails.
#[test]
fn difference_a_a_shadowed_entry_is_never_parsed_under_lazy() {
    let bad = serialized(
        &[],
        &[Obj::asset_bundle(
            1,
            Bundle {
                no_container: true,
                ..Bundle::default()
            },
        )],
    );
    let good = serialized(&[], &[Obj::named(1, "good", &[])]);
    let data = bundle(&[("CAB-X", bad), ("CAB-X", good)]);
    assert!(env(LoadMode::Eager).load(src("x.bundle", &data)).is_err());
    let lazy = env(LoadMode::Lazy);
    lazy.load(src("x.bundle", &data)).unwrap();
    assert_eq!(name_of(&object(&serialized_cab(&lazy, "cab-x"), 1)), "good");
    assert_eq!(lazy.stats().entries_parsed, 1);
}

/// (b): a nested bundle under a CAB's path: `Eager` keeps the CAB under the
/// key and registers the nested entries in place; `Lazy` registers the key
/// for the nested entry, whose first `get` answers `None` and reaches it:
/// its entries are registered then, at the ranks an eager load gives them.
#[test]
fn difference_b_a_nested_entry_is_a_key_until_its_first_get() {
    let scene = nested_after_cab();
    let eager = env(LoadMode::Eager);
    eager.load(file_of(&scene, 0)).unwrap();
    assert_eq!(keys(&eager), ["cab-n", "cab-inner"]);
    assert_eq!(name_of(&object(&serialized_cab(&eager, "cab-n"), 1)), "n1");

    let lazy = env(LoadMode::Lazy);
    lazy.load(file_of(&scene, 0)).unwrap();
    assert_eq!(keys(&lazy), ["cab-n"]);
    assert!(lazy.get_cab(&key("cab-n"), &unlimited()).unwrap().is_none());
    assert_eq!(keys(&lazy), ["cab-n", "cab-inner"]);
    assert_eq!(
        name_of(&object(&serialized_cab(&lazy, "cab-inner"), 2)),
        "inner"
    );
}

/// (b), and the order of reaching: a nested container's entries take the
/// place and the value an eager load gives them, whichever nested container
/// is asked for first, so two that hold a CAB of one name leave the second's
/// under the key either way, as `Eager` does (review round 1: under `Lazy`
/// the one asked for last won).
#[test]
fn nested_containers_register_in_directory_order_whichever_is_asked_first() {
    let scene = nested_duplicates();
    let eager = env(LoadMode::Eager);
    eager.load(file_of(&scene, 0)).unwrap();
    assert_eq!(keys(&eager), ["cab-dup"]);
    assert_eq!(
        name_of(&object(&serialized_cab(&eager, "cab-dup"), 1)),
        "from-n2"
    );
    let mut seen = Vec::new();
    for order in [["n1", "n2"], ["n2", "n1"]] {
        let lazy = env(LoadMode::Lazy);
        lazy.load(file_of(&scene, 0)).unwrap();
        for n in order {
            assert!(lazy.get_cab(&key(n), &unlimited()).unwrap().is_none());
        }
        let winner = name_of(&object(&serialized_cab(&lazy, "cab-dup"), 1));
        seen.push((keys(&lazy), winner));
    }
    assert_eq!(seen[0], seen[1], "the same whichever is asked first");
    assert_eq!(seen[0].0, ["n1", "cab-dup", "n2"]);
    assert_eq!(seen[0].1, "from-n2");
}

/// A budget that, once a thread arms it, stops that thread at its next
/// reservation until the test lets it go (a test's hook: a real budget
/// never blocks).
#[derive(Debug)]
struct Gate {
    armed: Mutex<Option<std::thread::ThreadId>>,
    stopped: Mutex<Option<mpsc::Sender<()>>>,
    go: Mutex<Option<mpsc::Receiver<()>>>,
}

impl MemoryBudget for Gate {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        let me = std::thread::current().id();
        let stop = {
            let mut armed = self.armed.lock().unwrap();
            let stop = *armed == Some(me);
            if stop {
                *armed = None;
            }
            stop
        };
        if stop {
            if let Some(tx) = self.stopped.lock().unwrap().take() {
                let _ = tx.send(());
            }
            let go = self.go.lock().unwrap().take();
            if let Some(rx) = go {
                unityrs_testkit::guard::hold(&rx, "the test to let the gated thread go");
            }
        }
        Ok(Permit::detached(bytes))
    }
}

/// (b) under threads: while one thread registers a nested container's
/// entries, another that asks for the container and then for an entry finds
/// the entry (review round 1: the first thread marked the container expanded
/// before it registered anything, so the second returned at once and found
/// nothing). The first thread is stopped at its first registration's charge.
#[test]
fn a_get_while_another_thread_expands_a_nested_container_sees_its_entries() {
    let inner = bundle(&[
        ("CAB-C1", serialized(&[], &[Obj::named(1, "c1", &[])])),
        ("CAB-C2", serialized(&[], &[Obj::named(2, "c2", &[])])),
    ]);
    let data = bundle(&[("CAB-NEST", inner)]);
    let (stopped_tx, stopped_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let gate = Arc::new(Gate {
        armed: Mutex::new(None),
        stopped: Mutex::new(Some(stopped_tx)),
        go: Mutex::new(Some(go_rx)),
    });
    let mut o = EnvOptions::unitypy();
    o.context = ctx_with(gate.clone());
    o.load = LoadMode::Lazy;
    let env = Environment::new(o);
    let File::Bundle(outer) = env.load(src("o.bundle", &data)).unwrap() else {
        panic!()
    };
    // Parsed now: the expanding thread's first charge is its first
    // registration's.
    outer.entry(0).unwrap();
    let first = {
        let env = env.clone();
        let gate = gate.clone();
        std::thread::spawn(move || {
            *gate.armed.lock().unwrap() = Some(std::thread::current().id());
            env.get_cab(&key("cab-nest"), &unlimited())
                .unwrap()
                .is_none()
        })
    };
    unityrs_testkit::guard::wait(
        &stopped_rx,
        "the first thread to stop at its first registration",
    );
    assert!(
        env.get_cab(&key("cab-nest"), &unlimited())
            .unwrap()
            .is_none()
    );
    let c2 = env.get_cab(&key("cab-c2"), &unlimited()).unwrap();
    go_tx.send(()).unwrap();
    assert!(first.join().unwrap());
    assert!(matches!(c2, Some(Cab::Serialized(_))), "{c2:?}");
    assert_eq!(keys(&env), ["cab-nest", "cab-c1", "cab-c2"]);
}

/// (c): a block past every entry's bytes that fails to decode fails the
/// `Eager` load and nothing under `Lazy`.
#[test]
fn difference_c_a_block_after_every_entry_fails_only_eager() {
    let scene = last_block_fails();
    assert!(env(LoadMode::Eager).load(file_of(&scene, 0)).is_err());
    let lazy = env(LoadMode::Lazy);
    lazy.load(file_of(&scene, 0)).unwrap();
    let sf = serialized_cab(&lazy, "cab-l");
    let target = lazy
        .deref(
            &sf,
            PPtr {
                file_id: 0,
                path_id: 1,
            },
        )
        .unwrap();
    assert_eq!(name_of(&target), "l1");
}

/// (d): entries whose spans are nested suffixes of the data: `Eager` decodes
/// one run and fits; `Lazy` decodes each entry's span again and keeps it,
/// so listing the objects stops at the meter (`InputMemory`).
#[test]
fn difference_d_overlapping_spans_stop_lazy_at_the_meter() {
    const N: usize = 24;
    const B: usize = 1 << 16;
    let entries: Vec<(String, Vec<u8>)> = (0..N)
        .map(|k| {
            (
                format!("suffix-{k:02}.resS"),
                (0..B)
                    .map(|i| ((i * 31 + k) % 251).to_le_bytes()[0])
                    .collect(),
            )
        })
        .collect();
    let mut spec = unityrs_testkit::bundle::BundleSpec {
        info_codec: unityrs_testkit::bundle::TestCodec::None,
        block_size: B,
        entries: entries
            .iter()
            .map(|(n, d)| unityrs_testkit::bundle::EntrySpec {
                name: n.clone().into_bytes(),
                data: d.clone(),
                flags: 0,
            })
            .collect(),
        ..unityrs_testkit::bundle::BundleSpec::default()
    };
    spec.data_codec = unityrs_testkit::bundle::TestCodec::Lz4;
    let mut data = unityrs_testkit::bundle::build(&spec);
    for k in 0..N {
        let name = format!("suffix-{k:02}.resS\0").into_bytes();
        let at = data.windows(name.len()).position(|w| w == name).unwrap();
        let offset = i64::try_from((N - 1 - k) * B).unwrap();
        let size = i64::try_from((k + 1) * B).unwrap();
        data[at - 20..at - 12].copy_from_slice(&offset.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    }
    let ctx = {
        let mut c = FileContext::unitypy();
        c.limits.max_input_memory = 6 << 20;
        Arc::new(c)
    };
    let mut o = EnvOptions::unitypy();
    o.context = ctx.clone();
    let eager = Environment::new(o.clone());
    eager.load(src("d.bundle", &data)).unwrap();
    assert!(eager.objects().all(|o| o.is_ok()));

    o.load = LoadMode::Lazy;
    let lazy = Environment::new(o);
    lazy.load(src("d.bundle", &data)).unwrap();
    let stopped = lazy.objects().find_map(Result::err).expect("a read stops");
    assert!(
        matches!(&stopped, Error::Limit(l) if l.kind() == LimitKind::InputMemory),
        "{stopped:?}"
    );
}
