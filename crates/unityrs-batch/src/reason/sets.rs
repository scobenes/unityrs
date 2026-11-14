//! 9.3's closed reason sets, one type per place, and 9.2's map from a
//! normal end's outcomes to its code.

use std::fmt;

use super::{Budget, Code, InputLimit, KeyRule, LimitClass, LimitKind, ObjectLimit};

/// An input `not_run` (7.3).
pub const STOPPED: &str = "stopped";
/// A head read that finds a later attempt's head (exit 7, 7.2).
pub const SUPERSEDED: &str = "superseded";
/// Validation's reason (exit 78); its detail is `BadSpec::detail`.
pub const BAD_SPEC: &str = "bad_spec";

/// Writes `name(arg, ...)`.
fn call(f: &mut fmt::Formatter<'_>, name: &str, args: &[&dyn fmt::Display]) -> fmt::Result {
    f.write_str(name)?;
    f.write_str("(")?;
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{a}")?;
    }
    f.write_str(")")
}

/// A bundle's or an input's reason: transport, data or budget (9.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InputReason {
    /// `download_failed`.
    DownloadFailed,
    /// `redirect_refused`.
    RedirectRefused,
    /// `source_missing`.
    SourceMissing,
    /// `source_not_regular`.
    SourceNotRegular,
    /// `source_changed`.
    SourceChanged,
    /// `integrity_mismatch`.
    IntegrityMismatch,
    /// `not_a_bundle`.
    NotABundle,
    /// `container_error`.
    ContainerError,
    /// `entry_error`.
    EntryError,
    /// `dependency_failed(<id>, <reason>)`: a member of the view failed
    /// (`batch.md` 2.3 rule 1).
    DependencyFailed {
        /// The member's id.
        id: String,
        /// The member's own reason.
        reason: Box<InputReason>,
    },
    /// `limit(<kind>, <value>)`, of any kind but `texture_pixels`.
    Limit(InputLimit),
    /// `budget(input)` or `budget(dependency)`.
    Budget(Budget),
}

impl InputReason {
    /// Whether 9.2 counts it a budget (exit 5): `budget(...)`, a budget
    /// `limit`, or `dependency_failed` whose inner reason is one of them
    /// but `attempt_evictions` and `export_work`, which always fail the
    /// input by their own reason.
    #[must_use]
    pub fn is_budget(&self) -> bool {
        match self {
            Self::Budget(_) => true,
            Self::Limit(l) => l.get().kind.is_budget(),
            Self::DependencyFailed { reason, .. } => match &**reason {
                Self::Limit(l) => {
                    let kind = l.get().kind;
                    kind.is_budget()
                        && !matches!(kind, LimitKind::AttemptEvictions | LimitKind::ExportWork)
                }
                inner => inner.is_budget(),
            },
            _ => false,
        }
    }

    /// Whether it is a transport reason (9.3's first row).
    #[must_use]
    pub fn is_transport(&self) -> bool {
        matches!(
            self,
            Self::DownloadFailed
                | Self::RedirectRefused
                | Self::SourceMissing
                | Self::SourceNotRegular
                | Self::SourceChanged
        )
    }

    /// Its code at a normal end (9.2): 5 for a budget, else 4.
    #[must_use]
    pub fn code(&self) -> Code {
        if self.is_budget() {
            Code::Budget
        } else {
            Code::Incomplete
        }
    }
}

impl fmt::Display for InputReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DownloadFailed => "download_failed",
            Self::RedirectRefused => "redirect_refused",
            Self::SourceMissing => "source_missing",
            Self::SourceNotRegular => "source_not_regular",
            Self::SourceChanged => "source_changed",
            Self::IntegrityMismatch => "integrity_mismatch",
            Self::NotABundle => "not_a_bundle",
            Self::ContainerError => "container_error",
            Self::EntryError => "entry_error",
            Self::DependencyFailed { id, reason } => {
                return call(f, "dependency_failed", &[id, reason]);
            }
            Self::Limit(l) => return l.get().fmt(f),
            Self::Budget(b) => return b.fmt(f),
        })
    }
}

