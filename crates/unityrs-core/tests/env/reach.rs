//! What a reach of a nested container under `LoadMode::Lazy` reaches, what
//! it costs and whom it charges (plan section 3.16, R2, R4 and R5): a
//! dropped container another handle cannot stand in for is reached, a
//! reach its input's meter refuses is counted as that open's work until the
//! allowance keeps the refusal, and a reach through a job's handle charges
//! a shared dependency's own meter.

use std::sync::Arc;

use unityrs_core::env::{Cab, EnvOptions, Environment, LoadMode, RawCab, Source};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{bundle, res_bytes};

use super::{TempFile, key, object, serialized_cab, stream, unlimited};

/// `CAB-I` of a bundle laid out as the scene `resource_in_nested_cab`:
/// the first entry of the nested bundle `INNER`.
fn inner_cab(outer: &File) -> Arc<unityrs_core::files::SerializedFile> {
    let File::Bundle(outer) = outer else {
        panic!("o.bundle is a bundle");
    };
    let File::Bundle(inner) = outer.entry(0).unwrap() else {
        panic!("INNER is a bundle");
    };
    let File::Serialized(cab) = inner.entry(0).unwrap() else {
        panic!("CAB-I is a SerializedFile");
    };
    cab
}

/// Two dependency loads of one path (the same input opened twice, whose
/// handles share no bytes): the second's slot for `INNER` cannot be told
/// to stand in for the first's from the first's handle, so the first's is
/// reached when the second drops it, and a reader inside the first's
/// `INNER` reads its sibling `.resS` as UnityPy does (the scene
/// `nested_reloaded`, recorded in `tests/fixtures/d/env.json`: `28292a2b`;
/// review round 5: the drop was taken for a stand-in, nothing reached it,
/// and the read missed every candidate).
#[test]
fn a_reader_inside_the_first_of_two_loads_of_one_path_reads_its_sibling_resource() {
    let scene = unityrs_testkit::env::resource_in_nested_cab();
    let file = TempFile::new("reload", &scene.files[0].1);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        let env = Environment::new(o);
        let first = env.load_dependency(Source::Path(file.0.clone())).unwrap();
        drop(env.load_dependency(Source::Path(file.0.clone())).unwrap());
        let cab = inner_cab(&first);
        let (path, offset, size) = stream(&object(&cab, 1));
        let bytes = env.resource(&cab, &path, offset, size).unwrap();
        assert_eq!(bytes.as_ref(), [0x28, 0x29, 0x2a, 0x2b], "{mode:?}");
    }
}

/// `o.bundle`'s nested bundle `INNER` of `n` raw entries, and `p.bundle`,
/// whose raw entry `INNER` drops it (the scene `nested_raw_replaced`'s
/// shape, larger).
fn raw_replaced(n: usize) -> (Bytes, Bytes) {
    let names: Vec<String> = (0..n).map(|i| format!("R{i}.resS")).collect();
    let entries: Vec<(&str, Vec<u8>)> = names
        .iter()
        .map(|name| (name.as_str(), res_bytes(4, 10)))
        .collect();
    let o = bundle(&[("INNER", bundle(&entries))]);
    let p = bundle(&[("INNER", res_bytes(4, 50))]);
    (Bytes::from(o), Bytes::from(p))
}

fn lazy_batch(ctx: FileContext) -> Environment {
    let mut o = EnvOptions::batch(Arc::new(ctx));
    o.load = LoadMode::Lazy;
    Environment::new(o)
}

fn bytes(name: &str, data: &Bytes) -> Source {
    Source::Bytes {
        name: RawStr::from(name),
        data: data.clone(),
    }
}

