//! Exit codes and reasons (`docs/BATCH_SPEC.md` 9.1 to 9.3).
//!
//! - [`Code`]: 9.1's codes, their statuses and their precedence.
//! - The closed reason sets of 9.3, one type per place a reason appears:
//!   [`InputReason`] (a bundle or an input), [`ObjectSkip`],
//!   [`OutputReason`] (an output's or archive's status), [`LeftOut`] (a
//!   member left out), [`StopReason`], [`Preflight`] and [`Warning`];
//!   `stopped` (an input `not_run`), `superseded` (a head read) and
//!   `bad_spec` (validation) are single names ([`STOPPED`], [`SUPERSEDED`],
//!   [`BAD_SPEC`]). Each renders as `name` or `name(argument, ...)`.
//! - [`LimitKind`]: 9.3's limit-kind table, its classes, and the core's
//!   kinds mapped through it.
//! - [`outcome_code`] and [`normal_end`]: 9.2's outcome-to-code map.

use std::fmt;

/// An exit code (9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    /// 0: `complete`.
    Complete,
    /// 3: `complete_with_skips`.
    CompleteWithSkips,
    /// 4: `incomplete`, a data or transport reason, or a publish failure.
    Incomplete,
    /// 5: `incomplete`, a budget decided an outcome.
    Budget,
    /// 6: stopped.
    Stopped,
    /// 7: superseded.
    Superseded,
    /// 69: could not start.
    CouldNotStart,
    /// 70: internal error.
    Internal,
    /// 74: host failure.
    Host,
    /// 78: a bad spec or command line.
    BadSpec,
}

impl Code {
    /// Every code, most severe first: 9.1's precedence.
    pub const PRECEDENCE: [Self; 10] = [
        Self::BadSpec,
        Self::CouldNotStart,
        Self::Internal,
        Self::Host,
        Self::Superseded,
        Self::Stopped,
        Self::Budget,
        Self::Incomplete,
        Self::CompleteWithSkips,
        Self::Complete,
    ];

    /// The process's exit status.
    #[must_use]
    pub fn value(self) -> u8 {
        match self {
            Self::Complete => 0,
            Self::CompleteWithSkips => 3,
            Self::Incomplete => 4,
            Self::Budget => 5,
            Self::Stopped => 6,
            Self::Superseded => 7,
            Self::CouldNotStart => 69,
            Self::Internal => 70,
            Self::Host => 74,
            Self::BadSpec => 78,
        }
    }

    /// The code whose status is `value`.
    #[must_use]
    pub fn from_value(value: u8) -> Option<Self> {
        Self::PRECEDENCE.into_iter().find(|c| c.value() == value)
    }

    /// Its place in [`Self::PRECEDENCE`]: lower is more severe.
    #[must_use]
    pub fn severity(self) -> usize {
        Self::PRECEDENCE
            .iter()
            .position(|&c| c == self)
            .unwrap_or(Self::PRECEDENCE.len())
    }

    /// The more severe of two codes, when several apply.
    #[must_use]
    pub fn worst(self, other: Self) -> Self {
        if other.severity() < self.severity() {
            other
        } else {
            self
        }
    }

    /// The head's `status` the finisher writes for this code (9.1), `None`
    /// where it writes none. A stop's or a run failure's head may also be
    /// none (6, 70 and 74 write `incomplete` when they can).
    #[must_use]
    pub fn status(self) -> Option<&'static str> {
        match self {
            Self::Complete => Some("complete"),
            Self::CompleteWithSkips => Some("complete_with_skips"),
            Self::Incomplete | Self::Budget | Self::Stopped | Self::Internal | Self::Host => {
                Some("incomplete")
            }
            Self::Superseded | Self::CouldNotStart | Self::BadSpec => None,
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value())
    }
}

