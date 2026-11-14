//! The controller's hand-over (`docs/plans/batch.md` 2.4, 2.11): what the
//! finisher may read, frozen at one instant, and nothing it must not drop.
//! The controller keeps every owner (job and stage slots, the Execution and
//! its runtime, publication's tasks) and parks them; this value holds only
//! read-only facts and shared handles to what the closed commit gate froze.
use crate::{
    lifetime::Boot,
    phase1::Facts,
    progress::Phase,
    publish::Owned,
    reason::Code,
    sink::directory::Directory,
    spool::{FileSystem, workspace::Workspace},
    start::spool::Started,
    sys::Shared,
    winners::Handoff,
};
use std::{path::PathBuf, sync::Arc, time::Instant};

/// Why the finisher runs. The stop word holds a stop's reason; this says
/// only which of 7.4's rows the controller's last check left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishCause {
    /// After phase 7, the controller's last check saw no stop: a later stop
    /// changes only the finisher's try budget, never the head.
    Normal,
    /// The controller saw a stop at a check, or requested one itself.
    Stop,
    /// A 69, 7 or 74 in preflight or at the `running` write: no head, and
    /// the code yields only to a more severe stop's (9.1).
    Terminal(Terminal),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terminal {
    pub code: Code,
    pub reason: &'static str,
}

/// Phase 6's hand-off, once phase 7 began.
#[allow(clippy::large_enum_variant)] // Built in place at the hand-over: a box would allocate after the head.
pub enum Selected {
    /// Publication's shared owner; its gate is closed.
    Owned(Shared<Owned>),
    /// A stop while the outcomes were being created: the parts, and the
    /// outcomes files read as a view (missing or short reads `not_written`).
    Parts {
        workspace: Workspace,
        handoff: Handoff,
    },
}
impl Selected {
    #[must_use]
    pub fn handoff(&self) -> &Handoff {
        match self {
            Self::Owned(owned) => &owned.handoff,
            Self::Parts { handoff, .. } => handoff,
        }
    }
    #[must_use]
    pub fn workspace(&self) -> &Workspace {
        match self {
            Self::Owned(owned) => &owned.workspace,
            Self::Parts { workspace, .. } => workspace,
        }
    }
}

/// The spool the finisher reads: `done/` for its one stat and the
/// committed records, the run directory for an outcomes view.
pub struct Spool {
    /// The run's directory, where phase 7's outcome files are.
    pub run: PathBuf,
    pub done: PathBuf,
    pub fs: Arc<dyn FileSystem>,
    /// The library's share of the spool's lock and ledger; a process host
    /// holds its own until `_exit`.
    pub lease: Option<Arc<Started>>,
}

pub struct HandOver {
    pub cause: FinishCause,
    /// The phase the controller reached (the head's `phase`).
    pub phase: Phase,
    /// Whether the `running` head was acknowledged (7.4's first rows).
    pub running: bool,
    /// None only before the controller's facts existed (a terminal end).
    pub facts: Option<Shared<Facts>>,
    /// The hand-over slot: the input whose commit was under way, if any.
    pub pending: Option<u32>,
    /// The hand-over instant: dispatch instants are monotonic, so a
    /// dispatched `stopped` row's `ms` is measured on this one.
    pub at: Instant,
    /// The same instant on `CLOCK_BOOTTIME`, where the host has it: the
    /// finisher's budget arithmetic is on that clock.
    pub boot: Option<Boot>,
    pub selected: Option<Selected>,
    pub directory: Option<Arc<Directory>>,
    pub spool: Option<Spool>,
}
impl HandOver {
    /// A hand-over at `phase`, its instant sampled now.
    #[must_use]
    pub fn new(cause: FinishCause, phase: Phase) -> Self {
        Self {
            cause,
            phase,
            running: false,
            facts: None,
            pending: None,
            at: Instant::now(),
            boot: Boot::now().ok(),
            selected: None,
            directory: None,
            spool: None,
        }
    }
}
