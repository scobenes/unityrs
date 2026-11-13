//! Fixed-size messages. Nothing here owns a permit or extracted buffer.
use crate::{
    budget::Snapshot,
    reason::{Code, Limit},
    records::Figures,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Done,
    Failed,
    Stopped,
    RunFailure,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonCode {
    NotABundle,
    ContainerError,
    EntryError,
    BudgetInput,
    BudgetDependency,
    Limit(Limit),
    HostMemory,
    HostIo,
    SpoolFull,
    Internal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailCode {
    Detect,
    OutOfBounds,
    Malformed,
    Unsupported,
    Decompress,
    Version,
    TypeIndex,
    Tree,
    AssetBundle,
    Read,
    NotFound,
    SizeBytes,
    Reserve,
    Enospc,
    Edquot,
}
impl DetailCode {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Detect => "detect",
            Self::OutOfBounds => "out_of_bounds",
            Self::Malformed => "malformed",
            Self::Unsupported => "unsupported",
            Self::Decompress => "decompress",
            Self::Version => "version",
            Self::TypeIndex => "type_index",
            Self::Tree => "tree",
            Self::AssetBundle => "asset_bundle",
            Self::Read => "read",
            Self::NotFound => "not_found",
            Self::SizeBytes => "size_bytes",
            Self::Reserve => "reserve",
            Self::Enospc => "ENOSPC",
            Self::Edquot => "EDQUOT",
        }
    }
    pub(crate) fn parse(s: &str) -> Option<Self> {
        [
            Self::Detect,
            Self::OutOfBounds,
            Self::Malformed,
            Self::Unsupported,
            Self::Decompress,
            Self::Version,
            Self::TypeIndex,
            Self::Tree,
            Self::AssetBundle,
            Self::Read,
            Self::NotFound,
            Self::SizeBytes,
            Self::Reserve,
            Self::Enospc,
            Self::Edquot,
        ]
        .into_iter()
        .find(|c| c.name() == s)
    }
}
/// Up to 1,024 source bytes repaired by the classifier may occupy 3,072
/// UTF-8 bytes. Store those without a second source-byte cut.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EntryDetail {
    bytes: [u8; 3072],
    len: u16,
}
impl Default for EntryDetail {
    fn default() -> Self {
        Self {
            bytes: [0; 3072],
            len: 0,
        }
    }
}
impl EntryDetail {
    pub(super) fn prepared(s: &str) -> Option<Self> {
        let mut out = Self::default();
        out.bytes.get_mut(..s.len())?.copy_from_slice(s.as_bytes());
        out.len = u16::try_from(s.len()).ok()?;
        Some(out)
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(self.bytes.get(..usize::from(self.len)).unwrap_or_default())
            .unwrap_or_default()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptEnd {
    pub input_index: u64,
    pub state: State,
    pub reason: Option<ReasonCode>,
    pub detail: Option<DetailCode>,
    /// Index in `AttemptView.members`, never a copied dependency id.
    pub dependency: Option<u32>,
    /// Entry-error detail, including its code and the space before its path.
    pub entry_detail: EntryDetail,
    pub cut: bool,
    pub code: Code,
    pub figures: Figures,
    pub spooled_part: u64,
    pub claimed_bytes: u64,
    pub outputs: u64,
    pub skips_expected: u64,
    pub diagnostics: u64,
    pub accounts: Snapshot,
    /// False after a leak, even if another run failure also occurred.
    pub slot_releasable: bool,
}
impl AttemptEnd {
    pub(crate) fn new(input_index: u64) -> Self {
        Self {
            input_index,
            state: State::Done,
            reason: None,
            detail: None,
            dependency: None,
            entry_detail: EntryDetail::default(),
            cut: false,
            code: Code::Complete,
            figures: Figures::default(),
            spooled_part: 0,
            claimed_bytes: 0,
            outputs: 0,
            skips_expected: 0,
            diagnostics: 0,
            accounts: Snapshot::default(),
            slot_releasable: true,
        }
    }
    pub(crate) fn run_failure(&mut self, reason: ReasonCode) {
        let code = match reason {
            ReasonCode::HostIo | ReasonCode::HostMemory => Code::Host,
            ReasonCode::SpoolFull => Code::Budget,
            _ => Code::Internal,
        };
        if self.state == State::RunFailure && self.code.severity() < code.severity() {
            return;
        }
        self.state = State::RunFailure;
        self.reason = Some(reason);
        self.detail = None;
        self.entry_detail = EntryDetail::default();
        self.dependency = None;
        self.cut = false;
        self.code = code;
    }
}

impl std::fmt::Debug for EntryDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}