/// A reach the input's own meter refuses is counted as that open's work
/// (`crate::once`'s rule), so once the open's allowance is used up the
/// refusal is `DecodeWork` and kept, and no later lookup does the reach's
/// work again (review round 5: every lookup by name redid it, uncounted,
/// with no bound per input: the `ZeroSizeElements` class). The limits are
/// measured on an unlimited run: room for the load and the container's
/// parse but not for the reach's registrations, and work for an unrefused
/// reach alone.
#[test]
fn a_reach_the_meter_refuses_is_counted_until_it_is_kept() {
    let (o, p) = raw_replaced(200);
    let measure = lazy_batch(FileContext::unitypy());
    let outer = measure.load(bytes("o.bundle", &o)).unwrap();
    let meter = outer.meter().clone();
    let File::Bundle(b) = &outer else {
        panic!("o.bundle is a bundle")
    };
    drop(b.entry(0).unwrap());
    let parsed = meter.used();
    drop(measure.load(bytes("p.bundle", &p)).unwrap());
    let (reached, work) = (meter.used(), meter.decoded());
    assert!(
        measure
            .get_cab(&key("R7.resS"), &unlimited())
            .unwrap()
            .is_some()
    );
    assert!(
        reached > parsed + 4096,
        "the reach registers: {parsed} -> {reached}"
    );

    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_memory = parsed + (reached - parsed) / 2;
    ctx.limits.max_decode_work = work;
    let env = lazy_batch(ctx);
    let outer = env.load(bytes("o.bundle", &o)).unwrap();
    let meter: Arc<Meter> = outer.meter().clone();
    // The drop's reach is refused by the meter, counted, and kept.
    drop(env.load(bytes("p.bundle", &p)).unwrap());
    assert!(
        meter.work_used_up().is_some(),
        "the refused reach's grants were not counted: {} of {work}",
        meter.decoded()
    );
    let (granted, done) = (meter.granted(), meter.decoded());
    // A lookup, the key listing and the container pass do the work no
    // more. The first pass meets the kept refusal reading the container's
    // entries, and is kept as UnityPy keeps a failed pass (the index built).
    let pass = env.container().map(|_| ()).map_err(|e| e.to_string());
    assert!(pass.is_err_and(|e| e.starts_with("limit DecodeWork")));
    let pass = env.container().map(|_| ()).map_err(|e| e.to_string());
    for i in 0..50 {
        drop(
            env.get_cab(&key(&format!("NONE{i}")), &unlimited())
                .unwrap(),
        );
        let _ = env.registry().keys().count();
        let again = env.container().map(|_| ()).map_err(|e| e.to_string());
        assert_eq!(again, pass);
    }
    assert_eq!(meter.granted(), granted, "a lookup redid the refused reach");
    assert_eq!(meter.decoded(), done, "a lookup counted more work");
}

/// A batch job's resource read that reaches a shared dependency's nested
/// container through the job's own handle charges the registrations to
/// the dependency's meter (the pool's), never the job's: once the job has
/// let go of its handles, its meter holds nothing (review round 5: the
/// shared registry's entries kept 3,388 bytes on the ended job's meter for
/// the Environment's life).
#[test]
fn a_jobs_read_leaves_no_charge_on_its_meter() {
    let scene = unityrs_testkit::env::resource_in_nested_cab();
    let data = Bytes::from(scene.files[0].1.clone());
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        let shared = Environment::new(o.clone());
        drop(shared.load_dependency(bytes("o.bundle", &data)).unwrap());
        let job = Environment::new(o);
        let outer = job.load(bytes("o.bundle", &data)).unwrap();
        let job_meter = outer.meter().clone();
        let cab = inner_cab(&outer);
        let (path, offset, size) = stream(&object(&cab, 1));
        let read = shared.resource(&cab, &path, offset, size).unwrap();
        assert_eq!(read.as_ref(), [0x28, 0x29, 0x2a, 0x2b], "{mode:?}");
        drop((read, path, cab, outer, job));
        assert_eq!(job_meter.used(), 0, "{mode:?}: the ended job's meter");
        assert!(
            shared
                .get_cab(&key("CAB-I.resS"), &unlimited())
                .unwrap()
                .is_some(),
            "{mode:?}"
        );
    }
}

