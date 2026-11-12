//! The NGUI tests' harness: a scene's options and views, and a cut of one
//! input over its view inside an attempt built as the engine builds one
//! (`ngui::register_bytes`, then `ngui::cut` with a `Context`): the
//! registration the engine will call, a key table, and a sink that keeps
//! every output it is handed, or drops it as the engine's copy-out does.
//! The engine logs its checks, its export work and each sink call, and a
//! budget can refuse one named reservation ([`Refuse`]).

#![allow(dead_code)]

pub mod pixels;

use std::backtrace::Backtrace;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation, Unbounded};
use unityrs_core::env::EnvOptions;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext};
use unityrs_export::ngui::{
    self, Admit, Candidate, Context, Failure, Hooks, KeyTable, Member, MemberTemplate, Options,
    Output, OutputKind, Reason, SheetRow, Skip, Template, TemplateKind, View,
};
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{
    AtlasScene, CutOptions, SceneBundle, atlas, file, format, mono_script, sheet, sprite, texture,
};

/// A scene's options as the cutter takes them.
pub fn options(scene: &AtlasScene) -> Options {
    let o = &scene.options;
    Options {
        sprite_key: Template::parse(&o.sprite_key, TemplateKind::AtlasSprite).unwrap(),
        json_key: Template::parse(&o.json_key, TemplateKind::AtlasJson).unwrap(),
        texture_key: o
            .texture_key
            .as_ref()
            .map(|k| Template::parse(k, TemplateKind::Texture).unwrap()),
        sprites_key: o
            .sprites_key
            .as_ref()
            .map(|k| Template::parse(k, TemplateKind::Sprite).unwrap()),
        script_classes: o
            .script_classes
            .iter()
            .map(|c| c.as_bytes().to_vec())
            .collect(),
        skip_name_suffixes: o
            .skip_name_suffixes
            .iter()
            .map(|c| c.as_bytes().to_vec())
            .collect(),
        atlases: true,
    }
}

/// `view(X)` (4.2): `X` and the `deps` of every bundle it holds.
pub fn view_of(scene: &AtlasScene, input: &str) -> Vec<Member> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![input.to_owned()];
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let b = scene.bundles.iter().find(|b| b.id == id).unwrap();
        pending.extend(b.deps.iter().cloned());
    }
    scene
        .bundles
        .iter()
        .filter(|b| seen.contains(&b.id))
        .map(|b| Member {
            id: b.id.clone(),
            data: Bytes::from(b.data.clone()),
        })
        .collect()
}

/// An output as the sink saw it, its bytes dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handed {
    pub kind: OutputKind,
    /// The object's SerializedFile and path id.
    pub object: (String, i64),
    pub key: String,
    pub unit: Option<String>,
    pub md5: [u8; 16],
}

/// What one attempt's cut gave: the outputs the sink was handed, in the
/// order it was handed them (kept whole unless the engine drops them), and
/// the cut's records, its skips in record order (the plan's, then the
/// export's).
#[derive(Debug, Default)]
pub struct Ran {
    pub outputs: Vec<Output>,
    pub handed: Vec<Handed>,
    pub skips: Vec<Skip>,
    pub plan_skips: usize,
    pub unresolved_scripts: u64,
    pub sheets: Vec<SheetRow>,
    pub nested_containers: Vec<unityrs_core::RawStr>,
}

impl Ran {
    /// 4.5's unit rule over what was handed: each unit is the key of an
    /// output handed over (review round 2's C6), and the winners of this
    /// input alone publish every key, none `unit_lost`.
    pub fn assert_units(&self) {
        let keys: BTreeSet<&str> = self.handed.iter().map(|h| h.key.as_str()).collect();
        for h in &self.handed {
            if let Some(u) = &h.unit {
                assert!(
                    keys.contains(u.as_str()),
                    "{} has the unit {u}, no output's",
                    h.key
                );
            }
        }
        let candidates: Vec<Candidate> = self
            .handed
            .iter()
            .map(|h| Candidate {
                input: "x.bundle".to_owned(),
                key: h.key.clone(),
                unit: h.unit.clone(),
                md5: h.md5,
            })
            .collect();
        for d in ngui::decide(&candidates, &BTreeMap::new(), &BTreeSet::new(), true) {
            assert!(d.publish, "{}: {:?}", d.key, d.reason);
        }
    }
}

/// A budget that counts its refusals, as the engine's attempt budget does
/// (2.7's probe).
pub struct Counted {
    inner: Arc<dyn MemoryBudget>,
    pub refusals: Arc<AtomicU64>,
}

impl fmt::Debug for Counted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Counted").finish_non_exhaustive()
    }
}

