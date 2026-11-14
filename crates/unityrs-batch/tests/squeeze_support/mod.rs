//! X13's squeeze (`docs/plans/extract.md` X13): one sealed attempt run
//! through `run_attempt` below validation, its CR8 refusals read before
//! and after it (`docs/plans/batch.md` section 6), its records normalized
//! for comparison, the refusal predicates of the `M` and `D` passes, and
//! the exact exclusion inventory of the scenes CR7 keeps out.
#![allow(dead_code)]

#[path = "../attempt_support/mod.rs"]
mod attempt_support;
pub mod compare;
pub mod exclusions;
#[path = "../peek_support/mod.rs"]
mod peek_support;
pub mod tap;

pub use attempt_support::Harness;

use std::fmt::Write as _;

use serde_json::{Value as J, json};
use sha2::{Digest, Sha256};
use unityrs_batch::attempt::{AttemptLimits, ReasonCode, State};
use unityrs_batch::peek::word::{FailureCode, Outcome as PeekOutcome};
use unityrs_batch::reason::LimitKind as Kind;
use unityrs_core::budget::Reservation;
use unityrs_core::error::LimitKind;
use unityrs_export::ngui;
use unityrs_testkit::core_counters::refusals;
use unityrs_testkit::ngui::{AtlasScene, SceneBundle, scenes};

/// Above every testkit input's need: each pass's other limits.
pub const BIG: u64 = 64 << 20;
/// Above any testkit input's evictions: the eviction limit, nonbinding.
pub const BIG_EVICTIONS: u64 = 1 << 20;

/// The record fields a squeezed run may change without changing its
/// result: the peaks and timing, and the spool figures, which count the
/// summary line whose figures they are.
const UNCOMPARED: [&str; 7] = [
    "input_peak",
    "dependency_peak",
    "decode_work_peak",
    "ms",
    "spooled_part",
    "claimed_bytes",
    "evictions",
];

/// The limits of a pass: `M` squeezed (its four meanings together:
/// `run_attempt` sets meter memory and input bytes to `m`, decode work to
/// `4m` and the Input budget's limit to `m`), the others nonbinding.
#[must_use]
pub fn limits(m: u64, d: u64, evictions: u64, work: u64) -> AttemptLimits {
    AttemptLimits {
        m,
        d,
        max_attempt_evictions: evictions,
        max_export_work: work,
    }
}

/// A typed end reason in its records' spelling (`docs/BATCH_SPEC.md` 9.3),
/// empty for none: the one representation the squeeze compares, never a
/// `Debug` rendering.
#[must_use]
pub fn spelled(reason: Option<ReasonCode>) -> String {
    use ReasonCode as R;
    let Some(r) = reason else {
        return String::new();
    };
    match r {
        R::NotABundle => "not_a_bundle",
        R::ContainerError => "container_error",
        R::EntryError => "entry_error",
        R::BudgetInput => "budget(input)",
        R::BudgetDependency => "budget(dependency)",
        R::Limit(l) => return l.to_string(),
        R::HostMemory => "host_memory",
        R::HostIo => "host_io",
        R::SpoolFull => "spool_full",
        R::Internal => "internal",
    }
    .to_owned()
}

/// Whether a limit kind is one of `M`'s.
#[must_use]
pub fn m_kind(k: Kind) -> bool {
    matches!(k, Kind::InputMemory | Kind::InputBytes | Kind::DecodeWork)
}

/// A typed peek outcome, spelled.
fn peek_spelled(o: Option<PeekOutcome>) -> String {
    match o {
        None => "none".into(),
        Some(PeekOutcome::Pending) => "pending".into(),
        Some(PeekOutcome::Clear(n)) => format!("clear({n})"),
        Some(PeekOutcome::Disk(n)) => format!("disk({n})"),
        Some(PeekOutcome::Failed(FailureCode::NotABundle)) => "failed(not_a_bundle)".into(),
        Some(PeekOutcome::Failed(FailureCode::Container(d))) => {
            format!("failed(container_error, {})", d.name())
        }
        Some(PeekOutcome::Failed(FailureCode::Limit(k))) => format!("failed(limit({}))", k.name()),
    }
}

/// One run's result, as a child reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub state: String,
    /// The end reason, [`spelled`].
    pub reason: String,
    pub dependency: Option<u32>,
    /// CR8's `InputMemory`, `InputBytes`, `DecodeWork` and `Budget(Input)`.
    pub m_refusals: u64,
    /// CR8's `Budget(Dependency)`.
    pub d_refusals: u64,
    /// The allocator's refusals: the counting allocator's and the core's.
    pub allocator: u64,
    pub evictions: u64,
    pub export_work: u64,
    pub clear: bool,
    /// Normalized records, in order, each written file's bytes in its
    /// record as their sha256 (`sha256`), and the tap's annotations
    /// ([`tap::annotate`]).
    pub records: Vec<String>,
    /// The run's `M`.
    pub m: u64,
    /// The records file's length, which its export work counts.
    pub records_len: u64,
    /// The export work no row follows, as JSON.
    pub tail: String,
}