/// A container pass whose walk of a preload table its file's own meter
/// refuses is counted as that open's work (plan section 3.16, P1: what the
/// walk was granted), so once the open's allowance is used up the refusal
/// is `DecodeWork`, which the input decides, and the pass is kept as
/// UnityPy keeps a failed pass (the index built): no later `container()`
/// walks the table again (review round 6: nothing tested it, and without
/// the count every `container()` redid the refused walk in full). The
/// limits are measured on runs of the scene `preload_paths`: memory for the
/// load and the containers' reads but not for the walk's `path_dict`
/// entries, and, on a run with that memory and no work bound, the work one
/// refused walk adds, half of which past the reads is allowed.
#[test]
fn a_pass_the_meter_refuses_is_counted_until_it_is_kept() {
    let scene = unityrs_testkit::env::preload_paths();
    let data = Bytes::from(scene.files[0].1.clone());
    let eager = |ctx: FileContext| {
        let mut o = EnvOptions::unitypy();
        o.context = Arc::new(ctx);
        Environment::new(o)
    };
    // The memory the reads take, and what the walk keeps.
    let env = eager(FileContext::unitypy());
    let meter = env.load(bytes("pp.bundle", &data)).unwrap().meter().clone();
    for cab in ["CAB-PP", "CAB-PQ"] {
        serialized_cab(&env, cab).container().unwrap();
    }
    let read = meter.used();
    let _ = env.container().unwrap().count();
    let walked = meter.used();
    assert!(
        walked > read + 64,
        "the walk keeps paths: {read} -> {walked}"
    );

    // The work a refused walk adds, on a run with no work bound.
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_memory = read + (walked - read) / 2;
    let env = eager(ctx.clone());
    let meter = env.load(bytes("pp.bundle", &data)).unwrap().meter().clone();
    let refused = |env: &Environment| env.container().map(|_| ()).map_err(|e| e.to_string());
    let first = refused(&env);
    assert!(
        first
            .as_ref()
            .is_err_and(|e| e.starts_with("limit InputMemory")),
        "the walk is refused by the meter: {first:?}"
    );
    let after_first = meter.decoded();
    assert_eq!(refused(&env), first);
    let per_walk = meter.decoded() - after_first;
    assert!(per_walk > 0, "a refused walk counted no work");

    // With half a walk's work allowed past the reads, the first refusal
    // uses the allowance up and is kept.
    ctx.limits.max_decode_work = after_first - per_walk / 2;
    let env = eager(ctx);
    let meter = env.load(bytes("pp.bundle", &data)).unwrap().meter().clone();
    let pass = refused(&env);
    assert!(
        pass.as_ref()
            .is_err_and(|e| e.starts_with("limit DecodeWork")),
        "the refused walk's work was not counted: {pass:?}"
    );
    let (granted, done) = (meter.granted(), meter.decoded());
    let kept = refused(&env);
    for _ in 0..50 {
        assert_eq!(refused(&env), kept);
    }
    assert_eq!(meter.granted(), granted, "a pass walked the table again");
    assert_eq!(meter.decoded(), done, "a pass counted more work");
}

/// The work counted on `a.bundle`'s meter: `m` raw entries of one 160-byte
/// block each, all alike, loaded `Lazy`, each reached once in directory
/// order (a read of its first bytes, the same for each); by `settle`, after
/// `b.bundle`'s nested `INNER`, which holds entries of the same names, drops
/// all `m` slots unreached at once, or, with `by_name`, by a lookup of each
/// name, which lists nothing.
fn reach_work(m: usize, by_name: bool) -> u64 {
    let names: Vec<String> = (0..m).map(|i| format!("E{i:05}")).collect();
    let entries = |data: &dyn Fn() -> Vec<u8>| -> Vec<(&str, Vec<u8>)> {
        names.iter().map(|n| (n.as_str(), data())).collect()
    };
    let a = Bytes::from(bundle(&entries(&|| res_bytes(160, 7))));
    let b = Bytes::from(bundle(&[("INNER", bundle(&entries(&|| res_bytes(8, 9))))]));
    let env = lazy_batch(FileContext::unitypy());
    let meter = env.load(bytes("a.bundle", &a)).unwrap().meter().clone();
    let asker = Meter::unlimited();
    let before = meter.decoded();
    if by_name {
        for n in &names {
            assert!(
                env.get_cab(&RawStr::from(n.as_str()), &asker)
                    .unwrap()
                    .is_some()
            );
        }
    } else {
        env.load(bytes("b.bundle", &b)).unwrap();
        let found = env.get_cab(&RawStr::from("INNER"), &asker);
        assert!(found.unwrap().is_none(), "INNER is a container");
    }
    meter.decoded() - before
}

