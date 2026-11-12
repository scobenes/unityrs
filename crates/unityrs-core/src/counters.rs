//! The core's side of CR8's test-only counters (`docs/plans/batch.md`
//! section 6, CR8; `crate::testing` under the `test-counters` feature):
//! the refusal counts, the fallible helper's mark, the load ids and the
//! span reads, each called where the core makes the event it counts, and
//! the identity of each vector-growth invocation across its retries.
//!
//! Without the feature every item here is a unit struct with no `Drop`, or
//! an empty function, so a field of one takes no room, a guard of one does
//! nothing when it drops, and each call compiles to nothing: the core is
//! built as it was before the counters (plan section 4.1, the property
//! *CR8's counters cost a build without the feature nothing*; the sizes
//! are asserted below).

#[cfg(feature = "test-counters")]
pub(crate) use crate::testing::{
    Fallible, Growing, LoadId, Origin, SpanRow, allocator, budget, limit, span_read,
};

#[cfg(not(feature = "test-counters"))]
pub(crate) use off::{
    Fallible, Growing, LoadId, Origin, SpanRow, allocator, budget, limit, span_read,
};

#[cfg(not(feature = "test-counters"))]
#[allow(clippy::unused_self, clippy::needless_pass_by_value)]
mod off {
    use std::mem::size_of;

    use crate::budget::Reservation;
    use crate::error::LimitKind;
    use crate::raw::RawStr;

    /// A refusal of `kind`'s bound: not counted.
    #[inline]
    pub(crate) fn limit(_: LimitKind) {}

    /// A budget's refusal: not counted.
    #[inline]
    pub(crate) fn budget(_: Reservation) {}

    /// The allocator's refusal: not counted.
    #[inline]
    pub(crate) fn allocator(_: Reservation) {}

    /// No mark.
    pub(crate) struct Fallible;

    impl Fallible {
        #[inline]
        pub(crate) fn enter() -> Self {
            Self
        }
    }

    /// No growth invocation mark.
    pub(crate) struct Growing;

    impl Growing {
        #[inline]
        pub(crate) fn enter() -> Self {
            Self
        }
    }

    /// No load id.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct LoadId;

    impl LoadId {
        /// No load's: a reopen's, which keeps no id without the feature,
        /// and a test's open outside a load.
        pub(crate) const BARE: Self = Self;

        #[inline]
        pub(crate) fn draw(_: &RawStr) -> Self {
            Self
        }
    }

    /// No key.
    #[derive(Debug)]
    pub(crate) struct Origin;

    impl Origin {
        #[inline]
        pub(crate) fn opened() -> Self {
            Self
        }
    }

    /// No row.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct SpanRow;

    impl SpanRow {
        pub(crate) const WHOLE: Self = Self;

        #[inline]
        pub(crate) const fn row(_: usize) -> Self {
            Self
        }
    }

    /// A span read: not counted.
    #[inline]
    pub(crate) fn span_read(_: &Origin, _: SpanRow) {}

    // Nothing the core keeps or passes takes room.
    const _: () = assert!(
        size_of::<Fallible>() == 0
            && size_of::<Growing>() == 0
            && size_of::<LoadId>() == 0
            && size_of::<Origin>() == 0
            && size_of::<SpanRow>() == 0
    );
}

/// The source scan that holds every refusal the core makes to a counted
/// site; its own file, which the scan skips, holds the fixtures that show
/// each form is caught.
#[cfg(test)]
#[path = "counters_scan_tests.rs"]
mod scan_tests;