impl Counted {
    pub fn new(inner: Arc<dyn MemoryBudget>) -> Self {
        Self {
            inner,
            refusals: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl MemoryBudget for Counted {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        self.inner.try_reserve(bytes, what).inspect_err(|_| {
            self.refusals.fetch_add(1, Ordering::AcqRel);
        })
    }
}

/// A budget that grants every reservation but its `at`-th (counting from
/// 0), which it refuses and counts, naming the operation that asked for it
/// from the frames of the stack it was asked on ([`site`]). Made by
/// [`Refuse::absorbed`], it counts its `at`-th and grants it, as a refusal
/// the core absorbed (the call it was in goes on), and refuses the next
/// without counting it, as the allocator refuses, naming both operations;
/// made by [`Refuse::refused_then`], it refuses and counts its `at`-th, for
/// the core to absorb or not as it does, and refuses the next uncounted.
pub struct Refuse {
    at: u64,
    then: Option<u64>,
    /// Whether the `at`-th, when a `then` follows, is granted.
    grant: bool,
    asked: AtomicU64,
    pub refusals: Arc<AtomicU64>,
    pub site: Mutex<Option<&'static str>>,
    pub then_site: Mutex<Option<&'static str>>,
    /// The two refusals' stacks, `<` and `>` taken out.
    pub stacks: Mutex<[String; 2]>,
    /// The bytes the `at`-th asked for.
    pub bytes: Mutex<u64>,
    /// The vector-growth invocation active at the refused reservation.
    pub growth: Mutex<Option<u64>>,
    /// Whether the reservation after the `at`-th is recorded ([`Self::next`]).
    follow: bool,
    /// The reservation after the `at`-th, granted, when [`Refuse::followed`]
    /// made the budget: its stack, bytes and growth invocation.
    pub next: Mutex<Option<Asked>>,
}

impl fmt::Debug for Refuse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refuse")
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

impl Refuse {
    pub fn new(at: u64) -> Self {
        Self {
            at,
            then: None,
            grant: false,
            asked: AtomicU64::new(0),
            refusals: Arc::new(AtomicU64::new(0)),
            site: Mutex::new(None),
            then_site: Mutex::new(None),
            stacks: Mutex::new([String::new(), String::new()]),
            bytes: Mutex::new(0),
            growth: Mutex::new(None),
            follow: false,
            next: Mutex::new(None),
        }
    }

    /// [`Refuse::new`], the reservation after the `at`-th granted and
    /// recorded ([`Self::next`]): whether the call refused asked again.
    pub fn followed(at: u64) -> Self {
        Self {
            follow: true,
            ..Self::new(at)
        }
    }

    /// The `at`-th counted and granted, the next refused uncounted.
    pub fn absorbed(at: u64) -> Self {
        Self {
            then: Some(at + 1),
            grant: true,
            ..Self::new(at)
        }
    }

    /// The `at`-th refused and counted, the next refused uncounted.
    pub fn refused_then(at: u64) -> Self {
        Self {
            then: Some(at + 1),
            ..Self::new(at)
        }
    }
}

/// The cutter's operation a stack is in, by its innermost frame of one.
/// The frames are read with their `<` and `>` taken out, so a method reads
/// the same under either demangling (`<a::B>::m` and `a::B::m::h..`).
pub fn site(stack: &str) -> &'static str {
    const SITES: [(&str, &str); 16] = [
        ("ngui::resource::", "resource"),
        ("ngui::attempt::Attempt::image", "decode"),
        ("unityrs_imageio::png", "png"),
        ("ngui::read::Reader::head", "head"),
        ("ngui::read::Reader::peek_name", "peek_name"),
        ("ngui::read::Reader::atlas_name", "atlas_name"),
        ("ngui::read::material_name", "material"),
        ("ngui::read::deref", "deref"),
        ("ngui::read::texture", "texture_read"),
        ("ngui::attempt::Attempt::script_class", "monoscript"),
        ("ngui::export::crop", "crop"),
        ("ngui::attempt::Attempt::output", "metadata"),
        ("ngui::attempt::Attempt::json", "json"),
        ("ngui::register::reach", "reach"),
        ("ngui::register::Peeked as", "peek_bits"),
        ("ngui::register::open", "open"),
    ];
    let stack: String = stack.chars().filter(|c| !matches!(c, '<' | '>')).collect();
    // The stack prints innermost first: the first frame of a site names it.
    let mut first: Option<(usize, &'static str)> = None;
    for (frame, name) in SITES {
        if let Some(at) = stack.find(frame)
            && first.is_none_or(|(f, _)| at < f)
        {
            first = Some((at, name));
        }
    }
    first.map_or("other", |(_, n)| n)
}

impl MemoryBudget for Refuse {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let n = self.asked.fetch_add(1, Ordering::AcqRel);
        let (named, k) = if n == self.at {
            self.refusals.fetch_add(1, Ordering::AcqRel);
            (&self.site, 0)
        } else if self.then == Some(n) {
            (&self.then_site, 1)
        } else {
            if self.follow && n == self.at + 1 {
                let stack = Backtrace::force_capture().to_string();
                let stack = stack.chars().filter(|c| !matches!(c, '<' | '>')).collect();
                *self.next.lock().unwrap() =
                    Some((stack, bytes, unityrs_core::testing::current_growth()));
            }
            return Unbounded.try_reserve(bytes, what);
        };
        if k == 0 {
            *self.bytes.lock().unwrap() = bytes;
            *self.growth.lock().unwrap() = unityrs_core::testing::current_growth();
        }
        let stack = Backtrace::force_capture().to_string();
        *named.lock().unwrap() = Some(site(&stack));
        if let Some(s) = self.stacks.lock().unwrap().get_mut(k) {
            *s = stack.chars().filter(|c| !matches!(c, '<' | '>')).collect();
        }
        if k == 0 && self.grant {
            return Unbounded.try_reserve(bytes, what);
        }
        Err(BudgetExceeded {
            requested: bytes,
            what,
        })
    }
}

/// What the engine saw, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A stop check.
    Check,
    /// Export work for a pixel buffer, before it is made.
    Work(u64),
    /// The sink was handed this output.
    Sink(String),
}

