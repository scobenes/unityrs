use std::fmt;
use unityrs_core::env::DerefError;
use unityrs_core::limits::ChargeError;
use unityrs_core::objmode::ViewError;
use unityrs_core::walk::ParseError;

/// A root failure, malformed field, or a work/memory refusal.
#[derive(Debug)]
pub enum SceneError {
    /// Fallible allocation or the caller's memory/work bound.
    Charge(ChargeError),
    /// A typetree could not be read.
    Parse(ParseError),
    /// A required pointer could not be resolved.
    Deref(DerefError),
    /// An object-mode attribute failure; malformed views are Tier U.
    View(ViewError),
    /// A dict field has the wrong shape, type or range.
    Malformed(&'static str),
    /// An object is not a `GameObject`, Transform or renderer as requested.
    WrongClass(i32),
    /// A `GameObject` has no Transform component.
    NoTransform,
}

#[cfg(test)]
mod tests;
const _: () = crate::send_sync::<SceneError>();

impl From<ChargeError> for SceneError {
    fn from(e: ChargeError) -> Self {
        Self::Charge(e)
    }
}
impl From<ParseError> for SceneError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}
impl From<DerefError> for SceneError {
    fn from(e: DerefError) -> Self {
        Self::Deref(e)
    }
}
impl From<ViewError> for SceneError {
    fn from(e: ViewError) -> Self {
        Self::View(e)
    }
}
impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Charge(e) => e.fmt(f),
            Self::Parse(e) => e.fmt(f),
            Self::Deref(e) => e.fmt(f),
            Self::View(e) => e.fmt(f),
            Self::Malformed(field) => write!(f, "malformed scene field: {field}"),
            Self::WrongClass(id) => write!(f, "unexpected scene class: {id}"),
            Self::NoTransform => f.write_str("GameObject has no Transform"),
        }
    }
}
impl std::error::Error for SceneError {}

impl SceneError {
    #[allow(
        clippy::match_same_arms,
        reason = "Keep audited refusals explicit beside the conservative non-exhaustive fallback"
    )]
    pub(crate) fn resource_refusal(&self) -> bool {
        use unityrs_core::detect::DetectError;
        use unityrs_core::typetree::TreeError;
        use unityrs_core::walk::{ParseError, ReadDetail};
        // Core error enums are non_exhaustive. Only audited data failures
        // may become issues; an unknown variant must remain fatal until
        // its resource policy is reviewed here and in the variant tests.
        fn tree(e: &TreeError) -> bool {
            match e {
                TreeError::Charge(_) | TreeError::Limit(_) | TreeError::TooManyNodes(_) => true,
                TreeError::Empty | TreeError::LevelUnderflow { .. } => false,
                _ => true,
            }
        }
        fn parse(e: &ParseError) -> bool {
            match e {
                ParseError::Read(r) => match &r.detail {
                    ReadDetail::Charge(_) | ReadDetail::Limit(_) => true,
                    ReadDetail::Tree(t) => tree(t),
                    ReadDetail::Eof { .. }
                    | ReadDetail::NegativeLength(_)
                    | ReadDetail::ArrayChildren
                    | ReadDetail::PairChildren
                    | ReadDetail::NoRefTypes
                    | ReadDetail::RefTypeNotFound
                    | ReadDetail::RefTypeNoTree
                    | ReadDetail::Python(_)
                    | ReadDetail::Stream(_) => false,
                    _ => true,
                },
                ParseError::Tree(t) => tree(t),
                ParseError::Deref(d) => deref(d),
                ParseError::Provider(p) => {
                    p.limit.is_some() || p.memory.is_some() || p.allocator.is_some()
                }
                ParseError::NoTypeTree { .. } | ParseError::CheckRead { .. } => false,
                _ => true,
            }
        }
        fn core(e: &unityrs_core::Error) -> bool {
            use unityrs_core::Error;
            match e {
                Error::Budget(_) | Error::Allocator(_) | Error::Limit(_) => true,
                Error::Detect(d) => match d {
                    DetectError::InflateLimit(_)
                    | DetectError::Work(_)
                    | DetectError::Allocator(_) => true,
                    DetectError::Unterminated | DetectError::Gzip(_) => false,
                    _ => true,
                },
                Error::Tree(t) => tree(t),
                Error::AssetBundle(p) => parse(p),
                Error::Entry { source, .. } => core(source),
                Error::Deref(d) => deref(d),
                Error::OutOfBounds { .. }
                | Error::Malformed { .. }
                | Error::Unsupported(_)
                | Error::Decompress { .. }
                | Error::Version(_)
                | Error::TypeIndex { .. }
                | Error::NotFound(_)
                | Error::Io(_) => false,
                _ => true,
            }
        }
        fn deref(e: &DerefError) -> bool {
            match e {
                DerefError::Load(e) => core(e),
                DerefError::Null
                | DerefError::FileIdPastEnd { .. }
                | DerefError::FileIdBelowStart { .. }
                | DerefError::NotFound { .. }
                | DerefError::RawTarget { .. }
                | DerefError::MissingObject { .. }
                | DerefError::ContainerDropped { .. } => false,
                _ => true,
            }
        }
        match self {
            Self::Charge(_) => true,
            Self::Parse(e) => parse(e),
            Self::Deref(e) => deref(e),
            Self::View(ViewError::Charge(_)) => true,
            Self::View(
                ViewError::Malformed(_) | ViewError::NoAttribute(_) | ViewError::ByteRange,
            ) => false,
            Self::Malformed(_) | Self::WrongClass(_) | Self::NoTransform => false,
        }
    }
}
