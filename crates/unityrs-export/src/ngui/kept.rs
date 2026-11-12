//! A skip's reason kept by a cache for the next object that asks the same
//! question (a texture's read and answers, a name, an atlas's read), and
//! made into a [`Reason`] only when a skip row is recorded, whose own
//! strings are charged then (B3's review round 1, R7): so a cache of many
//! entries holds no string `M_X` does not pay for.
//!
//! A reason of the closed forms 7.3 and 9.3 give from words of the cutter's
//! own ([`Code`]) is kept as those words, allocating nothing; any other (an
//! argument from the bytes, a format's number) is kept with its strings'
//! charge.

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter};

use super::classify::{Class, Reason};
use super::seam::{Hooks, probe};
use super::table;

/// The plain reasons kept as words.
const PLAIN: &[&str] = &[
    "bad_pointer",
    "budget(input)",
    "empty_image",
    "unsupported_platform(switch)",
    "unsupported_platform(xbox360)",
];

/// The `decode_error` details kept as words: 7.3's codes.
const DETAILS: &[&str] = &[
    "asset_bundle",
    "crop_overflow",
    "decompress",
    "decompression_bomb",
    "detect",
    "malformed",
    "negative_size",
    "no_image_data",
    "not_enough_data",
    "not_found",
    "out_of_bounds",
    "pillow_memory_error",
    "read",
    "render",
    "tree",
    "type_index",
    "unsupported",
    "version",
];

/// The limits' kinds kept as words (9.3's table).
const LIMITS: &[&str] = &[
    "block_output",
    "container_depth",
    "count",
    "decode_work",
    "inflate",
    "input_bytes",
    "input_memory",
    "texture_pixels",
    "tree_depth",
    "value_depth",
    "values_per_object",
    "zero_size_elements",
];

/// A reason of a closed form: no allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Code {
    /// A reason with no detail.
    Plain(&'static str),
    /// `decode_error`, with its detail.
    Decode(&'static str),
    /// `limit(<kind>, <max>)`.
    Limit(&'static str, u64),
}

fn word(words: &'static [&'static str], s: &str) -> Option<&'static str> {
    words.iter().copied().find(|w| *w == s)
}

impl Code {
    /// `r` as words, where it is of a closed form.
    fn of(r: &Reason) -> Option<Self> {
        if r.cut {
            return None;
        }
        match (r.reason.as_str(), r.detail.as_deref()) {
            ("decode_error", Some(d)) => word(DETAILS, d).map(Self::Decode),
            (reason, None) => {
                if let Some(w) = word(PLAIN, reason) {
                    return Some(Self::Plain(w));
                }
                let (kind, max) = reason
                    .strip_prefix("limit(")?
                    .strip_suffix(')')?
                    .split_once(", ")?;
                // The decimal as `format!` writes it: digits, no leading 0.
                let canonical = max.bytes().all(|b| b.is_ascii_digit())
                    && (max == "0" || !max.starts_with('0'));
                let max: u64 = max.parse().ok().filter(|_| canonical)?;
                Some(Self::Limit(word(LIMITS, kind)?, max))
            }
            _ => None,
        }
    }

    fn reason(self) -> Reason {
        match self {
            Self::Plain(r) => Reason::plain(r),
            Self::Decode(d) => Reason::decode_error(d),
            Self::Limit(kind, max) => Reason::plain(format!("limit({kind}, {max})")),
        }
    }
}

/// An object's skip, kept: as words, or with its strings' charge.
#[derive(Debug)]
pub(super) enum Kept {
    /// Words.
    Code(Code),
    /// Any other reason, its strings charged to `M_X`.
    Charged {
        /// The reason.
        reason: Reason,
        /// Its strings' charge.
        _charge: Charge,
    },
}