/// `unsupported_platform`'s argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    /// `switch`.
    Switch,
    /// `xbox360`.
    Xbox360,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Switch => "switch",
            Self::Xbox360 => "xbox360",
        })
    }
}

// Each variant and its test samples are declared together. A new variant
// cannot compile without samples; the classification table test consumes
// this generated list, so it must grow along with the enum.
macro_rules! object_skips {
    ($( $(#[$doc:meta])* $variant:ident $(($arg:ty))? => [$($sample:expr),+]; )*) => {
        /// An object's skip (9.3).
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub enum ObjectSkip {
            $( $(#[$doc])* $variant $(($arg))?, )*
        }

        #[cfg(test)]
        pub(super) fn every_object_skip() -> Vec<ObjectSkip> {
            vec![$($($sample),+),*]
        }
    };
}

object_skips! {
    /// `budget(input)`; `budget(dependency)` is never an object's skip
    /// (9.2).
    BudgetInput => [ObjectSkip::BudgetInput];
    /// `limit(<kind>, <value>)`: a budget kind of the core's
    /// (`input_memory`, `input_bytes`, `decode_work`) or a data kind.
    Limit(ObjectLimit) => [ObjectSkip::Limit(sample_limit(LimitKind::InputMemory)),
        ObjectSkip::Limit(sample_limit(LimitKind::TexturePixels))];
    /// `outside_view(<cab> | <path>)`.
    OutsideView(String) => [ObjectSkip::OutsideView("CAB-1".into())];
    /// `bad_pointer`.
    BadPointer => [ObjectSkip::BadPointer];
    /// `decode_error`.
    DecodeError => [ObjectSkip::DecodeError];
    /// `decoder_panic`.
    DecoderPanic => [ObjectSkip::DecoderPanic];
    /// `unsupported_format(<name>)`: UnityPy 1.25.2's `TextureFormat`
    /// member name, or the decimal value of one it has no name for.
    UnsupportedFormat(FormatName) => [ObjectSkip::UnsupportedFormat(FormatName::Named("ETC2_RGBA8".into())),
        ObjectSkip::UnsupportedFormat(FormatName::Value(4_294_967_296))];
    /// `unsupported_platform(switch | xbox360)`.
    UnsupportedPlatform(Platform) => [ObjectSkip::UnsupportedPlatform(Platform::Switch)];
    /// `empty_image`.
    EmptyImage => [ObjectSkip::EmptyImage];
    /// `duplicate_name`.
    DuplicateName => [ObjectSkip::DuplicateName];
    /// `duplicate_key`.
    DuplicateKey => [ObjectSkip::DuplicateKey];
    /// `unwritable_key(<rule>)`.
    UnwritableKey(KeyRule) => [ObjectSkip::UnwritableKey(KeyRule::DotSegment)];
    /// `empty_rect`.
    EmptyRect => [ObjectSkip::EmptyRect];
    /// `unnamed_atlas`.
    UnnamedAtlas => [ObjectSkip::UnnamedAtlas];
    /// `reference_atlas`.
    ReferenceAtlas => [ObjectSkip::ReferenceAtlas];
    /// `sheet_not_found`.
    SheetNotFound => [ObjectSkip::SheetNotFound];
    /// `ngui_layout_unsupported`.
    NguiLayoutUnsupported => [ObjectSkip::NguiLayoutUnsupported];
    /// `no_typetree`.
    NoTypetree => [ObjectSkip::NoTypetree];
    /// `json_unwritable`.
    JsonUnwritable => [ObjectSkip::JsonUnwritable];
}

#[cfg(test)]
fn sample_limit(kind: LimitKind) -> ObjectLimit {
    ObjectLimit::new(super::Limit { kind, value: 1 }).expect("an object's limit")
}

/// `unsupported_format`'s argument (9.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FormatName {
    /// UnityPy 1.25.2's `TextureFormat` member name.
    Named(String),
    /// The value as read, for one that enum has no member for (a format
    /// outside a 32-bit `int` among them).
    Value(i128),
}

impl fmt::Display for FormatName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(n) => f.write_str(n),
            Self::Value(v) => write!(f, "{v}"),
        }
    }
}

