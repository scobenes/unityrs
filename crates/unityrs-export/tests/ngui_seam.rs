//! The engine seam (`docs/plans/batch.md` 2.6; review rounds 1 to 3): the
//! cut inside an attempt the engine owns. X's own handle on `M_X` (its
//! reads charge `M_X`, so a refused head or MonoScript read and a
//! decode-work bound are its skips), the stop check, copy-out, export work,
//! the refusal probe, the key table, the sink's prefix and member paths,
//! and atlases turned off. Registration's own tests are in
//! `ngui_register.rs`.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use unityrs_core::FileContext;
use unityrs_core::budget::{Counting, Reservation, Unbounded};
use unityrs_core::env::EnvOptions;
use unityrs_core::limits::Meter;
use unityrs_export::ngui::{
    self, Admit, Context, Failure, KeyRule, KeyTable, MemberTemplate, NoHooks, Output, OutputKind,
    Rejected, SkipOutput, Template, TemplateKind, View,
};
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{
    AtlasScene, CutOptions, PropName, SceneBundle, atlas, file, format, material, mono_script,
    scenes, sheet, sprite, texture,
};

mod common;
use common::{
    Engine, Event, Ran, absorbed_at, big_sheet, big_sheet_named, options, refused_at, refused_in,
    refused_then_at, run_in, run_with, view_of,
};

fn scene(name: &str) -> AtlasScene {
    scenes()
        .unwrap()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

fn keys(ran: &Ran) -> Vec<&str> {
    ran.handed.iter().map(|o| o.key.as_str()).collect()
}

fn skips(ran: &Ran) -> Vec<(i64, &'static str, &str)> {
    ran.skips
        .iter()
        .map(|s| (s.path_id, s.output.name(), s.reason.reason.as_str()))
        .collect()
}

/// The view registered as the tests' harness does, unbounded.
fn register(view: View<'_>, m: &Arc<Meter>) -> ngui::Registered {
    let env = EnvOptions::batch(Arc::new(FileContext::default()));
    let admit = Admit {
        env: &env,
        meter: m,
        hooks: &NoHooks,
        max_evictions: u64::MAX,
    };
    ngui::register_bytes(view, admit).unwrap_or_else(|f| panic!("{f:?}"))
}

fn budget(n: u64) -> Arc<Meter> {
    Meter::new(u64::MAX, Arc::new(Counting::new(n)), Reservation::Input)
}

#[test]
fn a_refused_head_or_monoscript_read_is_counted_and_skips_its_atlas() {
    // Each reservation of M_X refused in turn, named by the operation that
    // asked for it (review round 2's T1): a refused head read and a refused
    // MonoScript read each leave the atlas unrecognised, counted, and
    // skipped at the plan for the budget, and only the texture written.
    let s = big_sheet();
    let mut seen = Vec::new();
    for at in 0.. {
        let (site, got) = refused_at(&s, at);
        let Some(site) = site else {
            break;
        };
        if !matches!(site, "head" | "monoscript") {
            continue;
        }
        let ran = got.unwrap_or_else(|f| panic!("{at} ({site}): {f:?}"));
        if ran.skips.is_empty() && ran.unresolved_scripts == 0 {
            // A charge the core may go without (a cache it then does not
            // fill): the read went on, and the cut is whole.
            assert_eq!(ran.handed.len(), 4, "{at} ({site})");
            continue;
        }
        assert_eq!(ran.unresolved_scripts, 1, "{at} ({site})");
        assert_eq!(
            skips(&ran),
            [(3, "atlas", "budget(input)")],
            "{at} ({site})"
        );
        assert_eq!(ran.plan_skips, 1, "{at} ({site}): at the plan");
        assert!(ran.handed.iter().all(|o| o.kind == OutputKind::Texture));
        seen.push(site);
        assert!(at < 1 << 16);
    }
    assert!(seen.contains(&"head"), "no head read was refused: {seen:?}");
    assert!(
        seen.contains(&"monoscript"),
        "no MonoScript read was refused"
    );
}

#[test]
fn a_decode_work_bound_on_m_x_is_an_objects_skip() {
    // A typetree compile counts its names as decode work on the reading
    // file's meter: X's own, so some bound refuses an object's read, not
    // X's open.
    let s = big_sheet();
    let mut seen = Vec::new();
    for work in 0..1 << 14 {
        let meter =
            Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input);
        match run_with(&s, "x.bundle", FileContext::default(), &meter) {
            Ok(ran) if ran.skips.is_empty() => break,
            Ok(ran) => {
                for k in &ran.skips {
                    assert!(k.reason.reason.starts_with("limit(decode_work, "), "{k:?}");
                    seen.push((k.class.clone(), k.output.name()));
                }
            }
            Err(Failure::Input(r)) => assert!(r.reason.starts_with("limit(decode_work, "), "{r:?}"),
            Err(f) => panic!("{f:?}"),
        }
    }
    assert!(!seen.is_empty(), "no object's read met the bound");
}