impl Kept {
    /// `class` kept: an object's skip as words, or with its strings'
    /// charge. Nothing is kept of a refusal of `M_X`'s ([`Self::metered`]),
    /// of a refused charge for the strings (B3's review round 2, S1: the
    /// refusal is the asking object's skip, a state of the meter, and the
    /// next object asks again), or of the input's or the run's failure:
    /// each is returned, its class, for the caller to make the current
    /// object's. The charge is under the refusal probe (2.7), with
    /// `hooks`; nothing else is, so a class handed in is returned as it is.
    pub fn keep(meter: &Arc<Meter>, hooks: &dyn Hooks, class: Class) -> Result<Self, Class> {
        if Self::metered(&class) {
            return Err(class);
        }
        let Class::Skip(reason) = class else {
            return Err(class);
        };
        if let Some(c) = Code::of(&reason) {
            return Ok(Self::Code(c));
        }
        let detail = reason.detail.as_ref().map_or(0, String::len);
        let cost = table::string(reason.reason.len()).saturating_add(table::string(detail));
        let charge = probe(hooks, || meter.charge(cost).map_err(|e| table::refused(&e)))?;
        Ok(Self::Charged {
            reason,
            _charge: charge,
        })
    }

    /// Whether `class` is a refusal of `M_X`'s, a state of the meter and
    /// not of the bytes (`budget(input)`, `limit(input_memory, ..)`,
    /// `limit(decode_work, ..)`): a cache that keeps it would hand one
    /// object's refusal to the next.
    #[must_use]
    pub fn metered(class: &Class) -> bool {
        let Class::Skip(r) = class else {
            return false;
        };
        matches!(
            Code::of(r),
            Some(Code::Plain("budget(input)") | Code::Limit("input_memory" | "decode_work", _))
        )
    }

    /// The skip, its reason made now.
    #[must_use]
    pub fn class(&self) -> Class {
        Class::Skip(match self {
            Self::Code(c) => c.reason(),
            Self::Charged { reason, .. } => reason.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_core::limits::Meter;

    use super::{Code, Kept};
    use crate::ngui::NoHooks;
    use crate::ngui::classify::{Class, Reason};

    #[test]
    fn closed_reasons_are_words_and_others_are_charged() {
        let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
        let words = [
            Reason::plain("bad_pointer"),
            Reason::plain("empty_image"),
            Reason::decode_error("render"),
            Reason::decode_error("asset_bundle"),
            Reason::plain("limit(texture_pixels, 268435456)"),
        ];
        for r in words {
            let k = Kept::keep(&meter, &NoHooks, Class::Skip(r.clone())).unwrap();
            assert!(matches!(k, Kept::Code(_)), "{r:?}");
            assert_eq!(k.class(), Class::Skip(r));
            assert_eq!(meter.used(), 0);
        }
        let others = [
            Reason::plain("outside_view(CAB-x)"),
            Reason::plain("unsupported_format(Alpha8)"),
            Reason::plain("limit(decode_work, 01)"),
            Reason::plain("limit(nothing, 1)"),
            Reason::decode_error("something"),
            Reason {
                reason: "decode_error".to_owned(),
                detail: Some("read".to_owned()),
                cut: true,
            },
        ];
        for r in others {
            assert_eq!(Code::of(&r), None, "{r:?}");
            let k = Kept::keep(&meter, &NoHooks, Class::Skip(r.clone())).unwrap();
            assert!(meter.used() > 0);
            assert_eq!(k.class(), Class::Skip(r));
            drop(k);
            assert_eq!(meter.used(), 0);
        }
        let input = Class::Input(Reason::plain("budget(dependency)"));
        assert_eq!(
            Kept::keep(&meter, &NoHooks, input.clone()).unwrap_err(),
            input
        );
    }

    #[test]
    fn a_refused_charge_is_returned_and_never_kept() {
        let meter = Meter::new(8, Arc::new(Unbounded), Reservation::Input);
        let refusal = Class::Skip(Reason::plain("limit(input_memory, 8)"));
        let long = || Class::Skip(Reason::plain("outside_view(CAB-a-long-name)"));
        assert_eq!(Kept::keep(&meter, &NoHooks, long()).unwrap_err(), refusal);
        // A refusal of M_X's is never kept, asked for or the charge's own.
        assert_eq!(
            Kept::keep(&meter, &NoHooks, refusal.clone()).unwrap_err(),
            refusal
        );
        for r in ["budget(input)", "limit(decode_work, 1000)"] {
            let c = Class::Skip(Reason::plain(r));
            assert_eq!(Kept::keep(&meter, &NoHooks, c.clone()).unwrap_err(), c);
            assert!(Kept::metered(&c));
        }
        assert!(Kept::metered(&refusal));
        assert_eq!(meter.used(), 0);
    }
}