/// The engine's side of an attempt, as a test sets it.
#[derive(Default)]
pub struct Engine {
    /// The stop flag rises at this stop check (counting from 0).
    pub stop_at: Option<u64>,
    /// Stop checks made so far.
    pub checks: Cell<u64>,
    /// The attempt budget's refusal count, when counted.
    pub refusals: Option<Arc<AtomicU64>>,
    /// `max_export_work`, when bounded.
    pub max_work: Option<u64>,
    /// Export work counted so far.
    pub work: Cell<u64>,
    /// `max_attempt_evictions`.
    pub max_evictions: Option<u64>,
    /// The dependency cache's bound `D`, when not the default.
    pub dep_cache_bytes: Option<u64>,
    /// The sink's prefix.
    pub prefix: String,
    /// The archives' member templates.
    pub members: Vec<MemberTemplate>,
    /// The sink refuses its n-th output (counting from 0) with this.
    pub refuse_output: Option<(usize, Failure)>,
    /// The sink drops each output once it has seen it, keeping nothing.
    pub dropping: bool,
    /// Every check, work count and sink call, in order.
    pub log: RefCell<Vec<Event>>,
    /// Each sprite group's Sprites, as the export told them: the group's
    /// number, the Sprite's file and its path id.
    pub groups: RefCell<Vec<(usize, String, i64)>>,
}

impl Hooks for Engine {
    fn stopped(&self) -> bool {
        let n = self.checks.get();
        self.checks.set(n + 1);
        self.log.borrow_mut().push(Event::Check);
        self.stop_at.is_some_and(|at| n >= at)
    }

    fn refusals(&self) -> Option<u64> {
        self.refusals.as_ref().map(|r| r.load(Ordering::Acquire))
    }

    fn sprite_group(&self, group: usize, file: &unityrs_core::RawStr, path_id: i64) {
        let file = String::from_utf8_lossy(file.as_bytes()).into_owned();
        self.groups.borrow_mut().push((group, file, path_id));
    }

    fn export_work(&self, bytes: u64) -> Result<(), Reason> {
        self.log.borrow_mut().push(Event::Work(bytes));
        let total = self.work.get().saturating_add(bytes);
        self.work.set(total);
        match self.max_work {
            Some(max) if total > max => Err(Reason::plain(format!("limit(export_work, {max})"))),
            _ => Ok(()),
        }
    }
}

impl Engine {
    /// The private Environment's options for `context`, with `D` set.
    pub fn env(&self, context: FileContext) -> EnvOptions {
        let mut env = EnvOptions::batch(Arc::new(context));
        if let Some(d) = self.dep_cache_bytes {
            env.dep_cache_bytes = d;
        }
        env
    }
}

