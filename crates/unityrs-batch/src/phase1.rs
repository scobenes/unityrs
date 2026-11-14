//! Spec-derived retained state. Construct this inside `alloc::measured`,
//! beside validation, for both execution and `--check`. No filesystem calls,
//! environment reads, threads, retained view closures or duplicated path table.
use crate::sys::Shared;
use crate::{
    attempt,
    peek::word::{Outcome, Word},
    spec::{self, model::Spec},
    spool::{Directory, DirectoryClaim, FileClaim, OwnedFile},
    stage::StageEnd,
    view::{Graph, Walker},
};
use std::{sync::Arc, time::Instant};

pub struct PreparedRun {
    pub(crate) memory: Arc<crate::job_memory::Plan>,
    pub spec_sha256: [u8; 32],
    pub spec: Arc<Spec>,
    pub graph: Arc<Graph>,
    pub extraction: Arc<attempt::Prepared>,
    pub winners: crate::winners::Prepared,
    /// Archive indices, not duplicated final key strings.
    pub archive_keys: Vec<u32>,
    /// Worker-published words are deliberately separate from received facts.
    pub peek_words: Arc<[Word]>,
    pub bundles: Vec<BundleState>,
    pub files: Vec<BundleOwners>,
    pub inputs: Vec<InputState>,
    pub uncommitted: Bits,
    pub peek_members: Bits,
    pub peek_queue: Vec<u32>,
    pub ready: Vec<u32>,
    pub failures: Vec<u32>,
    pub walker: Walker,
    pub active: Vec<ActiveSlot>,
    pub stages_active: Vec<StageSlot>,
}

#[derive(Default)]
pub struct BundleState {
    /// None until this staging completion is received.
    pub stage: Option<StageEnd>,

    /// None until the end is received, even if the word was published.
    pub peek: Option<PeekFacts>,
}
#[derive(Default)]
pub struct BundleOwners {
    pub staged: Option<FileClaim>,
    pub peek: Option<FileClaim>,
}
/// Received figures only. Account audits and owners remain in bounded active
/// slots, never duplicated in the per-bundle table.
#[derive(Clone, Copy, Debug)]
pub struct PeekFacts {
    pub input_peak: u64,
    pub decode_work: u64,
    pub outcome: Option<Outcome>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputPhase {
    #[default]
    Pending,
    Ready,
    Running,
    Committed,
}
#[derive(Default)]
pub struct InputState {
    pub phase: InputPhase,
    /// The selected failure's bundle index; no copied id or reason strings.
    pub failure_bundle: Option<u32>,
    pub received: Option<AttemptFacts>,
    pub committed: Option<DirectoryClaim>,
}
#[derive(Clone, Copy, Debug)]
pub struct AttemptFacts {
    pub state: attempt::State,
    pub figures: crate::records::Figures,
    pub spooled_part: u64,
    pub claimed_bytes: u64,
    pub outputs: u64,
    pub skips_expected: u64,
    pub diagnostics: u64,
    pub code: crate::reason::Code,
}
impl AttemptFacts {
    pub(crate) fn from_end(e: &attempt::AttemptEnd) -> Self {
        Self {
            state: e.state,
            figures: e.figures,
            spooled_part: e.spooled_part,
            claimed_bytes: e.claimed_bytes,
            outputs: e.outputs,
            skips_expected: e.skips_expected,
            diagnostics: e.diagnostics,
            code: e.code,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Attempt,
    Peek,
}
#[derive(Default)]
pub struct ActiveSlot {
    pub job: Option<(JobKind, u32)>,
    pub dispatched: Option<Instant>,
    pub directory: Option<Shared<Directory>>,
    pub peek_file: Option<Shared<crate::sys::Lock<Option<OwnedFile>>>>,
}
#[derive(Default)]
pub struct StageSlot {
    pub(crate) task: Option<crate::execution::Task<(crate::stage::local::LocalFinish, OwnedFile)>>,
    pub bundle: Option<u32>,
    pub file: Option<OwnedFile>,
}

/// Controller-owned fixed-capacity bit set. Out-of-range operations fail
/// instead of silently clearing another input or growing after measurement.
#[derive(Debug)]
pub struct Bits {
    words: Vec<u64>,
    len: usize,
}
impl Bits {
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        if index >= self.len {
            return None;
        }
        self.words
            .get(index / 64)
            .map(|w| w & (1 << (index % 64)) != 0)
    }
    /// Returns the previous bit, allowing duplicate/unknown end checks.
    pub fn set(&mut self, index: usize, value: bool) -> Option<bool> {
        if index >= self.len {
            return None;
        }
        let word = self.words.get_mut(index / 64)?;
        let mask = 1 << (index % 64);
        let old = *word & mask != 0;
        if value {
            *word |= mask;
        } else {
            *word &= !mask;
        }
        Some(old)
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Debug)]
pub enum PreparationError {
    Template(unityrs_export::ngui::TemplateError),
    Winners(crate::phase::Error),
}

/// Validation already checked these templates. The error is retained in the
/// library API for callers constructing a `Validated` value themselves.
pub fn prepare(validated: spec::Validated) -> Result<PreparedRun, PreparationError> {
    let spec::Validated {
        spec_sha256,
        spec,
        graph,
    } = validated;
    let extraction = attempt::Prepared::from_spec(&spec).map_err(PreparationError::Template)?;
    let winners = crate::winners::Prepared::from_spec(&spec).map_err(PreparationError::Winners)?;
    let n = spec.bundle_count();
    let inputs = spec.input_count();
    let walker = Walker::with_capacity(n, (spec.max_view_bundles as usize).min(n));
    let memory = crate::job_memory::Plan::new(&spec).map_err(PreparationError::Winners)?;
    Ok(PreparedRun {
        memory: Arc::new(memory),
        spec_sha256,
        archive_keys: (0..spec.archives.len())
            .map(|i| u32::try_from(i).unwrap_or(u32::MAX))
            .collect(),
        peek_words: (0..n).map(|_| Word::default()).collect(),
        bundles: (0..n).map(|_| BundleState::default()).collect(),
        inputs: (0..inputs).map(|_| InputState::default()).collect(),
        files: (0..n).map(|_| BundleOwners::default()).collect(),
        uncommitted: Bits::new(inputs),
        peek_members: Bits::new(n),
        peek_queue: Vec::with_capacity(n),
        ready: Vec::with_capacity(inputs),
        failures: Vec::with_capacity(inputs),
        walker,
        active: (0..spec.threads).map(|_| ActiveSlot::default()).collect(),
        stages_active: (0..spec.download_concurrency)
            .map(|_| StageSlot::default())
            .collect(),
        spec: Arc::new(spec),
        graph: Arc::new(graph),
        extraction: Arc::new(extraction),
        winners,
    })
}