/// The steps `pending.rs` counts for listing `m` slots one after another
/// and then settling them: each slot is listed on a list of `k` (`k` from 0
/// to `m - 1`), found by a search of a list of `n` and taken off it (`n`
/// from `m` down to 1), and the settle's last search finds an empty list;
/// a search of `n` counts ⌈log2 n⌉ + 1 steps, of none one. Worked out here
/// from that rule, not from the code's own `search_steps`.
fn design_steps(m: u64) -> u64 {
    let search = |n: u64| match n {
        0 => 1,
        n => u64::from(n.next_power_of_two().trailing_zeros()) + 1,
    };
    let listed: u64 = (0..m).map(search).sum();
    let settled: u64 = (1..=m).map(search).sum();
    listed + 2 * settled + 1
}

/// Settling the slots a registration dropped takes O(m log m) steps for m
/// slots, each counted as work of the slots' input (plan section 3.16, R5;
/// review round 6: the list was a `Vec` whose front removal shifted every
/// later slot, O(m²) steps that nothing counted). The steps are what
/// settling m slots counts past reaching each by name (`reach_work`): at
/// least every step of the design ([`design_steps`]: each slot's listing,
/// its search and its removal, review round 7: a floor of one step per
/// operation let the settle's own searches and removals go uncounted), and
/// at most ⌈log2 m⌉ + 1 for each of the three and for the last search, at
/// 8 bytes a step (`pending.rs`). That the steps are what the lists take,
/// a B-tree's, is `pending.rs`'s own test.
#[test]
fn settling_dropped_slots_counts_steps_that_grow_as_m_log_m() {
    for m in [64_u64, 256, 1024] {
        let n = usize::try_from(m).unwrap();
        let (settled, by_name) = (reach_work(n, false), reach_work(n, true));
        let steps = settled.checked_sub(by_name).expect("settling counts less") / 8;
        let log = u64::from((m - 1).bit_width()) + 1;
        let design = design_steps(m);
        assert!(
            steps >= design,
            "m = {m}: {steps} steps counted of {design}"
        );
        assert!(steps <= (3 * m + 1) * log, "m = {m}: {steps} steps");
    }
}

/// The steps `pending.rs` counts for listing `m` slots one after another,
/// worked out from its rule (a search of `n` counts ⌈log2 n⌉ + 1, of none
/// one), not from the code's own `search_steps`.
fn listing_steps(m: u64) -> u64 {
    (0..m).map(search).sum()
}

fn search(n: u64) -> u64 {
    match n {
        0 => 1,
        n => u64::from(n.next_power_of_two().trailing_zeros()) + 1,
    }
}