#[test]
fn a_check_falls_between_outputs_and_before_each_sheet_group() {
    // Every scene's cut, logged (review round 2's T1): between two outputs
    // handed over, and between an output and the next pixel buffer (a
    // group's decode, a crop), the stop is checked.
    let mut groups = 0;
    for (name, s) in scenes().unwrap() {
        for input in &s.inputs {
            let engine = Engine::default();
            let got = run_in(
                &s,
                input,
                FileContext::default(),
                &Meter::unlimited(),
                &engine,
            );
            if got.is_err() {
                continue;
            }
            let log = engine.log.borrow();
            let mut since_sink = None;
            for e in log.iter() {
                match e {
                    Event::Check => since_sink = None,
                    Event::Sink(k) => {
                        assert_eq!(since_sink, None, "{name}/{input}: no check before {k}");
                        since_sink = Some(k.clone());
                    }
                    Event::Work(_) => {
                        assert_eq!(since_sink, None, "{name}/{input}: no check before a buffer");
                    }
                }
            }
            if name == "two_sheet_groups" {
                groups += 1;
            }
        }
    }
    assert!(groups > 0, "the two-group scene ran");
}

#[test]
fn a_stop_ends_the_attempt_at_the_check_that_sees_it() {
    // The flag rises at check `at`, and the attempt returns there: no check
    // is made after it (review round 2's T1: the flag is sticky, so a
    // check removed would still end the attempt at a later one).
    let s = scene("two_sheet_groups");
    let full = run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &Engine::default(),
    )
    .unwrap();
    let mut at = 0;
    loop {
        let engine = Engine {
            stop_at: Some(at),
            ..Engine::default()
        };
        let got = run_in(
            &s,
            "x.bundle",
            FileContext::default(),
            &Meter::unlimited(),
            &engine,
        );
        match got {
            Err(Failure::Stopped) => assert_eq!(engine.checks.get(), at + 1, "{at}"),
            Ok(ran) => {
                assert_eq!(keys(&ran), keys(&full));
                break;
            }
            Err(f) => panic!("{f:?}"),
        }
        at += 1;
        assert!(at < 10_000);
    }
    assert!(at > 3, "checks between objects, groups and outputs: {at}");
}

#[test]
fn every_output_is_handed_over_before_the_next_buffer_is_made() {
    // A sink that refuses its n-th output ends the cut there, and no pixel
    // buffer after that output was counted (review round 2's T1): the
    // export work at the refusal is the work before that sink call.
    let s = big_sheet();
    let full = Engine::default();
    run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &full,
    )
    .unwrap();
    // The work counted before each sink call.
    let mut before = Vec::new();
    let mut work = 0;
    for e in full.log.borrow().iter() {
        match e {
            Event::Work(n) => work += n,
            Event::Sink(_) => before.push(work),
            Event::Check => {}
        }
    }
    // The texture and the JSON after the sheet's 32 x 32 x 4 bytes, each
    // crop's 2 x 2 x 4 after it is made and before the next (review round
    // 3's T1): known from the scene, not from another run.
    assert_eq!(before, [4096, 4096, 4112, 4128]);
    for (n, at) in before.iter().enumerate() {
        let engine = Engine {
            refuse_output: Some((n, Failure::Run(ngui::Reason::plain("host_io")))),
            ..Engine::default()
        };
        let got = run_in(
            &s,
            "x.bundle",
            FileContext::default(),
            &Meter::unlimited(),
            &engine,
        );
        assert!(matches!(got, Err(Failure::Run(r)) if r.reason == "host_io"));
        assert_eq!(engine.work.get(), *at, "output {n}");
    }
}