/// A limit's class (9.3's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitClass {
    /// A budget (exit 5): an object's skip or the input's failure.
    Budget,
    /// A budget (exit 5), always the input's failure.
    InputBudget,
    /// Data (exit 3 or 4).
    Data,
    /// Data (exit 3), always an object's skip.
    ObjectData,
}

/// A limit's kind: `limit(<kind>, <value>)`'s first argument (9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitKind {
    /// The core's `InputMemory`.
    InputMemory,
    /// The core's `InputBytes`.
    InputBytes,
    /// The core's `DecodeWork`.
    DecodeWork,
    /// The engine's (`batch.md` 2.6 step 7).
    AttemptEvictions,
    /// The engine's (`batch.md` 2.6 step 10).
    ExportWork,
    /// The core's `BlockOutput`.
    BlockOutput,
    /// The core's `Inflate`.
    Inflate,
    /// The core's `ContainerDepth`.
    ContainerDepth,
    /// The core's `TreeDepth`.
    TreeDepth,
    /// The core's `ValueDepth`.
    ValueDepth,
    /// The core's `ValuesPerObject`.
    ValuesPerObject,
    /// The core's `ZeroSizeElements`.
    ZeroSizeElements,
    /// The core's `Count`.
    Count,
    /// `unityrs_texture::MAX_PIXELS`.
    TexturePixels,
}

impl LimitKind {
    /// Every kind, in 9.3's table's order.
    pub const ALL: [Self; 14] = [
        Self::InputMemory,
        Self::InputBytes,
        Self::DecodeWork,
        Self::AttemptEvictions,
        Self::ExportWork,
        Self::BlockOutput,
        Self::Inflate,
        Self::ContainerDepth,
        Self::TreeDepth,
        Self::ValueDepth,
        Self::ValuesPerObject,
        Self::ZeroSizeElements,
        Self::Count,
        Self::TexturePixels,
    ];

    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::InputMemory => "input_memory",
            Self::InputBytes => "input_bytes",
            Self::DecodeWork => "decode_work",
            Self::AttemptEvictions => "attempt_evictions",
            Self::ExportWork => "export_work",
            Self::BlockOutput => "block_output",
            Self::Inflate => "inflate",
            Self::ContainerDepth => "container_depth",
            Self::TreeDepth => "tree_depth",
            Self::ValueDepth => "value_depth",
            Self::ValuesPerObject => "values_per_object",
            Self::ZeroSizeElements => "zero_size_elements",
            Self::Count => "count",
            Self::TexturePixels => "texture_pixels",
        }
    }

    /// Its class.
    #[must_use]
    pub fn class(self) -> LimitClass {
        match self {
            Self::InputMemory | Self::InputBytes | Self::DecodeWork => LimitClass::Budget,
            Self::AttemptEvictions | Self::ExportWork => LimitClass::InputBudget,
            Self::TexturePixels => LimitClass::ObjectData,
            _ => LimitClass::Data,
        }
    }

    /// Whether it is a budget (exit 5).
    #[must_use]
    pub fn is_budget(self) -> bool {
        matches!(self.class(), LimitClass::Budget | LimitClass::InputBudget)
    }

    /// The core's kind through the table; `None` for a kind the table
    /// lacks (a newer core's), which `classify` makes the run failure
    /// `internal` (`batch.md` 2.7).
    #[must_use]
    pub fn from_core(kind: unityrs_core::LimitKind) -> Option<Self> {
        use unityrs_core::LimitKind as K;
        Some(match kind {
            K::InputMemory => Self::InputMemory,
            K::InputBytes => Self::InputBytes,
            K::DecodeWork => Self::DecodeWork,
            K::BlockOutput => Self::BlockOutput,
            K::Inflate => Self::Inflate,
            K::ContainerDepth => Self::ContainerDepth,
            K::TreeDepth => Self::TreeDepth,
            K::ValueDepth => Self::ValueDepth,
            K::ValuesPerObject => Self::ValuesPerObject,
            K::ZeroSizeElements => Self::ZeroSizeElements,
            K::Count => Self::Count,
            _ => return None,
        })
    }
}