impl Outcome {
    #[must_use]
    pub fn to_json(&self) -> J {
        json!({
            "state": self.state, "reason": self.reason, "dependency": self.dependency,
            "m_refusals": self.m_refusals, "d_refusals": self.d_refusals,
            "allocator": self.allocator, "evictions": self.evictions,
            "export_work": self.export_work, "clear": self.clear,
            "records": self.records, "m": self.m, "records_len": self.records_len,
            "tail": self.tail,
        })
    }

    /// # Panics
    /// On a report not [`Self::to_json`]'s.
    #[must_use]
    pub fn from_json(v: &J) -> Self {
        let n = |k: &str| v[k].as_u64().unwrap();
        let strings = |k: &str| {
            v[k].as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect()
        };
        Self {
            state: v["state"].as_str().unwrap().to_owned(),
            reason: v["reason"].as_str().unwrap().to_owned(),
            dependency: v["dependency"].as_u64().map(|d| u32::try_from(d).unwrap()),
            m_refusals: n("m_refusals"),
            d_refusals: n("d_refusals"),
            allocator: n("allocator"),
            evictions: n("evictions"),
            export_work: n("export_work"),
            clear: v["clear"].as_bool().unwrap(),
            records: strings("records"),
            m: n("m"),
            records_len: n("records_len"),
            tail: v["tail"].as_str().unwrap().to_owned(),
        }
    }

    /// Whether its records, written bytes among them, are `other`'s, in
    /// order.
    #[must_use]
    pub fn same_result(&self, other: &Self) -> bool {
        self.state == other.state
            && self.reason == other.reason
            && self.records == other.records
            && self.tail == other.tail
    }

    /// The reasons its records name, wrappers' inner reasons among them.
    fn reasons(&self) -> impl Iterator<Item = String> + '_ {
        self.records.iter().filter_map(|r| {
            let v: J = serde_json::from_str(r).ok()?;
            v["reason"].as_str().map(str::to_owned)
        })
    }

    /// Whether a record names an `M` refusal: `budget(input)` or a
    /// `limit(...)` of `input_memory`, `input_bytes` or `decode_work`.
    #[must_use]
    pub fn names_m(&self) -> bool {
        self.reasons().any(|r| m_reason(&r))
    }

    /// Whether a record names `budget(dependency)`.
    #[must_use]
    pub fn names_d(&self) -> bool {
        self.reasons().any(|r| r.contains("budget(dependency)"))
    }
}

/// An `M` reason, alone or as a `dependency_failed` wrapper's inner one.
#[must_use]
pub fn m_reason(r: &str) -> bool {
    [
        "budget(input)",
        "limit(input_memory,",
        "limit(input_bytes,",
        "limit(decode_work,",
    ]
    .iter()
    .any(|m| r.contains(m))
}

/// The `M` pass's predicate: a refusal of `M`, absorbed or surfaced, in
/// CR8's counts or named by a record.
#[must_use]
pub fn refused_m(o: &Outcome) -> bool {
    o.m_refusals > 0 || m_reason(&o.reason) || o.names_m()
}

/// The `D` pass's predicate: a `Budget(Dependency)` refusal, counted or
/// named, or an eviction.
#[must_use]
pub fn refused_d(o: &Outcome) -> bool {
    o.d_refusals > 0 || o.evictions > 0 || o.reason == "budget(dependency)" || o.names_d()
}

/// Normalizes one record line: the [`UNCOMPARED`] fields left out, and
/// the bytes of the file it names, if any, in as `sha256`.
fn normalize(line: &J, dir: &std::path::Path) -> String {
    let mut v = line.clone();
    strip(&mut v);
    if let (Some(f), Some(map)) = (line["file"].as_str(), v.as_object_mut()) {
        let bytes = std::fs::read(dir.join(f)).unwrap();
        let hex = Sha256::digest(&bytes)
            .iter()
            .fold(String::new(), |mut hex, b| {
                let _ = write!(hex, "{b:02x}");
                hex
            });
        map.insert("sha256".into(), J::String(hex));
    }
    v.to_string()
}

/// A member variant's name: its scene's, with this suffix.
pub const MEMBERS: &str = "+members";

/// The member templates of a `+members` variant: every kind into one of
/// two archives, so its export work counts each member's expansion.
///
/// # Panics
/// On a template that does not parse (a test helper).
#[must_use]
pub fn templates() -> Vec<ngui::MemberTemplate> {
    use ngui::TemplateKind as K;
    [
        (0, K::Texture, "textures/{safe_name}_{path_id}.png"),
        (1, K::Sprite, "sprites/{safe_name}_{path_id}.png"),
        (
            0,
            K::AtlasSprite,
            "atlases/{safe_atlas}/{safe_sprite}_{path_id}.png",
        ),
        (1, K::AtlasJson, "atlases/{safe_atlas}_{path_id}.json"),
    ]
    .into_iter()
    .map(|(archive, kind, path)| ngui::MemberTemplate {
        archive,
        kind,
        path: ngui::Template::parse(path, kind).unwrap(),
    })
    .collect()
}

