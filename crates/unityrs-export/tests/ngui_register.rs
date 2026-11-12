//! Registration as the engine drives it (`docs/plans/batch.md` 2.6 steps 2
//! to 4; review round 2's S1, S2 and T2, round 3's P4 and T2 to T5): the
//! own open, a slot for step 3, each member's load handed to the engine and
//! its rows reached with bits loaded at that reach, X last; the checks
//! before X's open; the peek pass's hooks; the eviction bound as X's own
//! bare reason, at registration and in the cut; and the header the two
//! readers read differently.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use unityrs_core::budget::{Counting, Reservation, Unbounded};
use unityrs_core::env::EnvOptions;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_export::ngui::{
    self, Admit, Bits, Context, Data, Failure, Hooks, KeyTable, Member, NoHooks, Output, PeekBits,
    Registration, Staged, View,
};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, build};
use unityrs_testkit::ngui::{
    AtlasScene, failure_scenes, file, format, mono_script, scenes, sheet, texture,
};
use unityrs_testkit::scratch::Scratch;

mod common;
use common::{Engine, Refuse, big_sheet, core_call, options, run_with, view_of};

fn scene(name: &str) -> AtlasScene {
    scenes()
        .unwrap()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

/// Peeked bits, served at each reach as the engine's peek file is: copied
/// into a buffer charged to the meter it is asked for, under the probe.
struct Peeked(Vec<(String, Bits)>);

impl PeekBits for Peeked {
    fn load(&self, id: &str, meter: &Arc<Meter>, hooks: &dyn Hooks) -> Result<Bits, Failure> {
        let (_, bits) = self.0.iter().find(|(i, _)| i == id).expect("peeked");
        Bits::copied(bits, meter, hooks)
    }
}

fn peek_meter(context: &FileContext) -> Arc<Meter> {
    Meter::for_input(&context.limits, Arc::new(Unbounded), Reservation::Input)
}

/// A hook that stops at its `at`-th check.
struct StopAt(u64, std::cell::Cell<u64>);

impl Hooks for StopAt {
    fn stopped(&self) -> bool {
        let n = self.1.get();
        self.1.set(n + 1);
        n >= self.0
    }
}

/// What a cut gave, as the two routes are compared: each output's key and
/// MD5, every skip row, the sheets and the nested containers.
type Record = (
    Vec<(String, [u8; 16])>,
    Vec<ngui::Skip>,
    Vec<ngui::SheetRow>,
    Vec<RawStr>,
);

#[test]
fn a_view_registered_from_staged_paths_cuts_as_from_bytes() {
    // Every scene's every input (review round 3's T3): the staged route's
    // records are the bytes route's, and some of them hold skip rows and
    // nested containers to compare. Each member's peek is the same from its
    // file as from its bytes, and a view whose staged peek fails is
    // excluded only once the bytes route is seen to fail it at that member
    // (review round 4's T1), by that peek's own reason (round 5's T2): the
    // failure scenes are among them, so a member's peek fails.
    let dir = Scratch::new("ngui-staged");
    let (mut compared, mut skips, mut nested, mut deps) = (0, 0, 0, 0);
    let mut members = 0;
    for (name, s) in scenes()
        .unwrap()
        .into_iter()
        .chain(failure_scenes().unwrap())
    {
        for input in &s.inputs {
            let bytes = run_with(&s, input, FileContext::default(), &Meter::unlimited());
            let staged = match staged_run(&s, input, &dir) {
                Route::Cut(r) => r,
                Route::Peek(id, f) => {
                    let Err(a) = bytes else {
                        panic!("{name}/{input}: {id}'s peek failed staged only: {f:?}");
                    };
                    // A member's: its own peek's reason inside, and its
                    // detail (review round 5's T2).
                    match (&a, &f) {
                        _ if id == *input => assert_eq!(a, f, "{name}/{input}"),
                        (Failure::Input(r), Failure::Input(inner)) => {
                            let want = format!("dependency_failed({id}, {})", inner.reason);
                            assert_eq!(r.reason, want, "{name}/{input}");
                            assert_eq!(r.detail, inner.detail, "{name}/{input}");
                            members += 1;
                        }
                        _ => panic!("{name}/{input}: {a:?} against {f:?}"),
                    }
                    continue;
                }
            };
            match (bytes, staged) {
                (Ok(b), Ok(r)) => {
                    let md5s = b.outputs.iter().map(|o| (o.key.clone(), o.md5)).collect();
                    let want: Record = (md5s, b.skips, b.sheets, b.nested_containers);
                    skips += usize::from(!want.1.is_empty());
                    nested += usize::from(!want.3.is_empty());
                    deps += usize::from(view_of(&s, input).len() > 1);
                    assert_eq!(r, want, "{name}/{input}");
                }
                (Err(a), Err(b)) => assert_eq!(a, b, "{name}/{input}"),
                (a, b) => panic!("{name}/{input}: {a:?} against {b:?}"),
            }
            compared += 1;
        }
    }
    assert!(compared > 30, "{compared}");
    assert!(
        skips > 0 && nested > 0 && deps > 0 && members > 0,
        "{skips} {nested} {deps} {members}"
    );
}

/// What the staged route gave: a cut, or the first peek that failed (by
/// the bytes route's order, X then the members by id).
enum Route {
    Cut(Result<Record, Failure>),
    Peek(String, Failure),
}

/// A cut of `input` registered from staged paths, as the engine stages a
/// view: each bundle written to `dir` and peeked from its file (the same
/// as from its bytes), then [`ngui::register`] over the paths; or the
/// first peek that fails (the engine fails such a view before its attempt).
fn staged_run(s: &AtlasScene, input: &str, dir: &Scratch) -> Route {
    staged_run_in(s, input, dir, &Meter::unlimited(), &NoHooks)
}

/// `m`'s peek from its staged file at `path`, asserted the same as its peek
/// from its bytes (rows and bits, or the failure).
fn peek_both(
    m: &Member,
    name: &RawStr,
    path: &std::path::Path,
    input: &str,
) -> Result<Bits, Failure> {
    let context = Arc::new(FileContext::default());
    let meter = || peek_meter(&context);
    let staged = ngui::peek(name, Data::Path(path), &context, &meter(), &NoHooks);
    let id = RawStr::from(m.id.as_str());
    let bytes = ngui::peek(&id, Data::Bytes(&m.data), &context, &meter(), &NoHooks);
    let shape = |p: &Result<Bits, Failure>| {
        p.as_ref()
            .map(|b| (b.rows(), b.bytes().to_vec()))
            .map_err(Clone::clone)
    };
    assert_eq!(shape(&staged), shape(&bytes), "{input}: {}'s peek", m.id);
    staged
}

/// [`staged_run`] with `M_X` `m` and the engine's `hooks`.
fn staged_run_in(
    s: &AtlasScene,
    input: &str,
    dir: &Scratch,
    m: &Arc<Meter>,
    hooks: &dyn Hooks,
) -> Route {
    let mut members = view_of(s, input);
    members.sort_by(|a, b| (a.id != input, a.id.as_bytes()).cmp(&(b.id != input, b.id.as_bytes())));
    let context = FileContext::default();
    let mut files = Vec::new();
    let mut peeked = Vec::new();
    for m in &members {
        let path = dir.join(format!("{input}-{}", m.id));
        std::fs::write(&path, &m.data).unwrap();
        let name = RawStr::from(path.to_string_lossy().as_ref());
        match peek_both(m, &name, &path, input) {
            Ok(bits) => peeked.push((m.id.clone(), bits)),
            Err(f) => return Route::Peek(m.id.clone(), f),
        }
        files.push((m.id.clone(), name, path));
    }
    let staged: Vec<Staged<'_>> = files
        .iter()
        .map(|(id, name, path)| Staged {
            id,
            name,
            data: Data::Path(path),
        })
        .collect();
    let x = *staged.iter().find(|m| m.id == input).unwrap();
    let env = EnvOptions::batch(Arc::new(context));
    let admit = Admit {
        env: &env,
        meter: m,
        hooks,
        max_evictions: u64::MAX,
    };
    let loaded = RefCell::new(Vec::new());
    let registered = ngui::register(x, &staged, &Peeked(peeked), admit, &mut |id, b| {
        loaded
            .borrow_mut()
            .push((id.to_owned(), b.directory().len()));
        Ok(())
    });
    let registered = match registered {
        Ok(r) => r,
        Err(f) => return Route::Cut(Err(f)),
    };
    // The members in bytewise order of id, X last, each handed over loaded.
    let ids: Vec<String> = loaded.borrow().iter().map(|(i, _)| i.clone()).collect();
    let mut want: Vec<String> = members
        .iter()
        .map(|m| m.id.clone())
        .filter(|i| i != input)
        .collect();
    want.sort();
    want.push(input.to_owned());
    assert_eq!(ids, want);
    assert!(loaded.borrow().iter().all(|(_, n)| *n > 0));
    let mut table = KeyTable::new(m);
    let mut outs: Vec<Output> = Vec::new();
    let mut sink = |o: Output| {
        outs.push(o);
        Ok(())
    };
    let ctx = Context {
        registered: &registered,
        meter: m,
        input,
        hooks,
        max_evictions: u64::MAX,
        prefix: "",
        members: &[],
        keys: &mut table,
        sink: &mut sink,
    };
    let cut = match ngui::cut(ctx, &options(s)) {
        Ok(c) => c,
        Err(f) => return Route::Cut(Err(f)),
    };
    let md5s = outs.iter().map(|o| (o.key.clone(), o.md5)).collect();
    let mut rows = cut.plan_skips.clone();
    rows.extend(cut.export_skips.iter().cloned());
    Route::Cut(Ok((
        md5s,
        rows,
        cut.sheets.clone(),
        cut.nested_containers.clone(),
    )))
}

#[test]
fn each_refusal_no_budget_counted_on_the_staged_route_is_host_memory() {
    // Review round 4's T2: the staged route, each reservation of M_X refused
    // in turn with the attempt budget's count held still. Every result is
    // the whole cut or the stop `host_memory`, and the refusals reach X's
    // own read of its file (`read_path`), which the bytes route never makes.
    let s = scene("streamed_sheets");
    let dir = Scratch::new("ngui-staged-refusals");
    let Route::Cut(Ok(full)) = staged_run(&s, "x.bundle", &dir) else {
        panic!("the whole cut");
    };
    let mut reads = 0;
    for at in 0.. {
        assert!(at < 1 << 16);
        let refuse = Arc::new(Refuse::new(at));
        let engine = Engine {
            refusals: Some(Arc::new(AtomicU64::new(0))),
            ..Engine::default()
        };
        let m = Meter::new(u64::MAX, refuse.clone(), Reservation::Input);
        let Route::Cut(got) = staged_run_in(&s, "x.bundle", &dir, &m, &engine) else {
            panic!("{at}: a peek failed");
        };
        let Some(site) = *refuse.site.lock().unwrap() else {
            assert_eq!(got.as_ref().ok(), Some(&full));
            break;
        };
        let call = core_call(&refuse.stacks.lock().unwrap()[0]);
        match got {
            Ok(r) => assert_eq!(r, full, "{at} ({site})"),
            Err(Failure::Run(r)) if r.reason == "host_memory" => {
                reads += usize::from(call.ends_with("read_path"));
            }
            Err(f) => panic!("{at} ({site}, {call}): {f:?}"),
        }
    }
    assert!(reads > 0, "no refusal in read_path");
}

#[test]
fn the_engine_drives_each_step_in_order() {
    // Step 2's own open, a slot for step 3 (the own handle), each member,
    // then X; a member out of bytewise order of id is the cutter's fault.
    let s = scene("material_in_dependency");
    let members = view_of(&s, "x.bundle");
    let context = Arc::new(FileContext::default());
    let names: Vec<RawStr> = members
        .iter()
        .map(|m| RawStr::from(m.id.as_str()))
        .collect();
    let staged: Vec<Staged<'_>> = members
        .iter()
        .zip(&names)
        .map(|(m, name)| Staged {
            id: &m.id,
            name,
            data: Data::Bytes(&m.data),
        })
        .collect();
    let peeked = Peeked(
        staged
            .iter()
            .map(|m| {
                let bits = ngui::peek(m.name, m.data, &context, &peek_meter(&context), &NoHooks);
                (m.id.to_owned(), bits.unwrap())
            })
            .collect(),
    );
    let x = *staged.iter().find(|m| m.id == "x.bundle").unwrap();
    let mut deps: Vec<&Staged<'_>> = staged.iter().filter(|m| m.id != "x.bundle").collect();
    deps.sort_by(|a, b| a.id.cmp(b.id));
    assert!(!deps.is_empty());
    let m = Meter::unlimited();
    let env = EnvOptions::batch(context.clone());
    let admit = Admit {
        env: &env,
        meter: &m,
        hooks: &NoHooks,
        max_evictions: u64::MAX,
    };
    let ids: Vec<&str> = deps.iter().map(|d| d.id).collect();
    let mut reg = Registration::open(x, &ids, admit).unwrap();
    assert!(
        !reg.own().directory().is_empty(),
        "step 3 reads the own handle"
    );
    let mut seen = Vec::new();
    for d in &deps {
        reg.member(d, &peeked, &mut |id, _| {
            seen.push(id.to_owned());
            Ok(())
        })
        .unwrap();
    }
    reg.finish(&peeked, &mut |id, _| {
        seen.push(id.to_owned());
        Ok(())
    })
    .unwrap();
    let mut want: Vec<String> = deps.iter().map(|d| d.id.to_owned()).collect();
    want.push("x.bundle".to_owned());
    assert_eq!(seen, want);
    // Out of order (review round 3's P4 and T5): a member not the next of
    // the ids, and X's finish before every member.
    let internal = |got: Result<(), Failure>| {
        assert!(matches!(got, Err(Failure::Run(r)) if r.reason == "internal"));
    };
    let mut ids = ids.clone();
    ids.insert(0, "a.bundle");
    let mut reg = Registration::open(x, &ids, admit).unwrap();
    internal(reg.member(deps[0], &peeked, &mut |_, _| Ok(())));
    let reg = Registration::open(x, &ids, admit).unwrap();
    internal(reg.finish(&peeked, &mut |_, _| Ok(())).map(|_| ()));
}

#[test]
fn the_ids_are_checked_before_xs_open() {
    // Review round 3's P4: `Registration::open` takes the members' ids and
    // refuses, before X's open (X's bytes here open as nothing), a smaller
    // id after a larger one, a repeated id, and X's own id among them.
    let x = garbage("x.bundle");
    let name = RawStr::from("x.bundle");
    let staged = Staged {
        id: "x.bundle",
        name: &name,
        data: Data::Bytes(&x.data),
    };
    let env = EnvOptions::batch(Arc::new(FileContext::default()));
    let m = Meter::unlimited();
    let admit = Admit {
        env: &env,
        meter: &m,
        hooks: &NoHooks,
        max_evictions: u64::MAX,
    };
    for ids in [
        &["b.bundle", "a.bundle"][..],
        &["a.bundle", "a.bundle"],
        &["a.bundle", "x.bundle"],
    ] {
        let got = Registration::open(staged, ids, admit).map(|_| ());
        assert!(
            matches!(&got, Err(Failure::Run(r)) if r.reason == "internal"),
            "{ids:?}: {got:?}"
        );
    }
    // In order, the open is reached, and X's bytes fail it.
    let got = Registration::open(staged, &["a.bundle", "b.bundle"], admit).map(|_| ());
    assert!(matches!(got, Err(Failure::Input(_))), "{got:?}");
}

fn garbage(id: &str) -> Member {
    Member {
        id: id.to_owned(),
        data: Bytes::from_static(b"not a bundle at all, nothing to open"),
    }
}

fn register_view(x: &Member, members: &[Member], hooks: &dyn Hooks) -> Result<(), Failure> {
    let view = View { input: x, members };
    let env = EnvOptions::batch(Arc::new(FileContext::default()));
    let admit = Admit {
        env: &env,
        meter: &Meter::unlimited(),
        hooks,
        max_evictions: u64::MAX,
    };
    ngui::register_bytes(view, admit).map(|_| ())
}

#[test]
fn ids_and_the_stop_are_checked_before_xs_open() {
    // X's bytes open as nothing: a repeated id, or a stop at the first
    // check, answers before X's open would fail it.
    let x = garbage("x.bundle");
    let dep = garbage("d.bundle");
    let got = register_view(&x, &[x.clone(), dep.clone(), dep.clone()], &NoHooks);
    assert!(matches!(got, Err(Failure::Run(r)) if r.reason == "internal"));
    let s = big_sheet();
    let members = view_of(&s, "x.bundle");
    let good = members.iter().find(|m| m.id == "x.bundle").unwrap();
    let mut with_bad = members.clone();
    with_bad.push(dep.clone());
    // The peek of X checks the stop between its entries, before the bad
    // member is peeked.
    let stop = StopAt(0, std::cell::Cell::new(0));
    let got = register_view(good, &with_bad, &stop);
    assert_eq!(got, Err(Failure::Stopped));
    // Registration's own open checks the stop first.
    let x_name = RawStr::from("x.bundle");
    let env = EnvOptions::batch(Arc::new(FileContext::default()));
    let admit = Admit {
        env: &env,
        meter: &Meter::unlimited(),
        hooks: &StopAt(0, std::cell::Cell::new(0)),
        max_evictions: u64::MAX,
    };
    let staged = Staged {
        id: "x.bundle",
        name: &x_name,
        data: Data::Bytes(&x.data),
    };
    assert!(matches!(
        Registration::open(staged, &[], admit),
        Err(Failure::Stopped)
    ));
}

#[test]
fn xs_own_peek_failure_comes_before_any_members() {
    // 2.3's rule 1: X's peek failure first, then the member of least id.
    let x = garbage("x.bundle");
    let got = register_view(&x, &[x.clone(), garbage("a.bundle")], &NoHooks);
    let Err(Failure::Input(r)) = got else {
        panic!("{got:?}");
    };
    assert!(!r.reason.starts_with("dependency_failed"), "{r:?}");
    let s = big_sheet();
    let good = view_of(&s, "x.bundle").remove(0);
    let members = [good.clone(), garbage("b.bundle"), garbage("a.bundle")];
    let got = register_view(&good, &members, &NoHooks);
    let Err(Failure::Input(r)) = got else {
        panic!("{got:?}");
    };
    assert!(
        r.reason.starts_with("dependency_failed(a.bundle, "),
        "{r:?}"
    );
}

#[test]
fn a_peek_stops_between_entries_and_reads_a_budget_refusal_as_host_memory() {
    // A bundle of two rows (a CAB and its `.resS`).
    let s = scene("wide_texture_fields");
    let x = view_of(&s, "x.bundle")
        .into_iter()
        .find(|m| m.id == "x.bundle")
        .unwrap();
    let context = Arc::new(FileContext::default());
    let name = RawStr::from("x.bundle");
    let ok = ngui::peek(
        &name,
        Data::Bytes(&x.data),
        &context,
        &Meter::unlimited(),
        &NoHooks,
    )
    .unwrap();
    assert_eq!(ok.rows(), 2);
    assert_eq!(ok.bytes().len(), ok.rows().div_ceil(8) as usize);
    // A check before each entry (review round 3's T5): stopped at the
    // second, after one entry, and never at the last but one.
    let peek = |stop: &StopAt| {
        ngui::peek(
            &name,
            Data::Bytes(&x.data),
            &context,
            &Meter::unlimited(),
            stop,
        )
    };
    let stop = StopAt(1, std::cell::Cell::new(0));
    assert!(matches!(peek(&stop), Err(Failure::Stopped)));
    assert_eq!(stop.1.get(), 2);
    let stop = StopAt(2, std::cell::Cell::new(0));
    assert!(peek(&stop).is_ok());
    assert_eq!(stop.1.get(), 2);
    // Each budget's refusal on the peek's meter is the allocator's: the
    // peek's limit is its meter's bound, checked first.
    let mut seen = false;
    for n in (0..1 << 20).step_by(64) {
        let meter = Meter::new(u64::MAX, Arc::new(Counting::new(n)), Reservation::Input);
        match ngui::peek(&name, Data::Bytes(&x.data), &context, &meter, &NoHooks) {
            Ok(_) => break,
            Err(Failure::Run(r)) => {
                assert_eq!(r.reason, "host_memory");
                seen = true;
            }
            Err(f) => panic!("{n}: {f:?}"),
        }
    }
    assert!(seen);
}

#[test]
fn an_eviction_during_registration_is_xs_own_bare_limit() {
    // A view of three bundles under a dependency cache `D` that holds one
    // at a time: a member's load evicts another, and with no eviction
    // allowed X fails with its own bare reason, never wrapped in the
    // member's `dependency_failed` (review round 2's S2). Every D's answer
    // is checked, not only the first that fails.
    let (name, s) = scenes()
        .unwrap()
        .into_iter()
        .find(|(_, s)| s.inputs.iter().any(|i| view_of(s, i).len() >= 3))
        .expect("a view of three bundles");
    let input = s
        .inputs
        .iter()
        .find(|i| view_of(&s, i).len() >= 3)
        .unwrap()
        .clone();
    let members = view_of(&s, &input);
    let x = members.iter().find(|m| m.id == input).unwrap();
    let mut seen = 0;
    for d in (0..1 << 16).step_by(64) {
        let engine = Engine {
            dep_cache_bytes: Some(d),
            max_evictions: Some(0),
            ..Engine::default()
        };
        let env = engine.env(FileContext::default());
        let view = View {
            input: x,
            members: &members,
        };
        let admit = Admit {
            env: &env,
            meter: &Meter::unlimited(),
            hooks: &engine,
            max_evictions: 0,
        };
        match ngui::register_bytes(view, admit) {
            Ok(_) => {}
            Err(Failure::Input(r)) if r.reason.contains("attempt_evictions") => {
                assert_eq!(r.reason, "limit(attempt_evictions, 0)", "{name} at D {d}");
                assert_eq!(r.detail, None);
                seen += 1;
            }
            Err(f) => panic!("{name} at D {d}: {f:?}"),
        }
    }
    assert!(seen > 0, "{name}: no load evicted");
}

#[test]
fn a_header_the_two_readers_read_differently_fails_x() {
    // A `version_player` of 32,767 bytes with no NUL after it: the memory
    // reader (X's slot) returns all 32,767 bytes, the stream reader (X's
    // own open) all but the last, and both go on from the same place. The
    // registered header is not the own handle's, so X fails,
    // `container_error`, detail `malformed` (review round 2's T2).
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            texture(2, b"t", format::RGBA32, (1, 1), sheet(0x10, 1, 1, false)),
        ],
        true,
    )
    .unwrap();
    let spec = BundleSpec {
        format: 6,
        version_player: vec![b'a'; 32_767],
        block_size: 160,
        entries: vec![EntrySpec {
            name: b"CAB-xh".to_vec(),
            data: cab,
            flags: 4,
        }],
        ..BundleSpec::default()
    };
    let mut raw = build(&spec);
    let nul = b"UnityFS\0".len() + 4 + 32_767;
    assert_eq!(raw[nul], 0, "the string's NUL");
    raw.remove(nul);
    let x = Member {
        id: "x.bundle".to_owned(),
        data: Bytes::from(raw),
    };
    let got = register_view(&x, std::slice::from_ref(&x), &NoHooks);
    let Err(Failure::Input(r)) = got else {
        panic!("{got:?}");
    };
    assert_eq!(
        (r.reason.as_str(), r.detail.as_deref()),
        ("container_error", Some("malformed"))
    );
    // The stage (review round 3's T5): the stream reader's own open and
    // peek take it, and the comparison at X's registration fails it.
    let context = Arc::new(FileContext::default());
    let name = RawStr::from("x.bundle");
    let data = Data::Bytes(&x.data);
    let bits = ngui::peek(&name, data, &context, &peek_meter(&context), &NoHooks).unwrap();
    let env = EnvOptions::batch(context);
    let m = Meter::unlimited();
    let admit = Admit {
        env: &env,
        meter: &m,
        hooks: &NoHooks,
        max_evictions: u64::MAX,
    };
    let staged = Staged {
        id: "x.bundle",
        name: &name,
        data,
    };
    let reg = Registration::open(staged, &[], admit).expect("the own open takes it");
    let peeked = Peeked(vec![("x.bundle".to_owned(), bits)]);
    let got = reg.finish(&peeked, &mut |_, _| Ok(())).map(|_| ());
    let Err(Failure::Input(r)) = got else {
        panic!("{got:?}");
    };
    assert_eq!(r.detail.as_deref(), Some("malformed"));
}