#[test]
fn export_work_counts_every_pixel_buffer_before_it_is_made() {
    let s = big_sheet();
    let engine = Engine::default();
    run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &engine,
    )
    .unwrap();
    // The 32 x 32 RGBA sheet once, and two 2 x 2 RGBA crops.
    assert_eq!(engine.work.get(), 32 * 32 * 4 + 2 * 2 * 2 * 4);
    for max in [0, 4095, 4096, 4111] {
        let engine = Engine {
            max_work: Some(max),
            ..Engine::default()
        };
        let got = run_in(
            &s,
            "x.bundle",
            FileContext::default(),
            &Meter::unlimited(),
            &engine,
        );
        let Err(Failure::Input(r)) = got else {
            panic!("{max}: not refused");
        };
        assert_eq!(r.reason, format!("limit(export_work, {max})"));
    }
}

#[test]
fn a_refusal_the_budget_did_not_count_is_host_memory() {
    // The probe: a refusal during which the attempt budget's count moved is
    // the budget's; one during which it did not is no budget's.
    let s = big_sheet();
    let mut stops = 0;
    for n in (0..1 << 16).step_by(64) {
        let counted = common::Counted::new(Arc::new(Counting::new(n)));
        let moving = counted.refusals.clone();
        let meter = Meter::new(u64::MAX, Arc::new(counted), Reservation::Input);
        let engine = Engine {
            refusals: Some(moving),
            ..Engine::default()
        };
        let counted = run_in(&s, "x.bundle", FileContext::default(), &meter, &engine);
        let frozen = Engine {
            refusals: Some(Arc::new(AtomicU64::new(0))),
            ..Engine::default()
        };
        let plain = run_with(&s, "x.bundle", FileContext::default(), &budget(n));
        let stuck = run_in(&s, "x.bundle", FileContext::default(), &budget(n), &frozen);
        match (&plain, &counted) {
            (Ok(a), Ok(b)) => assert_eq!(skips(a), skips(b)),
            (Err(a), Err(b)) => assert_eq!(a, b),
            _ => panic!("{n}: the counted budget changed the answer"),
        }
        let refused = match &plain {
            Ok(ran) => !ran.skips.is_empty(),
            Err(_) => true,
        };
        if refused {
            assert!(
                matches!(&stuck, Err(Failure::Run(r)) if r.reason == "host_memory"),
                "{n}: {:?}",
                stuck.as_ref().err()
            );
            stops += 1;
        } else {
            break;
        }
    }
    assert!(stops > 0);
}

#[test]
fn each_refusal_no_budget_counted_is_host_memory_at_its_own_call() {
    // Review round 3's P1: every reservation of M_X refused in turn, the
    // attempt budget's count held still (an allocator's refusal), over a
    // cut with a dependency and streamed sheets: each is either absorbed by
    // the core (the cut whole) or the stop `host_memory`, never a budget's
    // skip or failure; and the sites named are the cutter's every kind of
    // call, registration's charges and the resource's calls among them.
    let mut sites = std::collections::BTreeSet::new();
    for (s, input) in [
        (big_sheet(), "x.bundle"),
        (scene("streamed_sheets"), "x.bundle"),
    ] {
        let full = run_in(
            &s,
            input,
            FileContext::default(),
            &Meter::unlimited(),
            &Engine::default(),
        )
        .unwrap();
        for at in 0.. {
            assert!(at < 1 << 16);
            let (site, got) = refused_in(&s, input, at, false);
            let Some(site) = site else {
                break;
            };
            match got {
                Ok(ran) => {
                    assert_eq!(keys(&ran), keys(&full), "{at} ({site})");
                    assert_eq!(skips(&ran), skips(&full), "{at} ({site})");
                }
                Err(Failure::Run(r)) if r.reason == "host_memory" => {
                    sites.insert(site);
                }
                Err(f) => panic!("{at} ({site}): {f:?}"),
            }
        }
    }
    for site in [
        "open",
        "reach",
        "peek_bits",
        "resource",
        "head",
        "monoscript",
        "decode",
        "png",
        "crop",
        "metadata",
        "json",
    ] {
        assert!(sites.contains(site), "no {site} refusal: {sites:?}");
    }
}