fn strip(v: &mut J) {
    if let Some(map) = v.as_object_mut() {
        for k in UNCOMPARED {
            map.remove(k);
        }
        for (_, inner) in map.iter_mut() {
            strip(inner);
        }
    }
}

/// Runs `input` of `scene` once under `limits`, in this process, its CR8
/// counts read just before and just after `run_attempt`.
///
/// # Panics
/// On a harness failure (a test helper).
#[must_use]
pub fn run(scene: &AtlasScene, input: &str, limits: AttemptLimits, members: bool) -> Outcome {
    let h = Harness::new(scene, input);
    let m = limits.m;
    let templates = if members { templates() } else { Vec::new() };
    let archives: &[&str] = if members {
        &["members-a.zip", "members-b.zip"]
    } else {
        &[]
    };
    let alloc_before = unityrs_batch::alloc::refusals();
    let before = refusals();
    let (end, events) = tap::tapped(|| h.run_members(limits, &ngui::NoHooks, &templates, archives));
    let counted = refusals().since(&before);
    let alloc = unityrs_batch::alloc::refusals() - alloc_before;
    let mut rows = h.rows();
    let tail = tap::annotate(&mut rows, &events);
    let records_len =
        std::fs::metadata(h.directory.path().join("records.ndjson")).map_or(0, |m| m.len());
    let m_refusals = [
        LimitKind::InputMemory,
        LimitKind::InputBytes,
        LimitKind::DecodeWork,
    ]
    .iter()
    .map(|&k| counted.limit(k))
    .sum::<u64>()
        + counted.budget(Reservation::Input);
    Outcome {
        state: format!("{:?}", end.state),
        reason: spelled(end.reason),
        dependency: end.dependency,
        m_refusals,
        d_refusals: counted.budget(Reservation::Dependency),
        allocator: alloc
            + counted.allocator(Reservation::Input)
            + counted.allocator(Reservation::Dependency),
        evictions: end.figures.evictions,
        export_work: end.figures.export_work,
        clear: end.accounts.is_clear() && end.slot_releasable,
        records: rows
            .iter()
            .map(|r| normalize(r, h.directory.path()))
            .collect(),
        m,
        records_len,
        tail: J::from(tail).to_string(),
    }
}

/// Whether `state` is a run failure: never a squeeze result.
#[must_use]
pub fn run_failure(o: &Outcome) -> bool {
    o.state == format!("{:?}", State::RunFailure)
}

/// The testkit's NGUI scenes, and one more whose run evicts at `M = D =`
/// `M`'s need (the eviction pass's scene): two dependencies of 400 KB
/// each, which registration reads whole, so a `D` below their sum evicts.
///
/// # Panics
/// When a scene does not build (a test helper).
#[must_use]
pub fn catalogue() -> Vec<(&'static str, AtlasScene)> {
    let mut all = scenes().unwrap();
    let mut evicting = attempt_support::simple(&[]);
    evicting.bundles[0].deps = vec!["a".into(), "b".into()];
    for id in ["a", "b"] {
        evicting.bundles.push(SceneBundle {
            id: id.into(),
            data: unityrs_testkit::env::bundle(&[("resource", vec![0; 400_000])]),
            deps: vec![],
        });
    }
    all.push(("evicting_dependencies", evicting));
    all
}

/// One sealed peek of `data` under `M` = `m`, in this process, its CR8
/// counts read around it: `(outcome, m refusals, allocator refusals,
/// whether its accounts are clear, its peek file's bytes)`.
///
/// # Panics
/// On a harness failure (a test helper).
#[must_use]
pub fn peek(data: &[u8], m: u64) -> J {
    let mut h = peek_support::Harness::new(data);
    let alloc_before = unityrs_batch::alloc::refusals();
    let before = refusals();
    let end = h.run(0, 1, m, &ngui::NoHooks);
    let counted = refusals().since(&before);
    let m_refusals = [
        LimitKind::InputMemory,
        LimitKind::InputBytes,
        LimitKind::DecodeWork,
    ]
    .iter()
    .map(|&k| counted.limit(k))
    .sum::<u64>()
        + counted.budget(Reservation::Input);
    let file = h.file.as_ref().map(|f| std::fs::read(f.path()).unwrap());
    json!({
        "state": format!("{:?}", end.state),
        "outcome": peek_spelled(end.outcome),
        "m_limit": matches!(
            end.outcome,
            Some(PeekOutcome::Failed(FailureCode::Limit(k))) if m_kind(k)
        ),
        "m_refusals": m_refusals,
        "allocator": unityrs_batch::alloc::refusals() - alloc_before
            + counted.allocator(Reservation::Input),
        "clear": end.accounts.is_clear() && end.slot_releasable,
        "file": file,
    })
}