#[test]
fn an_eviction_in_the_cut_is_xs_own_bare_limit() {
    // Review round 3's T2: registration's evictions counted, the cut is
    // allowed exactly those, and a dependency cache `D` at which the cut's
    // own Environment calls (a dereference, a resource) evict again fails X
    // with its bare limit at the check after that call.
    let mut seen = 0;
    for (name, s) in scenes().unwrap() {
        for input in &s.inputs {
            let members = view_of(&s, input);
            if members.len() < 2 {
                continue;
            }
            let x = members.iter().find(|m| m.id == *input).unwrap();
            for d in (0..1 << 15).step_by(256) {
                let engine = Engine {
                    dep_cache_bytes: Some(d),
                    ..Engine::default()
                };
                let env = engine.env(FileContext::default());
                let m = Meter::unlimited();
                let admit = Admit {
                    env: &env,
                    meter: &m,
                    hooks: &engine,
                    max_evictions: u64::MAX,
                };
                let view = View {
                    input: x,
                    members: &members,
                };
                let Ok(registered) = ngui::register_bytes(view, admit) else {
                    continue;
                };
                let at_registration = registered.env.stats().dependencies_evicted;
                let mut table = KeyTable::new(&m);
                let mut sink = |_: Output| Ok(());
                let ctx = Context {
                    registered: &registered,
                    meter: &m,
                    input,
                    hooks: &engine,
                    max_evictions: at_registration,
                    prefix: "",
                    members: &[],
                    keys: &mut table,
                    sink: &mut sink,
                };
                let got = ngui::cut(ctx, &options(&s));
                let after = registered.env.stats().dependencies_evicted;
                if after == at_registration {
                    assert!(got.is_ok(), "{name}/{input} at D {d}: {got:?}");
                    continue;
                }
                let Err(Failure::Input(r)) = got else {
                    panic!("{name}/{input} at D {d}: {got:?}");
                };
                let want = format!("limit(attempt_evictions, {at_registration})");
                assert_eq!((r.reason, r.detail), (want, None), "{name}/{input}");
                // The check follows the call that evicted: one past.
                assert_eq!(after, at_registration + 1, "{name}/{input} at D {d}");
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "no cut's call evicted");
}

#[test]
fn the_comparison_is_of_the_header_and_the_directory() {
    // `same_open` on two opens of one bundle: equal.
    let s = big_sheet();
    let x = view_of(&s, "x.bundle").remove(0);
    let context = Arc::new(FileContext::default());
    let env = EnvOptions::batch(context);
    let admit = Admit {
        env: &env,
        meter: &Meter::unlimited(),
        hooks: &NoHooks,
        max_evictions: u64::MAX,
    };
    let name = RawStr::from("x.bundle");
    let staged = Staged {
        id: "x.bundle",
        name: &name,
        data: Data::Bytes(&x.data),
    };
    let a = Registration::open(staged, &[], admit).unwrap();
    let b = Registration::open(staged, &[], admit).unwrap();
    assert_eq!(ngui::same_open(a.own(), b.own()), Ok(()));
}
