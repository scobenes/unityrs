//! The fuzz targets' input layouts (`fuzz/`, the fuzz plan of
//! `docs/plans/core.md` section 6), in one place: the targets read their
//! inputs through these, and the seed command (`examples/fuzz_seeds.rs`)
//! writes the builders' files, trees, objects and blocks through them, so
//! the two cannot drift.
//!
//! - [`TreeCase`]: a typetree and the bytes of one value, for the targets
//!   that read and write values (`boost_read`, `python_read`,
//!   `write_after_read`). The rows are what the readers and the tree
//!   compiler read of a node (level, meta flag, type, name), so a mutation
//!   of a type name or a level is one byte away, not an offset into a
//!   string buffer.
//! - [`blob_case`]: a stored typetree, the bytes `TypeTreeNode.parse_blob`
//!   or `TypeTreeNode.parse` read, for the `typetree` target.
//! - [`lz4_case`]: one LZ4 block and its declared size, for the `lz4`
//!   target.
//!
//! Every decoder here takes any bytes: a field the input does not reach is
//! empty (a row, a reference, the data), never an error.

/// A typetree row: what the readers and the compiler read of a node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// `m_Level`.
    pub level: u8,
    /// `m_MetaFlag` (`0x4000` aligns).
    pub meta_flag: i32,
    /// `m_Type`.
    pub ty: Vec<u8>,
    /// `m_Name`.
    pub name: Vec<u8>,
}

impl Row {
    /// A row of `level`, `ty` and `name`, with meta flag `meta_flag`.
    #[must_use]
    pub fn new(level: u8, ty: &[u8], name: &[u8], meta_flag: i32) -> Self {
        Self {
            level,
            meta_flag,
            ty: ty.to_vec(),
            name: name.to_vec(),
        }
    }
}

/// A ref type: its class, namespace and assembly, and its tree, if stored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefType {
    /// `m_ClassName`.
    pub class: Vec<u8>,
    /// `m_NameSpace`.
    pub namespace: Vec<u8>,
    /// `m_AssemblyName`.
    pub assembly: Vec<u8>,
    /// The stored tree; `None` is a ref type with none.
    pub rows: Option<Vec<Row>>,
}

/// A tree, the ref types it may reach, and the bytes of one value.
///
/// Layout: `flags` (a byte of [`TreeCase`]'s bits), `start` (an `i8`), the
/// rows, the count of ref types (a byte) and each ref type, then the value's
/// bytes, which run to the end. Rows are a `u16` count (little-endian) and
/// each row's level (a byte), meta flag (an `i32`, little-endian), type and
/// name; a string is a byte of length and its bytes. A ref type is its
/// class, namespace and assembly, a byte whose low bit says a tree follows,
/// and that tree's rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeCase {
    /// The bits below.
    pub flags: u8,
    /// Where the pure-Python reader starts, from the start of the data.
    pub start: i8,
    /// The value's tree.
    pub rows: Vec<Row>,
    /// The file's ref types, in order.
    pub refs: Vec<RefType>,
    /// The value's bytes.
    pub data: Vec<u8>,
}

impl TreeCase {
    /// The data is big-endian.
    pub const BIG_ENDIAN: u8 = 1;
    /// The file has no `ref_types` (format below 20), whatever `refs` holds.
    pub const REFS_ABSENT: u8 = 2;
    /// There is no file at all, whatever `refs` holds (wins over
    /// [`Self::REFS_ABSENT`]).
    pub const REFS_NO_FILE: u8 = 4;
    /// The pure-Python reader reads by the stream reader's rules (a file
    /// loaded from a path).
    pub const STREAM: u8 = 8;
    /// Float32 signalling NaNs keep their bits.
    pub const LOSSLESS: u8 = 16;
    /// `write_after_read` reads with the pure-Python reader, not
    /// UnityPyBoost's.
    pub const PYTHON: u8 = 32;
    /// The run bounds memory at the fuzz crate's small bound
    /// (`SMALL_MEMORY` in `fuzz/src/lib.rs`), which a read of many small
    /// values reaches before the work bound, not at the large one, which
    /// no value read reaches. Without it the code that grows a charged
    /// buffer at the memory bound never runs in the value targets.
    pub const SMALL_MEMORY: u8 = 64;

    /// Whether `flags` holds `bit`.
    #[must_use]
    pub fn has(&self, bit: u8) -> bool {
        self.flags & bit != 0
    }

    /// The case's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.flags, self.start.to_le_bytes()[0]];
        put_rows(&mut out, &self.rows);
        let refs = &self.refs[..self.refs.len().min(usize::from(u8::MAX))];
        out.push(u8::try_from(refs.len()).unwrap_or(u8::MAX));
        for r in refs {
            put_str(&mut out, &r.class);
            put_str(&mut out, &r.namespace);
            put_str(&mut out, &r.assembly);
            match &r.rows {
                Some(rows) => {
                    out.push(1);
                    put_rows(&mut out, rows);
                }
                None => out.push(0),
            }
        }
        out.extend_from_slice(&self.data);
        out
    }

    /// The case `input` holds; what it does not reach is empty.
    #[must_use]
    pub fn decode(input: &[u8]) -> Self {
        let mut c = Cursor(input);
        let mut case = Self {
            flags: c.u8().unwrap_or(0),
            start: c.u8().map_or(0, |b| i8::from_le_bytes([b])),
            ..Self::default()
        };
        let Some(rows) = c.rows() else {
            return case;
        };
        case.rows = rows;
        let Some(n) = c.u8() else {
            return case;
        };
        for _ in 0..n {
            let Some(r) = c.ref_type() else {
                return case;
            };
            case.refs.push(r);
        }
        case.data = c.0.to_vec();
        case
    }
}