/// A cut of `input` over its view, under `meter` and `context`, inside the
/// attempt `engine` describes.
pub fn run_in(
    scene: &AtlasScene,
    input: &str,
    context: FileContext,
    meter: &Arc<Meter>,
    engine: &Engine,
) -> Result<Ran, Failure> {
    let members = view_of(scene, input);
    let x = members.iter().find(|m| m.id == input).unwrap().clone();
    let view = View {
        input: &x,
        members: &members,
    };
    let env = engine.env(context);
    let admit = Admit {
        env: &env,
        meter,
        hooks: engine,
        max_evictions: engine.max_evictions.unwrap_or(u64::MAX),
    };
    let registered = ngui::register_bytes(view, admit)?;
    let mut keys = KeyTable::new(meter);
    let mut outputs = Vec::new();
    let mut handed = Vec::new();
    let mut sink = |o: Output| {
        if let Some((n, f)) = &engine.refuse_output
            && handed.len() == *n
        {
            return Err(f.clone());
        }
        engine.log.borrow_mut().push(Event::Sink(o.key.clone()));
        handed.push(Handed {
            kind: o.kind,
            object: (
                String::from_utf8_lossy(o.serialized_file.as_bytes()).into_owned(),
                o.path_id,
            ),
            key: o.key.clone(),
            unit: o.unit.clone(),
            md5: o.md5,
        });
        if !engine.dropping {
            outputs.push(o);
        }
        Ok(())
    };
    let ctx = Context {
        registered: &registered,
        meter,
        input,
        hooks: engine,
        max_evictions: engine.max_evictions.unwrap_or(u64::MAX),
        prefix: &engine.prefix,
        members: &engine.members,
        keys: &mut keys,
        sink: &mut sink,
    };
    let cut = ngui::cut(ctx, &options(scene))?;
    let plan_skips = cut.plan_skips.len();
    let mut skips = cut.plan_skips.clone();
    skips.extend(cut.export_skips.iter().cloned());
    Ok(Ran {
        outputs,
        handed,
        skips,
        plan_skips,
        unresolved_scripts: cut.unresolved_scripts,
        sheets: cut.sheets.clone(),
        nested_containers: cut.nested_containers.clone(),
    })
}

/// A cut of `input` over its view, under `meter` and `context`.
pub fn run_with(
    scene: &AtlasScene,
    input: &str,
    context: FileContext,
    meter: &Arc<Meter>,
) -> Result<Ran, Failure> {
    run_in(scene, input, context, meter, &Engine::default())
}

/// A cut of `input` over its view, unmetered, which must not fail.
pub fn run(scene: &AtlasScene, input: &str) -> Ran {
    run_with(scene, input, FileContext::default(), &Meter::unlimited())
        .unwrap_or_else(|f| panic!("{input}: {f:?}"))
}

/// A cut of X of `s` whose `at`-th reservation of `M_X` is refused, with
/// the sink dropping each output: the operation that asked for it (none
/// when the cut made fewer), and the cut. The engine counts the refusal,
/// so the probe reads it as the budget's (`budget(input)`).
pub fn refused_at(s: &AtlasScene, at: u64) -> (Option<&'static str>, Result<Ran, Failure>) {
    refused_in(s, "x.bundle", at, true)
}

/// [`refused_at`] of `input`, the engine counting the refusal when
/// `counted`, else holding its count still, as for a refusal no budget
/// made (the allocator's), which the probe reads as `host_memory`.
pub fn refused_in(
    s: &AtlasScene,
    input: &str,
    at: u64,
    counted: bool,
) -> (Option<&'static str>, Result<Ran, Failure>) {
    let refuse = Arc::new(Refuse::new(at));
    let count = if counted {
        refuse.refusals.clone()
    } else {
        Arc::new(AtomicU64::new(0))
    };
    let engine = Engine {
        refusals: Some(count),
        dropping: true,
        ..Engine::default()
    };
    let meter = Meter::new(u64::MAX, refuse.clone(), Reservation::Input);
    let got = run_in(s, input, FileContext::default(), &meter, &engine);
    let site = *refuse.site.lock().unwrap();
    (site, got)
}

/// [`refused_in`], counted, with the refusal's stack (`<` and `>` taken
/// out): the read it was in, captured at the hook.
pub fn refused_with_stack(
    s: &AtlasScene,
    input: &str,
    at: u64,
) -> (Option<&'static str>, String, Result<Ran, Failure>) {
    let refuse = Arc::new(Refuse::new(at));
    let engine = Engine {
        refusals: Some(refuse.refusals.clone()),
        dropping: true,
        ..Engine::default()
    };
    let meter = Meter::new(u64::MAX, refuse.clone(), Reservation::Input);
    let got = run_in(s, input, FileContext::default(), &meter, &engine);
    let site = *refuse.site.lock().unwrap();
    let [stack, _] = refuse.stacks.lock().unwrap().clone();
    (site, stack, got)
}