/// An object skip's class (9.3's table): what decides whether it sets a
/// code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SkipClass {
    /// A budget's refusal: 5.
    Budget,
    /// An *expected skip*: the object reads correctly, and version 1 leaves
    /// it without an output by its fixed scope or by a rule about the
    /// object's own content or name. It sets no code.
    Expected,
    /// The data could not be read, resolved or decoded: 3.
    Unreadable,
    /// A format value UnityPy cannot name: 3.
    UnnamedValue,
    /// An object version 1 cannot read at all, its class's tree missing: 3.
    NoTree,
    /// A defect of the object's own data: 3.
    Defect,
    /// The caller's spec makes a key that cannot be written or collides:
    /// 3.
    CallerKey,
}

impl ObjectSkip {
    /// Its class (9.3's table), by an exhaustive match, so a new reason
    /// is classed where it is added.
    #[must_use]
    pub fn class(&self) -> SkipClass {
        match self {
            Self::BudgetInput => SkipClass::Budget,
            Self::Limit(l) => match l.get().kind.class() {
                LimitClass::Budget | LimitClass::InputBudget => SkipClass::Budget,
                LimitClass::Data | LimitClass::ObjectData => SkipClass::Unreadable,
            },
            Self::UnsupportedFormat(FormatName::Named(_))
            | Self::UnsupportedPlatform(_)
            | Self::NguiLayoutUnsupported
            | Self::EmptyRect
            | Self::EmptyImage
            | Self::DuplicateName
            | Self::ReferenceAtlas => SkipClass::Expected,
            Self::OutsideView(_)
            | Self::BadPointer
            | Self::DecodeError
            | Self::DecoderPanic
            | Self::SheetNotFound => SkipClass::Unreadable,
            Self::UnsupportedFormat(FormatName::Value(_)) => SkipClass::UnnamedValue,
            Self::NoTypetree => SkipClass::NoTree,
            Self::UnnamedAtlas => SkipClass::Defect,
            Self::DuplicateKey | Self::UnwritableKey(_) | Self::JsonUnwritable => {
                SkipClass::CallerKey
            }
        }
    }

    /// Whether it is an expected skip (9.3), which sets no code.
    #[must_use]
    pub fn is_expected(&self) -> bool {
        self.class() == SkipClass::Expected
    }

    /// Its code at a normal end (9.2): none for an expected skip; 5 for a
    /// budget's (`budget(input)`, a limit of kind `input_memory`,
    /// `input_bytes` or `decode_work`); else 3.
    #[must_use]
    pub fn code(&self) -> Option<Code> {
        match self.class() {
            SkipClass::Expected => None,
            SkipClass::Budget => Some(Code::Budget),
            SkipClass::Unreadable
            | SkipClass::UnnamedValue
            | SkipClass::NoTree
            | SkipClass::Defect
            | SkipClass::CallerKey => Some(Code::CompleteWithSkips),
        }
    }
}

impl fmt::Display for ObjectSkip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BudgetInput => return Budget::Input.fmt(f),
            Self::Limit(l) => return l.get().fmt(f),
            Self::OutsideView(s) => return call(f, "outside_view", &[s]),
            Self::BadPointer => "bad_pointer",
            Self::DecodeError => "decode_error",
            Self::DecoderPanic => "decoder_panic",
            Self::UnsupportedFormat(s) => return call(f, "unsupported_format", &[s]),
            Self::UnsupportedPlatform(p) => return call(f, "unsupported_platform", &[p]),
            Self::EmptyImage => "empty_image",
            Self::DuplicateName => "duplicate_name",
            Self::DuplicateKey => "duplicate_key",
            Self::UnwritableKey(r) => return call(f, "unwritable_key", &[&r.name()]),
            Self::EmptyRect => "empty_rect",
            Self::UnnamedAtlas => "unnamed_atlas",
            Self::ReferenceAtlas => "reference_atlas",
            Self::SheetNotFound => "sheet_not_found",
            Self::NguiLayoutUnsupported => "ngui_layout_unsupported",
            Self::NoTypetree => "no_typetree",
            Self::JsonUnwritable => "json_unwritable",
        })
    }
}