#[test]
fn a_refusal_after_one_the_core_absorbed_is_host_memory() {
    // Review round 4's S2: a reservation counted as refused and granted (a
    // refusal the core absorbed: its call went on), then the next refused
    // uncounted, in the next core call of one operation. A probe begun
    // before the first would see the count move and read the second as the
    // budget's; one probe per core call reads it as `host_memory`. Over the
    // resource's own-chain calls (each candidate's `joined` and each
    // `simplify_name`) and X's own open (its bytes' charge, then its parse),
    // each seen. (A row's `entry` then its `entry_range` cannot both
    // reserve in one read: `the_resource_reads_entry_reserves_nothing`.)
    let s = scene("streamed_sheets");
    let mut seen = std::collections::BTreeSet::new();
    for at in 0.. {
        assert!(at < 1 << 16);
        let ([first, then], got) = absorbed_at(&s, "x.bundle", at);
        let Some((site, call)) = first else {
            break;
        };
        let Some((then_site, then_call)) = then else {
            continue;
        };
        if !matches!(site, "resource" | "open") || then_site != site || then_call == call {
            continue;
        }
        let Err(Failure::Run(r)) = &got else {
            panic!("{at} ({site}: {call}, then {then_call}): {got:?}");
        };
        assert_eq!(
            r.reason, "host_memory",
            "{at} ({site}: {call}, then {then_call})"
        );
        seen.insert((site, call, then_call));
    }
    let sites: std::collections::BTreeSet<_> = seen.iter().map(|s| s.0).collect();
    assert_eq!(sites, ["open", "resource"].into(), "{seen:?}");
    let pair = |site: &str, call: &str, then: &str| {
        seen.iter()
            .any(|(s, c, t)| *s == site && c.ends_with(call) && t.ends_with(then))
    };
    assert!(
        pair("resource", "simplify_name", "Candidate::joined"),
        "{seen:?}"
    );
    assert!(
        pair("resource", "Candidate::joined", "simplify_name"),
        "{seen:?}"
    );
    assert!(
        pair("open", "Meter::charge", "parse_file_with_reader"),
        "{seen:?}"
    );
}

#[test]
fn the_resource_reads_entry_reserves_nothing() {
    // Review round 5's T4: the pair `entry` then `entry_range` of one own
    // row (`resource::read_row`) cannot both reserve M_X in one read, so S2
    // has no such pair to require. Selection opens every top-level row of
    // X first (`Attempt::select`, attempt.rs:242-253, a failure failing X),
    // and the core parses a row once and publishes it (`BundleFile::entry`,
    // unityrs-core files/bundle/mod.rs:408-414), so `read_row`'s `entry` is
    // that published row and reserves nothing; only `entry_range` can, when
    // the row's run is not cached (its publish refused, runs.rs:436; the
    // run index evicts nothing, and the Environment's evictions are of
    // dependencies, never X's own bundle, env/mod.rs:111). Each
    // reservation refused for real and counted, then the next uncounted
    // (`Refuse::refused_then`, a refusal the core absorbs as it does): none
    // is in a resource read's `entry`, and each uncounted one in a resource
    // read's later core call is `host_memory`.
    let s = scene("streamed_sheets");
    let mut resource = 0;
    for at in 0.. {
        assert!(at < 1 << 16);
        let ([first, then], got) = refused_then_at(&s, "x.bundle", at);
        let Some((site, call)) = first else {
            break;
        };
        for (site, call) in [Some((site, call.clone())), then.clone()]
            .into_iter()
            .flatten()
        {
            if site == "resource" {
                resource += 1;
                assert!(!call.ends_with("BundleFile::entry"), "{at}: {call}");
            }
        }
        let Some((then_site, then_call)) = then else {
            continue;
        };
        if then_site == "resource" && then_call != call {
            let Err(Failure::Run(r)) = &got else {
                panic!("{at} ({site}: {call}, then {then_call}): {got:?}");
            };
            assert_eq!(r.reason, "host_memory", "{at} ({call}, then {then_call})");
        }
    }
    assert!(resource > 0);
}