/// The dependency `a.bundle` of `m` raw entries `E00000`.. (one 160-byte
/// block each) and a last, `A-KEEP`; then the dependency `b.bundle`, whose
/// nested `INNER` holds entries of the same `m` names, so that a lookup of
/// `INNER` reaches it and drops all `m` of `a.bundle`'s slots unreached at
/// once, under `LoadMode::Lazy`. With `evicted`, a cache that holds one
/// dependency has evicted `a.bundle` by then (loading `b.bundle`), so the
/// lookup's `settle` moves each dropped slot to the list only the container
/// pass reaches (`defer`, plan section 9, Q13), and a lookup of `A-KEEP`
/// opens `a.bundle` again before the pass, which reaches them; without it,
/// the `settle` reaches each, after a lookup of `A-KEEP` as well, so that
/// the open that reads the `m` entries has read the same before them (a
/// bundle's read counts its searches of what it has read, plan section
/// 3.6). The work `a.bundle`'s first open counted across the lookup of
/// `INNER`, and the work its reopen counted across the container pass.
fn dropped_work(m: usize, evicted: bool) -> (u64, u64) {
    let names: Vec<String> = (0..m).map(|i| format!("E{i:05}")).collect();
    let entries = |data: &dyn Fn() -> Vec<u8>| -> Vec<(&str, Vec<u8>)> {
        names.iter().map(|n| (n.as_str(), data())).collect()
    };
    let mut ours = entries(&|| res_bytes(160, 7));
    ours.push(("A-KEEP", res_bytes(160, 8)));
    let a = Bytes::from(bundle(&ours));
    let b = Bytes::from(bundle(&[("INNER", bundle(&entries(&|| res_bytes(8, 9))))]));
    let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    o.dep_cache_bytes = if evicted { 1 } else { u64::MAX };
    let env = Environment::new(o);
    let first = env
        .load_dependency(bytes("a.bundle", &a))
        .unwrap()
        .meter()
        .clone();
    drop(env.load_dependency(bytes("b.bundle", &b)).unwrap());
    let asker = Meter::unlimited();
    let kept = || env.get_cab(&RawStr::from("A-KEEP"), &asker).unwrap();
    let mut keep = (!evicted).then(kept);
    let before = first.decoded();
    let found = env.get_cab(&RawStr::from("INNER"), &asker);
    assert!(found.unwrap().is_none(), "INNER is a container");
    let dropped = first.decoded() - before;
    // `a.bundle` open again, held while the pass runs.
    let keep = keep.get_or_insert_with(kept);
    let Some(Cab::Raw(RawCab::InBundle { bundle: open, .. })) = keep else {
        panic!("A-KEEP is a raw entry of a.bundle: {keep:?}");
    };
    assert_eq!(!Arc::ptr_eq(open.meter(), &first), evicted, "a reopen");
    let before = open.meter().decoded();
    let _ = env.container().unwrap().count();
    (dropped, open.meter().decoded() - before)
}

/// The steps that move a dropped slot of an evicted dependency to the list
/// only the container pass reaches (plan section 9, Q13) count as its
/// input's work, every designed step (plan section 9, "How a test sees
/// bounded work"; review round 8: `defer`'s count was pinned by no test).
/// With `a.bundle` evicted, the lookup that drops its `m` slots counts on
/// its first open exactly: each slot's listing (a search of the list of the
/// `k` listed before it), and for each in turn the settle's search and its
/// removal from the list of the `n` still listed (`n` from `m` down to 1)
/// and its insertion into the list of the `e` moved before it (`e` from 0
/// to `m - 1`), and the last search of the empty list; it reads nothing.
/// The container pass then counts on the reopen, for each slot, what a
/// settle that reaches a listed slot counts past its listing: a search and
/// a removal of the list it is on, and the read of its first bytes; so the
/// pass counts what the settle without the eviction counts, but for the
/// listings. A step is 8 bytes (`pending.rs`).
#[test]
fn moving_a_dropped_slot_of_an_evicted_dependency_counts_every_step() {
    for m in [64_u64, 256, 1024] {
        let n = usize::try_from(m).unwrap();
        let ((deferred, pass), (settled, _)) = (dropped_work(n, true), dropped_work(n, false));
        let moved: u64 = (1..=m).map(|n| 2 * search(n)).sum::<u64>() + listing_steps(m) + 1;
        assert_eq!(
            deferred,
            8 * (listing_steps(m) + moved),
            "m = {m}: the drop and the moves"
        );
        assert_eq!(pass, settled - 8 * listing_steps(m), "m = {m}: the pass");
    }
}