/// The `typetree` target's input: the format (a byte), a byte whose low bit
/// says big-endian, then the stored tree.
#[must_use]
pub fn blob_case(format: u8, big_endian: bool, tree: &[u8]) -> Vec<u8> {
    let mut out = vec![format, u8::from(big_endian)];
    out.extend_from_slice(tree);
    out
}

/// The `lz4` target's input: the declared size (a `u32`, little-endian),
/// then the block.
#[must_use]
pub fn lz4_case(declared: u32, block: &[u8]) -> Vec<u8> {
    let mut out = declared.to_le_bytes().to_vec();
    out.extend_from_slice(block);
    out
}

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    let s = &s[..s.len().min(usize::from(u8::MAX))];
    out.push(u8::try_from(s.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(s);
}

fn put_rows(out: &mut Vec<u8>, rows: &[Row]) {
    let rows = &rows[..rows.len().min(usize::from(u16::MAX))];
    out.extend_from_slice(&u16::try_from(rows.len()).unwrap_or(u16::MAX).to_le_bytes());
    for r in rows {
        out.push(r.level);
        out.extend_from_slice(&r.meta_flag.to_le_bytes());
        put_str(out, &r.ty);
        put_str(out, &r.name);
    }
}

/// The bytes not yet read.
struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn str(&mut self) -> Option<Vec<u8>> {
        let n = self.u8()?;
        Some(self.take(usize::from(n))?.to_vec())
    }

    fn rows(&mut self) -> Option<Vec<Row>> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().ok()?);
        let mut rows = Vec::new();
        for _ in 0..n {
            let level = self.u8()?;
            let meta_flag = i32::from_le_bytes(self.take(4)?.try_into().ok()?);
            let ty = self.str()?;
            let name = self.str()?;
            rows.push(Row {
                level,
                meta_flag,
                ty,
                name,
            });
        }
        Some(rows)
    }

    fn ref_type(&mut self) -> Option<RefType> {
        let class = self.str()?;
        let namespace = self.str()?;
        let assembly = self.str()?;
        let rows = if self.u8()? & 1 == 1 {
            Some(self.rows()?)
        } else {
            None
        };
        Some(RefType {
            class,
            namespace,
            assembly,
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RefType, Row, TreeCase, blob_case, lz4_case};

    fn case() -> TreeCase {
        TreeCase {
            flags: TreeCase::BIG_ENDIAN | TreeCase::PYTHON,
            start: -3,
            rows: vec![
                Row::new(0, b"Base", b"Base", 0),
                Row::new(1, b"int", b"x", 0x4000),
            ],
            refs: vec![
                RefType {
                    class: b"A".to_vec(),
                    namespace: b"n".to_vec(),
                    assembly: b"a".to_vec(),
                    rows: Some(vec![Row::new(0, b"A", b"Base", 0)]),
                },
                RefType::default(),
            ],
            data: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn a_case_decodes_to_what_was_encoded() {
        let c = case();
        assert_eq!(TreeCase::decode(&c.encode()), c);
        assert!(c.has(TreeCase::PYTHON) && !c.has(TreeCase::STREAM));
    }

    #[test]
    fn a_cut_case_keeps_what_it_reaches_and_empties_the_rest() {
        let c = case();
        let bytes = c.encode();
        // Every cut decodes, and past the rows and the ref types only the
        // data is cut.
        for n in 0..bytes.len() {
            let d = TreeCase::decode(&bytes[..n]);
            assert!(d.data.len() <= c.data.len());
        }
        let cut = TreeCase::decode(&bytes[..bytes.len() - 1]);
        assert_eq!((cut.rows, cut.refs), (c.rows.clone(), c.refs.clone()));
        assert_eq!(cut.data, [1, 2, 3]);
        let rows_only = TreeCase::decode(&bytes[..bytes.len() - 20]);
        assert_eq!(rows_only.rows, c.rows);
        assert!(rows_only.refs.is_empty() && rows_only.data.is_empty());
        assert_eq!(TreeCase::decode(&[]), TreeCase::default());
    }

    #[test]
    fn long_strings_and_lists_are_cut_to_what_their_lengths_hold() {
        let c = TreeCase {
            rows: vec![Row::new(0, &[b'x'; 300], b"Base", 0)],
            ..TreeCase::default()
        };
        let d = TreeCase::decode(&c.encode());
        assert_eq!(d.rows[0].ty.len(), 255);
        assert_eq!(d.rows[0].name, b"Base");
    }

    #[test]
    fn the_blob_and_lz4_cases_put_their_header_first() {
        assert_eq!(blob_case(19, true, &[9]), [19, 1, 9]);
        assert_eq!(lz4_case(0x0102_0304, &[7]), [4, 3, 2, 1, 7]);
    }
}