/// `limit(<kind>, <value>)`: the kind and its configured value, never the
/// amount asked for (9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Limit {
    /// The kind.
    pub kind: LimitKind,
    /// The configured (or fixed) bound.
    pub value: u64,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "limit({}, {})", self.kind.name(), self.value)
    }
}

/// A `limit` a bundle or an input may fail by: any kind but
/// `texture_pixels`, which is always an object's skip (9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InputLimit(Limit);

impl InputLimit {
    /// `limit`, or `None` for a kind 9.3 keeps from an input.
    #[must_use]
    pub fn new(limit: Limit) -> Option<Self> {
        (limit.kind.class() != LimitClass::ObjectData).then_some(Self(limit))
    }

    /// The limit.
    #[must_use]
    pub fn get(self) -> Limit {
        self.0
    }
}

/// A `limit` an object may be skipped by: any kind but
/// `attempt_evictions` and `export_work`, which always fail the input
/// (9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectLimit(Limit);

impl ObjectLimit {
    /// `limit`, or `None` for a kind 9.3 keeps from an object's skip.
    #[must_use]
    pub fn new(limit: Limit) -> Option<Self> {
        (limit.kind.class() != LimitClass::InputBudget).then_some(Self(limit))
    }

    /// The limit.
    #[must_use]
    pub fn get(self) -> Limit {
        self.0
    }
}

/// `budget(<which>)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Budget {
    /// `budget(input)`.
    Input,
    /// `budget(dependency)`.
    Dependency,
}

impl fmt::Display for Budget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Input => "budget(input)",
            Self::Dependency => "budget(dependency)",
        })
    }
}

/// A key rule of 2.3: `unwritable_key(<rule>)`'s and
/// `unwritable_path(<rule>)`'s argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyRule {
    /// `empty`.
    Empty,
    /// `not_utf8`.
    NotUtf8,
    /// `control_char`.
    ControlChar,
    /// `leading_slash`.
    LeadingSlash,
    /// `trailing_slash`.
    TrailingSlash,
    /// `empty_segment`.
    EmptySegment,
    /// `dot_segment`.
    DotSegment,
    /// `segment_too_long`.
    SegmentTooLong,
    /// `too_long`.
    TooLong,
    /// `reserved_prefix`.
    ReservedPrefix,
    /// `reserved_name`.
    ReservedName,
    /// `prefix_conflict`.
    PrefixConflict,
    /// `archive_conflict`.
    ArchiveConflict,
}

impl KeyRule {
    /// Every rule, in 2.3's order.
    pub const ALL: [Self; 13] = [
        Self::Empty,
        Self::NotUtf8,
        Self::ControlChar,
        Self::LeadingSlash,
        Self::TrailingSlash,
        Self::EmptySegment,
        Self::DotSegment,
        Self::SegmentTooLong,
        Self::TooLong,
        Self::ReservedPrefix,
        Self::ReservedName,
        Self::PrefixConflict,
        Self::ArchiveConflict,
    ];

    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NotUtf8 => "not_utf8",
            Self::ControlChar => "control_char",
            Self::LeadingSlash => "leading_slash",
            Self::TrailingSlash => "trailing_slash",
            Self::EmptySegment => "empty_segment",
            Self::DotSegment => "dot_segment",
            Self::SegmentTooLong => "segment_too_long",
            Self::TooLong => "too_long",
            Self::ReservedPrefix => "reserved_prefix",
            Self::ReservedName => "reserved_name",
            Self::PrefixConflict => "prefix_conflict",
            Self::ArchiveConflict => "archive_conflict",
        }
    }
}

mod sets;

pub use self::sets::{
    BAD_SPEC, InputReason, LeftOut, ObjectSkip, Outcome, OutputReason, Platform, Preflight,
    STOPPED, SUPERSEDED, StopReason, Warning, normal_end, outcome_code,
};

#[cfg(test)]
mod tests;
