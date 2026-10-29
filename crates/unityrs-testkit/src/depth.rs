//! Deep values (plan section 3.10, "Bounds"): the shapes of
//! `tools/fixtures/walk.py`'s `depth.json`, built the same way.
//!
//! Ref type `A` nests one kind of level as deep as one tree may go around a
//! `ReferencedObject` whose data is the next `A`; the chain ends at ref type
//! `Z` (`Z { int z }`). Every ref type's namespace is `n` and its assembly
//! `a`. Rows and bytes are built without recursion.

use crate::writer::ByteWriter;

/// One level of a shape's nesting, outermost first (`walk.py`'s
/// `DEPTH_SHAPES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    /// A class holding the next level.
    Class,
    /// A vector (unflagged element) of the next level.
    Vector,
    /// A vector whose element, the next level, carries the align flag: the
    /// pure-Python reader's `read_value_array`.
    Aligned,
    /// An aligned vector of a class holding the next level.
    ClassArray,
    /// A pair whose `first` is the next level.
    Pair,
    /// An aligned vector of pairs whose `first` is a class holding the next
    /// level (`read_value_array`'s `lambda`).
    PairArray,
}

impl Wrap {
    /// The wrap `walk.py` names `name`.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        Some(match name {
            "Class" => Self::Class,
            "Vector" => Self::Vector,
            "Aligned" => Self::Aligned,
            "ClassArray" => Self::ClassArray,
            "Pair" => Self::Pair,
            "PairArray" => Self::PairArray,
            _ => return None,
        })
    }

    fn vector(self) -> bool {
        matches!(
            self,
            Self::Vector | Self::Aligned | Self::ClassArray | Self::PairArray
        )
    }
}

/// A typetree row: level, type, name, meta flag.
pub type Row = (u8, &'static str, &'static str, i32);

/// The align flag.
pub const ALIGN: i32 = 0x4000;

/// Ref type `Z`'s rows, which ends a chain.
pub const Z_ROWS: [Row; 2] = [(0, "Z", "Base", 0), (1, "int", "z", 0)];

/// One shape: ref type `A`'s rows (also the object's tree) and the bytes of
/// one `A` around its `ReferencedObject`.
#[derive(Clone, Debug)]
pub struct Depth {
    /// Ref type `A`'s rows.
    pub rows: Vec<Row>,
    prefix: Vec<u8>,
    suffix: Vec<u8>,
}

impl Depth {
    /// `walk.py`'s `depth_rows`.
    ///
    /// # Panics
    ///
    /// Past 255 levels.
    #[must_use]
    pub fn new(wraps: &[Wrap]) -> Self {
        let mut rows = vec![(0, "A", "Base", 0)];
        let (mut level, mut name, mut meta) = (1u8, "x", 0);
        let mut tails = Vec::new();
        let at = |level: u8, by: u8| level.checked_add(by).expect("at most 255 levels");
        for &w in wraps {
            if w.vector() {
                rows.extend([
                    (level, "vector", name, meta),
                    (at(level, 1), "Array", "Array", 0),
                    (at(level, 2), "int", "size", 0),
                ]);
                (level, name, meta) = (
                    at(level, 2),
                    "data",
                    if w == Wrap::Vector { 0 } else { ALIGN },
                );
            }
            match w {
                Wrap::Class | Wrap::ClassArray => {
                    rows.push((level, "C", name, meta));
                    (level, name, meta) = (at(level, 1), "c", 0);
                }
                Wrap::Pair => {
                    rows.push((level, "pair", name, meta));
                    tails.push((at(level, 1), "int", "second", 0));
                    (level, name, meta) = (at(level, 1), "first", 0);
                }
                Wrap::PairArray => {
                    rows.extend([(level, "pair", name, meta), (at(level, 1), "C", "first", 0)]);
                    tails.push((at(level, 1), "int", "second", 0));
                    (level, name, meta) = (at(level, 2), "c", 0);
                }
                Wrap::Vector | Wrap::Aligned => {}
            }
        }
        rows.extend([
            (level, "ReferencedObject", name, meta),
            (at(level, 1), "ReferencedManagedType", "type", 0),
            (at(level, 2), "string", "class", 0),
            (at(level, 2), "string", "ns", 0),
            (at(level, 2), "string", "asm", 0),
            (at(level, 1), "ReferencedObjectData", "data", 0),
        ]);
        rows.extend(tails.into_iter().rev());
        let mut prefix = ByteWriter::le();
        for w in wraps {
            if !matches!(w, Wrap::Class | Wrap::Pair) {
                prefix.i32(1);
            }
        }
        let mut suffix = ByteWriter::le();
        for w in wraps.iter().rev() {
            if matches!(w, Wrap::Pair | Wrap::PairArray) {
                suffix.i32(5);
            }
        }
        Self {
            rows,
            prefix: prefix.into_inner(),
            suffix: suffix.into_inner(),
        }
    }

    /// `n` `A`s, the last holding a `Z` (`walk.py`'s `depth_data`).
    #[must_use]
    pub fn data(&self, n: usize) -> Vec<u8> {
        let mut w = ByteWriter::le();
        for i in 1..=n {
            w.bytes(&self.prefix);
            for s in [if i == n { "Z" } else { "A" }, "n", "a"] {
                w.i32(i32::try_from(s.len()).unwrap_or(0))
                    .bytes(s.as_bytes())
                    .align(4);
            }
        }
        w.i32(7);
        let mut d = w.into_inner();
        d.extend(self.suffix.repeat(n));
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_array_level_nests_four_deep() {
        let d = Depth::new(&[Wrap::PairArray]);
        let types: Vec<&str> = d.rows.iter().map(|r| r.1).collect();
        assert_eq!(
            types,
            [
                "A",
                "vector",
                "Array",
                "int",
                "pair",
                "C",
                "ReferencedObject",
                "ReferencedManagedType",
                "string",
                "string",
                "string",
                "ReferencedObjectData",
                "int"
            ]
        );
        assert_eq!(d.rows.last(), Some(&(4, "int", "second", 0)));
        assert_eq!(d.data(1).len(), 4 + 3 * 8 + 4 + 4);
    }
}
