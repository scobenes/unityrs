//! Resource lookup (`helpers/ResourceReader.py:10-38`).
//!
//! `get_resource_data(res_path, ...)` tries four CAB names, in order:
//! `basename`, `name + ".resource"`, `name + ".assets.resS"`,
//! `name + ".resS"`, where `basename = ntpath.basename(res_path)` and `name`
//! is `ntpath.splitext(basename)[0]`. On a miss it loads the file's externals
//! and the four candidates as dependencies, then tries again, and raises
//! `FileNotFoundError` when all four still miss. The bytes are
//! `reader.read_bytes(size)` from `offset`.
//!
//! **Memory.** `res_path` is an object's string (`m_StreamData.path`,
//! `m_Resource.m_Source`), a slice of its file as long as the object's
//! bytes allow. [`resource_candidates`] copies nothing: each [`Candidate`]
//! is a slice of the path and a static suffix. A lookup that needs the whole
//! name (the registry's lowered key, a resolver's argument) builds it with
//! [`Candidate::joined`], charged to the asking file's meter while it lives,
//! one candidate at a time.
//!
//! Owner: package D ([`Candidate`] and its methods: the scaffold).

// STUB(D): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables
)]

use std::sync::Arc;

use crate::limits::{ChargeError, Meter};
use crate::raw::RawStr;

/// The suffixes of the four candidates, in UnityPy's order.
pub const SUFFIXES: [&[u8]; 4] = [b"", b".resource", b".assets.resS", b".resS"];

/// One candidate name: a slice of the resource path followed by a suffix,
/// built only when a lookup needs it whole ([`Self::joined`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// A slice of the resource path (no copy): its `basename` for the first
    /// candidate, that basename's `splitext` root for the others.
    pub stem: RawStr,
    /// One of [`SUFFIXES`].
    pub suffix: &'static [u8],
}

impl Candidate {
    /// The name's length in bytes.
    pub fn len(&self) -> usize {
        self.stem.len() + self.suffix.len()
    }

    /// Whether the name is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole name: the stem itself when there is no suffix (no copy),
    /// else the stem and the suffix in one buffer charged to `meter` first
    /// ([`RawStr::charged_concat`]), which the name carries until its last
    /// clone drops. A refusal allocates nothing.
    pub fn joined(&self, meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
        if self.suffix.is_empty() {
            return Ok(self.stem.clone());
        }
        RawStr::charged_concat(&[self.stem.as_bytes(), self.suffix], meter)
    }
}

const _: () = crate::send_sync::<Candidate>();

/// The four candidates, in UnityPy's order, each a slice of `res_path` and a
/// suffix (module docs): nothing is copied.
pub fn resource_candidates(res_path: &RawStr) -> [Candidate; 4] {
    todo!()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Candidate, resource_candidates};
    use crate::budget::{OWNER_COST, Reservation, Unbounded};
    use crate::limits::{Meter, heap_cost};
    use crate::raw::RawStr;

    #[test]
    fn candidates_are_slices_of_the_path_built_whole_only_when_charged() {
        // Round 4: `resource_candidates(&[u8]) -> [RawStr; 4]` could only
        // build four uncharged copies of a basename as long as the object's
        // bytes, on every resource read.
        let _: fn(&RawStr) -> [Candidate; 4] = resource_candidates;
        let path = RawStr::from_shared(bytes::Bytes::from(b"archive:/a/tex.png".to_vec()));
        let m = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let first = Candidate {
            stem: RawStr::from_shared(path.clone().into_bytes().slice(11..)),
            suffix: b"",
        };
        let whole = first.joined(&m).unwrap();
        assert_eq!(whole.as_bytes(), b"tex.png");
        assert_eq!(whole.as_bytes().as_ptr(), first.stem.as_bytes().as_ptr());
        assert_eq!(m.used(), 0, "the basename is the path's own bytes");
        let res = Candidate {
            stem: RawStr::from_shared(path.into_bytes().slice(11..14)),
            suffix: b".assets.resS",
        };
        assert_eq!(res.len(), 15);
        let name = res.joined(&m).unwrap();
        assert_eq!(name.as_bytes(), b"tex.assets.resS");
        assert_eq!(m.used(), heap_cost(15) + OWNER_COST);
        drop(name);
        assert_eq!(m.used(), 0);
        let full = Meter::new(OWNER_COST, Arc::new(Unbounded), Reservation::Input);
        assert!(res.joined(&full).is_err());
        assert_eq!(full.used(), 0);
    }
}