#[test]
fn the_key_table_is_the_engines_and_keys_are_checked_after_the_prefix() {
    let s = big_sheet();
    let m = Meter::unlimited();
    let members = view_of(&s, "x.bundle");
    let x = members.iter().find(|m| m.id == "x.bundle").unwrap().clone();
    let view = View {
        input: &x,
        members: &members,
    };
    let registered = register(view, &m);
    let mut table = KeyTable::new(&m);
    // A key another stage took first.
    assert!(table.take("big.json").unwrap());
    let members = [
        MemberTemplate {
            archive: 0,
            kind: TemplateKind::AtlasSprite,
            path: Template::parse("crops/{atlas}/{sprite}.png", TemplateKind::AtlasSprite).unwrap(),
        },
        MemberTemplate {
            archive: 1,
            kind: TemplateKind::AtlasSprite,
            path: Template::parse("{atlas}/{sprite}/", TemplateKind::AtlasSprite).unwrap(),
        },
    ];
    let mut outs: Vec<Output> = Vec::new();
    let mut sink = |o: Output| {
        outs.push(o);
        Ok(())
    };
    let ctx = Context {
        registered: &registered,
        meter: &m,
        input: "x.bundle",
        hooks: &NoHooks,
        max_evictions: u64::MAX,
        prefix: "out/",
        members: &members,
        keys: &mut table,
        sink: &mut sink,
    };
    let cut = ngui::cut(ctx, &options(&s)).unwrap();
    let json: Vec<_> = cut
        .plan_skips
        .iter()
        .filter(|k| k.output == SkipOutput::AtlasJson)
        .map(|k| k.reason.reason.as_str())
        .collect();
    assert_eq!(json, ["duplicate_key"]);
    let crop = outs
        .iter()
        .find(|o| o.kind == OutputKind::AtlasSprite)
        .unwrap();
    let paths: Vec<_> = crop
        .members
        .iter()
        .map(|p| (p.archive, p.path.clone()))
        .collect();
    // A rejected path keeps its bytes and its rule for 2.8's
    // `archive_members` row (review round 2's S5).
    let rejected = Rejected {
        rule: KeyRule::TrailingSlash,
        path: "big/s/".to_owned(),
        cut: false,
    };
    assert_eq!(
        paths,
        [(0, Ok("crops/big/s.png".to_owned())), (1, Err(rejected))]
    );
    assert_eq!(table.len(), 4, "the texture, the JSON's key and two crops");
    // The prefix's own refusal.
    let mut table = KeyTable::new(&m);
    let mut sink = |_: Output| Ok(());
    let ctx = Context {
        registered: &registered,
        meter: &m,
        input: "x.bundle",
        hooks: &NoHooks,
        max_evictions: u64::MAX,
        prefix: "_unityrs/",
        members: &[],
        keys: &mut table,
        sink: &mut sink,
    };
    let cut = ngui::cut(ctx, &options(&s)).unwrap();
    assert!(
        cut.plan_skips
            .iter()
            .all(|k| k.reason.reason == "unwritable_key(reserved_prefix)")
    );
    assert_eq!(cut.plan_skips.len(), 4);
}

#[test]
fn a_rejected_member_path_is_kept_cut_by_the_text_rule() {
    // A path past 1,024 bytes is `too_long`, its first 1,024 bytes kept and
    // marked cut.
    let s = big_sheet();
    let m = Meter::unlimited();
    let members = view_of(&s, "x.bundle");
    let x = members.iter().find(|m| m.id == "x.bundle").unwrap().clone();
    let view = View {
        input: &x,
        members: &members,
    };
    let registered = register(view, &m);
    let long = format!("{}{{sprite}}", "a/".repeat(600));
    let members = [MemberTemplate {
        archive: 0,
        kind: TemplateKind::AtlasSprite,
        path: Template::parse(&long, TemplateKind::AtlasSprite).unwrap(),
    }];
    let mut table = KeyTable::new(&m);
    let mut outs: Vec<Output> = Vec::new();
    let mut sink = |o: Output| {
        outs.push(o);
        Ok(())
    };
    let ctx = Context {
        registered: &registered,
        meter: &m,
        input: "x.bundle",
        hooks: &NoHooks,
        max_evictions: u64::MAX,
        prefix: "",
        members: &members,
        keys: &mut table,
        sink: &mut sink,
    };
    ngui::cut(ctx, &options(&s)).unwrap();
    let crop = outs
        .iter()
        .find(|o| o.kind == OutputKind::AtlasSprite)
        .unwrap();
    let [member] = crop.members.as_slice() else {
        panic!("one member path");
    };
    let Err(rejected) = &member.path else {
        panic!("rejected");
    };
    assert_eq!(rejected.rule, KeyRule::TooLong);
    assert_eq!(rejected.path, "a/".repeat(512));
    assert!(rejected.cut);
    // Review round 3's P3: a sprite named 1,023 bytes and a four-byte
    // character, its loose key short, its member path `{sprite}`. The
    // expansion stops two bytes into the character, which the text rule of
    // the whole path drops: the path kept is the 1,023 bytes, cut.
    // One loose key for both sprites: the later, the long one, wins (E30).
    let kept = format!("{}x", "a/".repeat(511));
    let name = format!("{kept}\u{1f600}");
    let s = big_sheet_named([b"s", name.as_bytes()]);
    let rejected = rejected_member(&s, "{sprite}", "{atlas}/{path_id}.png");
    assert_eq!(rejected.rule, KeyRule::TooLong);
    assert_eq!((rejected.path, rejected.cut), (kept.clone(), true));
    // Review round 4's A3, Codex's case: the character spans two variables
    // of `{atlas}{sprite}`, its lead the atlas's last byte (the atlas 1,024
    // bytes), the rest the sprite's first, the sheet the Material's, every
    // loose key constant. The cut character is dropped as before.
    let mut atlas_name = kept.clone().into_bytes();
    atlas_name.push(0xF0);
    let s = material_sheet(&atlas_name, b"\x9f\x98\x80z");
    let rejected = rejected_member(&s, "{atlas}{sprite}", "k.png");
    assert_eq!(rejected.rule, KeyRule::TooLong);
    assert_eq!((rejected.path, rejected.cut), (kept, true));
}