/// An output's or an archive's status reason (9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutputReason {
    /// `withheld` (E61).
    Withheld,
    /// `unit_conflict`.
    UnitConflict,
    /// `unit_lost`.
    UnitLost,
    /// `publish_incomplete`.
    PublishIncomplete,
    /// `publish_failed`.
    PublishFailed,
    /// `publish_mismatch`.
    PublishMismatch,
    /// `credentials_expired`.
    CredentialsExpired,
    /// `archive_too_large`.
    ArchiveTooLarge,
    /// `unwritable_key(<rule>)`.
    UnwritableKey(KeyRule),
}

impl fmt::Display for OutputReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Withheld => "withheld",
            Self::UnitConflict => "unit_conflict",
            Self::UnitLost => "unit_lost",
            Self::PublishIncomplete => "publish_incomplete",
            Self::PublishFailed => "publish_failed",
            Self::PublishMismatch => "publish_mismatch",
            Self::CredentialsExpired => "credentials_expired",
            Self::ArchiveTooLarge => "archive_too_large",
            Self::UnwritableKey(r) => return call(f, "unwritable_key", &[&r.name()]),
        })
    }
}

/// Why an archive member was left out (9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeftOut {
    /// `duplicate_member`.
    DuplicateMember,
    /// `unwritable_path(<rule>)`.
    UnwritablePath(KeyRule),
    /// `prefix_conflict`.
    PrefixConflict,
}

impl fmt::Display for LeftOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateMember => f.write_str("duplicate_member"),
            Self::UnwritablePath(r) => call(f, "unwritable_path", &[&r.name()]),
            Self::PrefixConflict => f.write_str("prefix_conflict"),
        }
    }
}

/// A stop's reason (9.3) and its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StopReason {
    /// `deadline` (6).
    Deadline,
    /// `sigterm` (6).
    Sigterm,
    /// `sigint` (6).
    Sigint,
    /// `sighup` (6).
    Sighup,
    /// `parent_died` (6).
    ParentDied,
    /// `stdin_eof` (6).
    StdinEof,
    /// `spool_full` (5).
    SpoolFull,
    /// `host_io` (74).
    HostIo,
    /// `host_memory` (74).
    HostMemory,
    /// `internal` (70).
    Internal,
}

impl StopReason {
    /// Its code.
    #[must_use]
    pub fn code(self) -> Code {
        match self {
            Self::Deadline
            | Self::Sigterm
            | Self::Sigint
            | Self::Sighup
            | Self::ParentDied
            | Self::StdinEof => Code::Stopped,
            Self::SpoolFull => Code::Budget,
            Self::HostIo | Self::HostMemory => Code::Host,
            Self::Internal => Code::Internal,
        }
    }
}