/// A reservation as the budget was asked for it: its stack (`<` and `>`
/// taken out), its bytes, and the core's active vector-growth invocation.
pub type Asked = (String, u64, Option<u64>);

/// [`refused_with_stack`], with the refused reservation's bytes and the
/// reservation after it, granted ([`Refuse::followed`]).
pub fn refused_with_next(
    s: &AtlasScene,
    input: &str,
    at: u64,
) -> (
    Option<&'static str>,
    Asked,
    Option<Asked>,
    Result<Ran, Failure>,
) {
    let refuse = Arc::new(Refuse::followed(at));
    let engine = Engine {
        refusals: Some(refuse.refusals.clone()),
        dropping: true,
        ..Engine::default()
    };
    let meter = Meter::new(u64::MAX, refuse.clone(), Reservation::Input);
    let got = run_in(s, input, FileContext::default(), &meter, &engine);
    let site = *refuse.site.lock().unwrap();
    let [stack, _] = refuse.stacks.lock().unwrap().clone();
    let bytes = *refuse.bytes.lock().unwrap();
    let growth = *refuse.growth.lock().unwrap();
    let next = refuse.next.lock().unwrap().clone();
    (site, (stack, bytes, growth), next, got)
}

/// Each refusal's operation and core call ([`absorbed_at`]).
pub type Sites = [Option<(&'static str, String)>; 2];

/// A cut of `input` of `s` whose `at`-th reservation of `M_X` is counted
/// as refused and granted, as a refusal the core absorbed, and the next
/// refused uncounted ([`Refuse::absorbed`]): for each, the operation that
/// asked for it (none when the cut made fewer) and the core call it was in
/// ([`core_call`]); and the cut.
pub fn absorbed_at(s: &AtlasScene, input: &str, at: u64) -> (Sites, Result<Ran, Failure>) {
    two_refusals(s, input, Refuse::absorbed(at))
}

/// [`absorbed_at`], but the `at`-th refused for real, for the core to
/// absorb as it does ([`Refuse::refused_then`]).
pub fn refused_then_at(s: &AtlasScene, input: &str, at: u64) -> (Sites, Result<Ran, Failure>) {
    two_refusals(s, input, Refuse::refused_then(at))
}

fn two_refusals(s: &AtlasScene, input: &str, refuse: Refuse) -> (Sites, Result<Ran, Failure>) {
    let refuse = Arc::new(refuse);
    let engine = Engine {
        refusals: Some(refuse.refusals.clone()),
        dropping: true,
        ..Engine::default()
    };
    let meter = Meter::new(u64::MAX, refuse.clone(), Reservation::Input);
    let got = run_in(s, input, FileContext::default(), &meter, &engine);
    let [a, b] = refuse.stacks.lock().unwrap().clone().map(|s| core_call(&s));
    let sites = [
        refuse.site.lock().unwrap().map(|s| (s, a)),
        refuse.then_site.lock().unwrap().map(|s| (s, b)),
    ];
    (sites, got)
}

/// The core call a stack is in: the frame just inside the cutter's
/// innermost (a call the cutter made, which its probe takes on its own, or
/// a charge it made itself; a free function prints with no path), up to
/// any generic arguments, which run on after a space or `,` once `<` and
/// `>` are taken out ([`site`]).
pub fn core_call(stack: &str) -> String {
    let frames: Vec<&str> = stack
        .lines()
        .filter_map(|l| {
            let (n, rest) = l.trim_start().split_once(": ")?;
            n.parse::<u32>().is_ok().then_some(rest)
        })
        .collect();
    let call = frames
        .iter()
        .position(|f| f.starts_with("unityrs_export::"))
        .and_then(|cutter| frames.get(cutter.checked_sub(1)?))
        .copied()
        .unwrap_or("other");
    call.split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or(call)
        .to_owned()
}

/// A sheet of 32 x 32 RGBA pixels (a 4 KiB decode, more than any charge
/// before it), exported as a texture, and an atlas cutting two 2 x 2 crops
/// from it, with its JSON.
pub fn big_sheet() -> AtlasScene {
    big_sheet_named([b"s", b"t"])
}

/// [`big_sheet`] with its two sprites named `names`.
pub fn big_sheet_named(names: [&[u8]; 2]) -> AtlasScene {
    let sprites = vec![
        sprite(names[0], (0, 0, 2, 2), [0; 8]),
        sprite(names[1], (2, 2, 2, 2), [0; 8]),
    ];
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
            atlas(3, b"big", (0, 1), (0, 0), sprites, Vec::new()),
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
        options: CutOptions::textures("{name}.png"),
    }
}