/// The rejected member path of the one crop of `s`'s cut, its member path
/// `template`, its loose key `sprite_key`.
fn rejected_member(s: &AtlasScene, template: &str, sprite_key: &str) -> Rejected {
    let m = Meter::unlimited();
    let members = view_of(s, "x.bundle");
    let x = members.iter().find(|m| m.id == "x.bundle").unwrap().clone();
    let view = View {
        input: &x,
        members: &members,
    };
    let registered = register(view, &m);
    let members = [MemberTemplate {
        archive: 0,
        kind: TemplateKind::AtlasSprite,
        path: Template::parse(template, TemplateKind::AtlasSprite).unwrap(),
    }];
    let mut table = KeyTable::new(&m);
    let mut outs: Vec<Output> = Vec::new();
    let mut sink = |o: Output| {
        outs.push(o);
        Ok(())
    };
    let ctx = Context {
        registered: &registered,
        meter: &m,
        input: "x.bundle",
        hooks: &NoHooks,
        max_evictions: u64::MAX,
        prefix: "",
        members: &members,
        keys: &mut table,
        sink: &mut sink,
    };
    let mut o = options(s);
    o.sprite_key = Template::parse(sprite_key, TemplateKind::AtlasSprite).unwrap();
    ngui::cut(ctx, &o).unwrap();
    let crop = outs
        .iter()
        .find(|o| o.kind == OutputKind::AtlasSprite)
        .unwrap();
    let [member] = crop.members.as_slice() else {
        panic!("one member path");
    };
    let Err(rejected) = &member.path else {
        panic!("rejected");
    };
    rejected.clone()
}

/// A sheet named `big` the atlas `atlas_name` selects by its Material, one
/// sprite `sprite_name`, the JSON's and the crops' keys constant.
fn material_sheet(atlas_name: &[u8], sprite_name: &[u8]) -> AtlasScene {
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            texture(
                2,
                b"big",
                format::RGBA32,
                (32, 32),
                sheet(0x10, 32, 32, false),
            ),
            material(4, "m", PropName::Plain, &[("_MainTex", (0, 2))]),
            atlas(
                3,
                atlas_name,
                (0, 1),
                (0, 4),
                vec![sprite(sprite_name, (0, 0, 2, 2), [0; 8])],
                Vec::new(),
            ),
        ],
        true,
    )
    .unwrap();
    AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".to_owned(),
            data: bundle(&[("CAB-xb", cab)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions {
            sprite_key: "k.png".to_owned(),
            json_key: "a.json".to_owned(),
            ..CutOptions::default()
        },
    }
}

#[test]
fn atlases_can_be_turned_off() {
    let s = big_sheet();
    let m = Meter::unlimited();
    let members = view_of(&s, "x.bundle");
    let x = members.iter().find(|m| m.id == "x.bundle").unwrap().clone();
    let view = View {
        input: &x,
        members: &members,
    };
    let registered = register(view, &m);
    let mut table = KeyTable::new(&m);
    let mut outs: Vec<Output> = Vec::new();
    let mut sink = |o: Output| {
        outs.push(o);
        Ok(())
    };
    let ctx = Context {
        registered: &registered,
        meter: &m,
        input: "x.bundle",
        hooks: &NoHooks,
        max_evictions: u64::MAX,
        prefix: "",
        members: &[],
        keys: &mut table,
        sink: &mut sink,
    };
    let mut o = options(&s);
    o.atlases = false;
    let cut = ngui::cut(ctx, &o).unwrap();
    assert!(cut.plan_skips.is_empty() && cut.export_skips.is_empty());
    assert_eq!(cut.unresolved_scripts, 0);
    assert_eq!(cut.sheets, []);
    let kinds: Vec<_> = outs.iter().map(|o| (o.kind, o.key.as_str())).collect();
    assert_eq!(kinds, [(OutputKind::Texture, "big.png")]);
}
