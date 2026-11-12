//! Provisional artifact contract, until B7's manifest (`BATCH_SPEC` 7.3).
use super::Text;
use crate::digest::Hashes;
use crate::reason::LeftOut;
use unityrs_export::ngui::{OutputKind, Picture, SkipOutput};

/// A classifier's prepared reason, or a wrapper with a separately cut
/// argument. Wrappers themselves are never truncated away.
#[derive(Clone, Copy, Debug)]
pub enum Reason<'a> {
    Text(Text<'a>),
    Argument { code: &'static str, value: Text<'a> },
    Dependency { id: Text<'a>, reason: Text<'a> },
}
/// One copied output. `plan_index` determines `file`; it need not be dense.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub key: &'a str,
    pub unit: Option<&'a str>,
    pub input: &'a str,
    pub serialized_file: Text<'a>,
    pub kind: OutputKind,
    pub path_id: i64,
    pub hashes: Hashes,
    pub picture: Option<&'a Picture>,
    pub size: u64,
    pub plan_index: u64,
    pub cut: bool,
}
/// One line per expanded archive member, in expansion order.
#[derive(Clone, Copy, Debug)]
pub struct Member<'a> {
    pub archive: u64,
    pub path: Text<'a>,
    pub key: &'a str,
    pub input: &'a str,
    pub md5: &'a [u8; 16],
    pub reason: Option<LeftOut>,
    pub cut: bool,
}
/// A plan or export skip; the caller keeps those subsequences in order.
#[derive(Clone, Copy, Debug)]
pub struct Skip<'a> {
    pub input: &'a str,
    pub serialized_file: Text<'a>,
    pub path_id: i64,
    /// A ClassIDType name or signed decimal id, at most 64 ASCII bytes.
    pub class: &'a str,
    pub output: SkipOutput,
    pub sprite: Option<u64>,
    pub reason: Reason<'a>,
    pub detail: Option<Text<'a>>,
    pub cut: bool,
}
#[derive(Clone, Copy, Debug)]
pub enum DiagnosticKind {
    SameNamedCab,
    UndeclaredDependency,
    NestedContainer,
}
impl DiagnosticKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::SameNamedCab => "same_named_cab",
            Self::UndeclaredDependency => "undeclared_dependency",
            Self::NestedContainer => "nested_container",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Diagnostic<'a> {
    pub kind: DiagnosticKind,
    pub input: &'a str,
    pub bundle: Option<&'a str>,
    pub name: Text<'a>,
    pub cut: bool,
}
/// Fixed-size end-message figures, flattened into the terminal summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Figures {
    pub input_peak: u64,
    pub dependency_peak: u64,
    pub evictions: u64,
    pub export_work: u64,
    pub decode_work_peak: u64,
    pub ms: u64,
    pub skips: u64,
    pub unresolved_scripts: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Complete,
    CompleteWithSkips,
    Failed,
}
impl Status {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::CompleteWithSkips => "complete_with_skips",
            Self::Failed => "failed",
        }
    }
}
/// Terminal input summary. `None` figures means failure before dispatch.
/// Failure records have zero skips and `unresolved_scripts`, as 7.3 requires.
#[derive(Clone, Copy, Debug)]
pub struct Summary<'a> {
    pub input: &'a str,
    pub status: Status,
    pub reason: Option<Reason<'a>>,
    pub detail: Option<Text<'a>>,
    pub figures: Option<Figures>,
    pub spooled_part: u64,
    pub claimed_bytes: u64,
    pub cut: bool,
}
/// Typed lines: summary is written only by `finish`/`write_failure_record`.
#[derive(Clone, Copy, Debug)]
pub enum Record<'a> {
    Candidate(Candidate<'a>),
    Member(Member<'a>),
    Skip(Skip<'a>),
    Diagnostic(Diagnostic<'a>),
}