impl StopReason {
    /// Its name in a head's `stop_reason` and an `exit` line's `reason`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Deadline => "deadline",
            Self::Sigterm => "sigterm",
            Self::Sigint => "sigint",
            Self::Sighup => "sighup",
            Self::ParentDied => "parent_died",
            Self::StdinEof => "stdin_eof",
            Self::SpoolFull => "spool_full",
            Self::HostIo => "host_io",
            Self::HostMemory => "host_memory",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

macro_rules! names {
    ($(#[$doc:meta])* $ty:ident { $($v:ident => $s:literal,)* }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $ty {
            $(#[doc = concat!("`", $s, "`.")] $v,)*
        }

        impl $ty {
            /// Every name, in 9.3's order.
            pub const ALL: &[Self] = &[$(Self::$v,)*];

            /// Its name.
            #[must_use]
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$v => $s,)*
                }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

names! {
    /// A preflight reason (exit 69, 9.3). Its detail, where 9.3 names one,
    /// is the caller's.
    Preflight {
        SpoolNotDedicated => "spool_not_dedicated",
        SpoolInMemory => "spool_in_memory",
        SpoolOverlapsSink => "spool_overlaps_sink",
        SpoolLocked => "spool_locked",
        SpoolNoSpace => "spool_no_space",
        SpoolUnusable => "spool_unusable",
        SinkRootUnusable => "sink_root_unusable",
        OutputLocked => "output_locked",
        ManifestUnreadable => "manifest_unreadable",
        BucketMissing => "bucket_missing",
        SinkUnreachable => "sink_unreachable",
        PreviousAttemptLive => "previous_attempt_live",
        CredentialsMissing => "credentials_missing",
        CredentialsInvalid => "credentials_invalid",
        HeaderEnvMissing => "header_env_missing",
        HeaderEnvTooLong => "header_env_too_long",
        HeaderEnvInvalid => "header_env_invalid",
        CredentialsExpire => "credentials_expire",
        DeadlineTooClose => "deadline_too_close",
        RlimitTooLow => "rlimit_too_low",
        OvercommitStrict => "overcommit_strict",
        MapCountTooLow => "map_count_too_low",
        MemoryTooSmall => "memory_too_small",
        CgroupUnreadable => "cgroup_unreadable",
        ProcfsUnreadable => "procfs_unreadable",
        StartupFailed => "startup_failed",
        ThreadSpawnFailed => "thread_spawn_failed",
        KernelTooOld => "kernel_too_old",
        MountCrossing => "mount_crossing",
        MountCheckUnavailable => "mount_check_unavailable",
    }
}

names! {
    /// A warning in the head's `warnings` (9.3).
    Warning {
        ResumeListFailed => "resume_list_failed",
        VerifyListFailed => "verify_list_failed",
        EtagNotMd5 => "etag_not_md5",
        StderrDropped => "stderr_dropped",
        AbortFailed => "abort_failed",
        StdinWatchEnded => "stdin_watch_ended",
    }
}

/// An outcome at a normal end (9.2's rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome<'a> {
    /// An object skipped.
    ObjectSkipped(&'a ObjectSkip),
    /// An input failed.
    InputFailed(&'a InputReason),
    /// A bundle failed, whether or not an input's view holds it.
    BundleFailed(&'a InputReason),
    /// An output's, archive's, row file's or the head's status.
    Output(&'a OutputReason),
    /// An archive member left out.
    MemberLeftOut(&'a LeftOut),
    /// An output or archive `not_written`.
    NotWritten,
    /// An archive `empty`.
    ArchiveEmpty,
}

/// The code one outcome sets at a normal end (9.2), `None` for one that
/// sets none of its own.
#[must_use]
pub fn outcome_code(outcome: Outcome<'_>) -> Option<Code> {
    match outcome {
        Outcome::ObjectSkipped(s) => s.code(),
        Outcome::InputFailed(r) | Outcome::BundleFailed(r) => Some(r.code()),
        Outcome::Output(r) => match r {
            OutputReason::PublishFailed
            | OutputReason::PublishMismatch
            | OutputReason::CredentialsExpired
            | OutputReason::ArchiveTooLarge => Some(Code::Incomplete),
            OutputReason::UnitConflict | OutputReason::UnwritableKey(_) => {
                Some(Code::CompleteWithSkips)
            }
            // A key withheld: the failed input sets the code. A unit lost
            // to a winner is no failure.
            OutputReason::Withheld | OutputReason::PublishIncomplete | OutputReason::UnitLost => {
                None
            }
        },
        Outcome::MemberLeftOut(_) => Some(Code::CompleteWithSkips),
        Outcome::NotWritten => Some(Code::Incomplete),
        Outcome::ArchiveEmpty => None,
    }
}

/// A normal end's code: the most severe its outcomes set, 0 when none
/// sets one.
pub fn normal_end<'a>(outcomes: impl IntoIterator<Item = Outcome<'a>>) -> Code {
    outcomes
        .into_iter()
        .filter_map(outcome_code)
        .fold(Code::Complete, Code::worst)
}
