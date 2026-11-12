//! The `sprites` kind under the engine's bounds (`docs/BATCH_SPEC.md` 4.4,
//! 7.3; `docs/plans/batch.md` 2.6 steps 7 and 10; B3's review rounds 1 to
//! 3, R6, T1, T2 and T3 to T5): each reservation of `M_X` refused in turn,
//! counted and not, its scope worked out from its site and its change
//! found by each object's identity, against the unrefused plan's groups; a
//! decode-work bound met inside a render; a dependency's sheet held while
//! its group exports under a dependency cache that evicts, the eviction
//! seen during the export; one dependency file reached around an eviction
//! and its reopening, one group; and an image too wide for Pillow's
//! encoder skipped before its export work.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use unityrs_core::FileContext;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_export::ngui::{
    self, Admit, Context, Cut, Failure, KeyTable, Output, Registered, SkipOutput, View,
};
use unityrs_testkit::ngui::{
    AtlasScene, CutOptions, SceneBundle, SpriteSpec, file, format, sheet, sprite_scenes, texture,
};

mod common;
use common::{Asked, Engine, Ran, options, refused_in, refused_with_next, run_in, view_of};

fn scene(name: &str) -> AtlasScene {
    sprite_scenes()
        .unwrap()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

/// An object's identity: its SerializedFile, its path id and the output.
type Id = (String, i64, &'static str);

/// Each object's outcome by its identity: the key and MD5 it was handed
/// under, or its skip's reason.
type Outcomes = BTreeMap<Id, String>;

fn outcomes(ran: &Ran) -> Outcomes {
    let mut out = BTreeMap::new();
    for h in &ran.handed {
        let id = (h.object.0.clone(), h.object.1, h.kind.name());
        let was = out.insert(id.clone(), format!("out {} {:x?}", h.key, h.md5));
        assert_eq!(was, None, "{id:?}: one outcome");
    }
    for k in &ran.skips {
        let file = String::from_utf8_lossy(k.serialized_file.as_bytes()).into_owned();
        let id = (file, k.path_id, k.output.name());
        let was = out.insert(
            id.clone(),
            format!("skip {} {:?}", k.reason.reason, k.reason.detail),
        );
        assert_eq!(was, None, "{id:?}: one outcome");
    }
    out
}

/// `s`'s unrefused cut: each object's outcome, and the plan's sprite
/// groups, each its Sprites, as the export told them (`Hooks::sprite_group`).
fn planned(s: &AtlasScene) -> (Outcomes, Vec<BTreeSet<Id>>) {
    let engine = Engine::default();
    let ran = run_in(
        s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &engine,
    )
    .unwrap_or_else(|f| panic!("{f:?}"));
    let mut groups: Vec<BTreeSet<Id>> = Vec::new();
    for (g, file, path_id) in engine.groups.borrow().iter() {
        if groups.len() <= *g {
            groups.resize_with(g + 1, BTreeSet::new);
        }
        assert!(groups[*g].insert((file.clone(), *path_id, "sprite")));
    }
    (outcomes(&ran), groups)
}

/// What a refusal may change, worked out from its site alone (B3's review
/// round 3, T1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// The input: it fails, `budget(input)`, or the core absorbs it.
    Input,
    /// The members of the group whose sheet is exported.
    Group,
    /// One Sprite.
    Sprite,
}

/// A refusal's scope from its stack, the sites that record a class as the
/// object's: inside a Sprite's plan (`plan_sprite`), its name's read
/// (`peek_name`), its sheet's (`sheets`) and, inside that, an atlas name's
/// in its tag walk (`atlas_name`, B3's review round 5, Q6); inside its export
/// (`export_sprite`), its render (`render_sprite`), its PNG (`png`) and its
/// output's charge (`Attempt::output`, short of the sink's `emit`): the
/// Sprite's. Inside a group's export (`export_group`) but outside its
/// Sprites', its sheet's decode (`group_sheet`) and merge (`merged`): the
/// group's. Every other site (the cut's tables, `Attempt::charged` and
/// `Attempt::hold`, its skip rows, the key table) is the input's.
fn scope(stack: &str) -> Scope {
    let read = frame(stack, "peek_name") || frame(stack, "atlas_name") || frame(stack, "sheets");
    let plan = frame(stack, "plan_sprite") && read;
    let made = frame(stack, "output") && !frame(stack, "emit");
    let export = frame(stack, "export_sprite")
        && (frame(stack, "render_sprite") || frame(stack, "png") || made);
    let group = frame(stack, "export_group")
        && !frame(stack, "export_sprite")
        && (frame(stack, "group_sheet") || frame(stack, "merged"));
    if plan || export {
        Scope::Sprite
    } else if group {
        Scope::Group
    } else {
        Scope::Input
    }
}

/// Whether a frame of `stack` is the function `name` (a path's last
/// segment, or one before a closure's).
fn frame(stack: &str, name: &str) -> bool {
    stack.lines().any(|l| {
        let f = l.trim().split_once(": ").map_or("", |(_, f)| f);
        f.split("::").any(|seg| seg == name)
    })
}

/// `stack`'s frames of this crate, for a failure's message.
fn ours(stack: &str) -> String {
    let frames = stack
        .lines()
        .filter(|l| l.contains("unityrs_export::") && !l.trim().starts_with("at "));
    frames.collect::<Vec<_>>().join("\n")
}

/// The objects whose outcome `ran` changed from `whole`'s, each a Sprite
/// now skipped `budget(input)`; every object has one outcome still.
fn changed(whole: &Outcomes, ran: &Ran, tag: &str) -> BTreeSet<Id> {
    let got = outcomes(ran);
    let ids: BTreeSet<&Id> = got.keys().collect();
    assert_eq!(ids, whole.keys().collect(), "{tag}: every object, once");
    let changed: BTreeSet<Id> = whole
        .iter()
        .filter(|(k, v)| got.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();
    for k in &changed {
        assert_eq!(k.2, "sprite", "{tag}: {k:?}");
        assert_eq!(got[k], "skip budget(input) None", "{tag}: {k:?}");
    }
    changed
}

/// The core's fallbacks that absorb a refused reservation `asked` (B3's
/// review rounds 4 and 5, R5 and Q4): a vector's growth ladder, whose
/// refused doubling is asked for again smaller down to exactly what it
/// needs (`limits::vec::grow_counted`, behind every metered vector's
/// growth and a block's `DecodeBuf`), seen to retry: the reservation
/// `next`, granted, right after the refused one, has the same invocation
/// id from the core's test counters (a source location alone cannot tell
/// two calls from one loop site apart) and
/// asks for no more (a smaller capacity, which the heap's rounding can
/// cost the same: a small vector's 32 bytes), where a refusal at the size
/// it needs is returned, not absorbed, and its caller's next reservation
/// is another call's; and a decoded run the bundle returns uncached when
/// its index cannot grow (`BundleFile::publish`), by its frame.
fn absorbed(asked: &Asked, next: Option<&Asked>) -> bool {
    let (stack, bytes, growth) = asked;
    let retried = growth.is_some() && next.is_some_and(|(_, b, g)| b <= bytes && g == growth);
    retried
        || stack
            .lines()
            .any(|l| l.contains("unityrs_core::files::bundle::BundleFile::publish"))
}

/// The two requests recorded by a followed refusal.
fn requests(refuse: &common::Refuse) -> (Asked, Asked) {
    let stack = refuse.stacks.lock().unwrap().first().unwrap().clone();
    let asked = (
        stack,
        *refuse.bytes.lock().unwrap(),
        *refuse.growth.lock().unwrap(),
    );
    (asked, refuse.next.lock().unwrap().clone().unwrap())
}

#[test]
fn successive_growth_calls_from_one_loop_site_do_not_absorb_each_others_refusals() {
    let refuse = Arc::new(common::Refuse::followed(0));
    let meter = Meter::new(u64::MAX, refuse.clone(), Reservation::Input);
    let results: Vec<_> = (0..2)
        .map(|_| MeteredVec::<u8>::new(&meter).reserve_exact(4).is_ok())
        .collect();
    assert_eq!(results, [false, true]);
    let (asked, next) = requests(&refuse);
    assert_eq!(
        asked.1, next.1,
        "same-sized calls from the same source line"
    );
    assert!(asked.2.is_some());
    assert_ne!(asked.2, next.2, "different invocations");
    assert!(!absorbed(&asked, None), "an exact-size refusal returns");
    assert!(
        !absorbed(&asked, Some(&next)),
        "the next call did not retry the refused call"
    );
}

#[test]
fn a_granted_retry_in_the_refused_growth_call_is_absorbed() {
    // The small growth can have equal rounded heap charges for its two
    // capacities; the larger one has a strictly smaller retry charge.
    for capacity in [4, 64] {
        let refuse = Arc::new(common::Refuse::followed(1));
        let meter = Meter::new(u64::MAX, refuse.clone(), Reservation::Input);
        let mut v = MeteredVec::<u8>::new(&meter);
        v.reserve_exact(capacity).unwrap();
        v.reserve(v.capacity() + 1).unwrap();
        let (asked, next) = requests(&refuse);
        assert!(asked.2.is_some());
        assert_eq!(asked.2, next.2, "the ladder kept its invocation");
        assert!(absorbed(&asked, Some(&next)));
    }
}

/// The uncounted pass's stride: every `STRIDE`-th reservation refused (the
/// counted pass refuses each).
const STRIDE: u64 = 3;

/// The sweep's uncounted pass over `s` (every [`STRIDE`]-th reservation
/// refused, the allocator's): the cut ends `host_memory`, or the core
/// absorbs the refusal and the cut is `whole`.
fn uncounted(name: &str, s: &AtlasScene, whole: &Outcomes) {
    for at in (0..).step_by(usize::try_from(STRIDE).unwrap()) {
        let (site, got) = refused_in(s, "x.bundle", at, false);
        let tag = format!("{name} at {at} ({site:?}, uncounted)");
        match got {
            Ok(ran) => assert_eq!(outcomes(&ran), *whole, "{tag}"),
            Err(Failure::Run(r)) => assert_eq!(r.reason, "host_memory", "{tag}"),
            Err(f) => panic!("{tag}: {f:?}"),
        }
        if site.is_none() {
            break;
        }
        assert!(at < 1 << 20, "{tag}: the cut never ends");
    }
}

/// The scenes the sweep refuses each reservation of.
const SWEPT: [&str; 13] = [
    "own_sheets",
    "alpha_textures",
    "dependency_sheets",
    "atlas_lookup",
    "atlas_dependency",
    "nested_cabs",
    "render_failures",
    "sprite_classes",
    "atlas_classes",
    "shadowed_rows",
    "sprite_pointers",
    "sprite_attributes",
    "int_tags",
];

#[test]
fn each_refused_reservation_is_one_sprites_skip_or_its_groups() {
    // The `at`-th reservation of `M_X` refused, for every `at` until the cut
    // makes fewer (review round 1's T3, rounds 2 and 3's T1). Its scope is
    // worked out first, from its site alone ([`scope`]); then, counted (the
    // budget's): a Sprite's refusal (inside one Sprite's plan or render)
    // changes the outcome of that one Sprite to `budget(input)`; a group's
    // (inside the group's sheet) changes exactly the members of one of the
    // groups the unrefused plan made ([`planned`]); an input's fails the
    // input, `budget(input)`, which only such a site may; and any of them
    // may be absorbed by one of the core's named fallbacks ([`absorbed`]),
    // nothing changed. A refusal that changes nothing anywhere else was
    // swallowed, and there are none (review round 4, R5). Every object has
    // one outcome, whatever is refused.
    // Uncounted (the allocator's), the cut ends `host_memory`, or the core
    // absorbs it and the cut is the whole run. `render_failures` holds a
    // texture answer two Sprites share (`unsupported_format`, kept charged
    // in the attempt's cache): a refusal of its charge is the one Sprite's
    // that asked, and the next asks again.
    let started = Instant::now();
    let mut singles = 0;
    let mut grouped = 0;
    let mut absorbed = 0;
    let mut swallowed = Vec::new();
    let mut inputs = 0;
    let mut atlas_names = 0;
    for name in SWEPT {
        let s = scene(name);
        let (whole, plan) = planned(&s);
        assert!(!plan.is_empty(), "{name}: no group planned");
        let mut refused_groups = BTreeSet::new();
        for at in 0.. {
            let (site, asked, next, got) = refused_with_next(&s, "x.bundle", at);
            let stack = &asked.0;
            let Some(site) = site else {
                let ran = got.unwrap_or_else(|f| panic!("{name}: {f:?}"));
                assert_eq!(outcomes(&ran), whole, "{name}: nothing refused");
                break;
            };
            let scope = scope(stack);
            if site == "atlas_name" {
                assert_eq!(
                    scope,
                    Scope::Sprite,
                    "{name} at {at}: the tag walk's name read"
                );
                atlas_names += 1;
            }
            let tag = format!("{name} at {at} ({site}, {scope:?}, counted)");
            let ran = match got {
                Ok(ran) => ran,
                Err(Failure::Input(r)) => {
                    assert_eq!(scope, Scope::Input, "{tag}: {stack}");
                    assert_eq!(r.reason, "budget(input)", "{tag}");
                    inputs += 1;
                    continue;
                }
                Err(f) => panic!("{tag}: {f:?}"),
            };
            let changed = changed(&whole, &ran, &tag);
            if changed.is_empty() {
                if self::absorbed(&asked, next.as_ref()) {
                    absorbed += 1;
                } else {
                    swallowed.push(format!("{tag}:\n{}", ours(stack)));
                }
                continue;
            }
            match scope {
                Scope::Group => {
                    let g = plan.iter().position(|g| *g == changed);
                    let g =
                        g.unwrap_or_else(|| panic!("{tag}: {changed:?} is no group of {plan:?}"));
                    refused_groups.insert(g);
                    grouped += 1;
                }
                Scope::Sprite => {
                    // Inside one Sprite's plan (its name's read, 4.4 step
                    // 1, among it) or render: that Sprite alone.
                    assert_eq!(changed.len(), 1, "{tag}: {changed:?}");
                    singles += 1;
                }
                Scope::Input => panic!(
                    "{tag}: {changed:?} changed by the input's refusal:\n{}",
                    ours(stack)
                ),
            }
        }
        assert!(
            !refused_groups.is_empty(),
            "{name}: no group's sheet was refused"
        );
        uncounted(name, &s, &whole);
    }
    assert!(
        swallowed.is_empty(),
        "swallowed refusals:\n{}",
        swallowed.join("\n")
    );
    assert!(atlas_names > 0, "no tag walk's name read refused");
    assert!(
        singles > 0 && grouped > 0 && absorbed > 0 && inputs > 0,
        "{singles} Sprites', {grouped} groups', {inputs} inputs' and {absorbed} absorbed refusals"
    );
    eprintln!(
        "the sweep took {:?}: {singles} Sprites', {grouped} groups', {inputs} inputs' and {absorbed} absorbed refusals",
        started.elapsed()
    );
}

/// `s`'s cut of `x.bundle` under `engine` and `meter`, `seen` asked at each
/// output handed (with the registered view): the identities handed, in
/// order, and the cut; none where the view does not register.
fn cut_with(
    s: &AtlasScene,
    engine: &Engine,
    meter: &Arc<Meter>,
    mut seen: impl FnMut(&Output, &Registered),
) -> Option<(Vec<(String, i64)>, Cut)> {
    let members = view_of(s, "x.bundle");
    let x = members.iter().find(|m| m.id == "x.bundle").unwrap();
    let env = engine.env(FileContext::default());
    let admit = Admit {
        env: &env,
        meter,
        hooks: engine,
        max_evictions: u64::MAX,
    };
    let view = View {
        input: x,
        members: &members,
    };
    let registered = ngui::register_bytes(view, admit).ok()?;
    let mut table = KeyTable::new(meter);
    let mut handed = Vec::new();
    let mut sink = |o: Output| {
        seen(&o, &registered);
        let file = String::from_utf8_lossy(o.serialized_file.as_bytes()).into_owned();
        handed.push((file, o.path_id));
        Ok(())
    };
    let ctx = Context {
        registered: &registered,
        meter,
        input: "x.bundle",
        hooks: engine,
        max_evictions: u64::MAX,
        prefix: "",
        members: &[],
        keys: &mut table,
        sink: &mut sink,
    };
    let cut = ngui::cut(ctx, &options(s)).unwrap_or_else(|f| panic!("{f:?}"));
    Some((handed, cut))
}

fn skip_id(k: &ngui::Skip) -> (String, i64) {
    let file = String::from_utf8_lossy(k.serialized_file.as_bytes()).into_owned();
    (file, k.path_id)
}

#[test]
fn a_decode_work_bound_skips_a_sprite_at_its_render() {
    // A render counts its loops as decode work (E59) on the input's meter
    // (review round 1's T4, round 2's T2). Unbounded, the decode work at
    // each output handed is `E_k`; between the outputs `k - 1` and `k` of
    // one group, every unit counted is output `k`'s render. A bound of
    // `E_k - 1` therefore enters that render with the plan done and every
    // earlier output handed: the plan's skips are the whole run's, the
    // outputs before `k` are handed, and `k` is the export's first skip,
    // `limit(decode_work, ...)`.
    let s = scene("own_sheets");
    let engine = Engine {
        dropping: true,
        ..Engine::default()
    };
    let unbounded =
        Meter::with_decode_work(u64::MAX, u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let mut at_sink = Vec::new();
    let (whole, cut) = cut_with(&s, &engine, &unbounded, |_, _| {
        at_sink.push(unbounded.decoded());
    })
    .unwrap();
    assert!(cut.export_skips.is_empty(), "{:?}", cut.export_skips);
    let plan: Vec<_> = cut.plan_skips.iter().map(skip_id).collect();
    let mut at_render = 0;
    for k in 1..whole.len() {
        let meter = Meter::with_decode_work(
            u64::MAX,
            at_sink[k] - 1,
            Arc::new(Unbounded),
            Reservation::Input,
        );
        let (handed, cut) = cut_with(&s, &engine, &meter, |_, _| {}).unwrap();
        assert_eq!(
            cut.plan_skips.iter().map(skip_id).collect::<Vec<_>>(),
            plan,
            "{k}"
        );
        let Some(first) = cut.export_skips.first() else {
            panic!("{k}: no skip under {}", at_sink[k] - 1)
        };
        assert!(
            first.reason.reason.starts_with("limit(decode_work, "),
            "{k}: {first:?}"
        );
        assert_eq!(first.output, SkipOutput::Sprite);
        if handed == whole[..k] && skip_id(first) == whole[k] {
            at_render += 1;
        } else {
            // Output `k` opens its own group: the bound met its sheet.
            assert!(handed.len() <= k, "{k}: {handed:?}");
        }
    }
    assert!(at_render > 0, "no render met the bound");
}

/// Two dependencies, each with a sheet: `both` renders from the first's
/// texture and the second's alpha texture, `first` and `second` from one
/// each.
fn two_dependencies() -> AtlasScene {
    dependencies(&[
        ("both", (1, 1), Some((2, 1))),
        ("first", (1, 1), None),
        ("second", (2, 1), None),
    ])
}

/// A Sprite of [`dependencies`]: its name, its texture and its alpha
/// texture, each a pointer's file and path ids.
type Dependent<'a> = (&'a str, (i32, i64), Option<(i32, i64)>);

/// Two dependency bundles of one texture each (`CAB-t1` and `CAB-t2`, at
/// path id 1) and `x.bundle`, whose Sprites render from them.
fn dependencies(sprites: &[Dependent<'_>]) -> AtlasScene {
    let rgba = |base: u8| texture(1, b"t", format::RGBA32, (4, 4), sheet(base, 4, 4, false));
    let d1 = file(&[], &[rgba(0x10)], true).unwrap();
    let d2 = file(&[], &[rgba(0x90)], true).unwrap();
    let objects: Vec<_> = sprites
        .iter()
        .zip(1..)
        .map(|((name, tex, alpha), path_id)| {
            let spec = SpriteSpec::rect(name, [0.0, 0.0, 3.0, 2.0], *tex);
            match alpha {
                Some(a) => spec.alpha(*a),
                None => spec,
            }
            .object(path_id)
        })
        .collect();
    let x = file(
        &["archive:/CAB-t1/CAB-t1", "archive:/CAB-t2/CAB-t2"],
        &objects,
        true,
    )
    .unwrap();
    let bundle = |id: &str, cab: &str, data: Vec<u8>, deps: &[&str]| SceneBundle {
        id: id.to_owned(),
        data: unityrs_testkit::env::bundle(&[(cab, data)]),
        deps: deps.iter().map(|d| (*d).to_owned()).collect(),
    };
    AtlasScene {
        bundles: vec![
            bundle("d1.bundle", "CAB-t1", d1, &[]),
            bundle("d2.bundle", "CAB-t2", d2, &[]),
            bundle("x.bundle", "CAB-tx", x, &["d1.bundle", "d2.bundle"]),
        ],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions {
            sprites_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    }
}

/// What [`under_cache`] saw: each object's outcome by its SerializedFile
/// and path id, the evictions before the first output and at the last, and
/// the decode work counted.
type Cached = (BTreeMap<(String, i64), String>, [u64; 2], u64);

/// `s`'s cut under a dependency cache of `d` bytes ([`Cached`]); none where
/// the view does not register.
fn under_cache(s: &AtlasScene, d: u64) -> Option<Cached> {
    let engine = Engine {
        dep_cache_bytes: Some(d),
        dropping: true,
        ..Engine::default()
    };
    let m = Meter::with_decode_work(u64::MAX, u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let mut evictions: Vec<u64> = Vec::new();
    let mut keys = Vec::new();
    let (handed, cut) = cut_with(s, &engine, &m, |o, r| {
        evictions.push(r.env.stats().dependencies_evicted);
        keys.push(format!("out {} {:x?}", o.key, o.md5));
    })?;
    let mut got: BTreeMap<(String, i64), String> = handed.into_iter().zip(keys).collect();
    for k in cut.plan_skips.iter().chain(&cut.export_skips) {
        got.insert(
            skip_id(k),
            format!("skip {} {:?}", k.reason.reason, k.reason.detail),
        );
    }
    let span = [
        evictions.first().copied().unwrap_or(0),
        evictions.last().copied().unwrap_or(0),
    ];
    Some((got, span, m.decoded()))
}

#[test]
fn a_dependencys_sheet_stays_held_across_an_eviction() {
    // Under a dependency cache `D` too small for both dependencies, the
    // cut's dereferences evict (review round 1's T5): the next group's
    // load evicts the bundle the last one held. A group's sheet is held
    // with its top-level bundles while it is read and decoded, so every
    // `D` gives the whole run's outputs and skips; and some `D` evicts
    // between the first output handed and the last, during the export
    // (review round 2's T2).
    let s = two_dependencies();
    let (whole, _, _) = under_cache(&s, u64::MAX).unwrap();
    assert_eq!(whole.len(), 3, "{whole:?}");
    let mut during_export = 0;
    for d in (0..1 << 15).step_by(128) {
        let Some((got, [first, last], _)) = under_cache(&s, d) else {
            continue;
        };
        assert_eq!(got, whole, "D {d}");
        if last > first {
            during_export += 1;
        }
    }
    assert!(during_export > 0, "no export evicted");
}

#[test]
fn one_dependency_file_reached_around_its_reopening_is_one_group() {
    // `a` and `b` render from the first dependency's texture, `c`, planned
    // between them, from the second's (B3's review round 2, S2): under a
    // `D` that holds one dependency, `c`'s dereference evicts the first and
    // `b`'s reopens it. The file is known by its row chain, not by the
    // handle it was opened as, so `a` and `b` are one group: groups run in
    // the order of each one's first Sprite, so the export hands `a`, `b`,
    // then `c` (two groups would hand `c` between them), and the sheet is
    // decoded once: the decode work is the uncached run's but the
    // reopened handle's identity (one unit for its one level, each time it
    // is reopened), never the sheet's decode again.
    let s = dependencies(&[
        ("a", (1, 1), None),
        ("c", (2, 1), None),
        ("b", (1, 1), None),
    ]);
    let order = |d| {
        let engine = Engine {
            dep_cache_bytes: Some(d),
            dropping: true,
            ..Engine::default()
        };
        let m = Meter::unlimited();
        let (handed, _) = cut_with(&s, &engine, &m, |_, _| {})?;
        Some(handed.into_iter().map(|(_, id)| id).collect::<Vec<_>>())
    };
    assert_eq!(order(u64::MAX), Some(vec![1, 3, 2]));
    let (_, _, unevicted) = under_cache(&s, u64::MAX).unwrap();
    let mut reopened = 0;
    for d in (0..1 << 15).step_by(128) {
        let Some(got) = order(d) else { continue };
        assert_eq!(got, [1, 3, 2], "D {d}");
        let (_, [first, _], decoded) = under_cache(&s, d).unwrap();
        if first > 0 {
            reopened += 1;
            // At most each of the three Sprites' dereferences reopens.
            assert!(
                (unevicted..=unevicted + 3).contains(&decoded),
                "D {d}: the decode work {decoded}, {unevicted} uncached"
            );
        }
    }
    assert!(reopened > 0, "no D evicted before the export");
}

#[test]
fn an_image_too_wide_for_the_encoder_is_skipped_before_its_work() {
    // A rectangle 2^26 pixels wide and 1 high: the crop makes it (Pillow's
    // crop takes it), and the encoder refuses it (answer 11:
    // `pillow_memory_error`), asked before its export work is counted, so
    // a `max_export_work` that holds only the sheet skips the Sprite and
    // never fails the input (review round 1's R6).
    let cab = file(
        &[],
        &[
            texture(1, b"t", format::RGBA32, (2, 2), sheet(0x10, 2, 2, false)),
            SpriteSpec::rect("wide", [0.0, 0.0, 67_108_864.0, 1.0], (0, 1)).object(2),
            SpriteSpec::rect("fine", [0.0, 0.0, 1.0, 1.0], (0, 1)).object(3),
        ],
        true,
    )
    .unwrap();
    let s = AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".to_owned(),
            data: unityrs_testkit::env::bundle(&[("CAB-w", cab)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions {
            sprites_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    };
    let engine = Engine {
        max_work: Some(2 * 2 * 4 + 4),
        dropping: true,
        ..Engine::default()
    };
    let ran = run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &engine,
    )
    .unwrap();
    let skips: Vec<_> = ran
        .skips
        .iter()
        .map(|k| {
            (
                k.path_id,
                k.reason.reason.as_str(),
                k.reason.detail.as_deref(),
            )
        })
        .collect();
    assert_eq!(skips, [(2, "decode_error", Some("pillow_memory_error"))]);
    assert_eq!(ran.handed.len(), 1);
    assert_eq!(engine.work.get(), 2 * 2 * 4 + 4);
}
