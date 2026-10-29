//! `SerializedFile`s, every format from 1 to 22 (below 9 the header's endian
//! byte sits at `file_size - metadata_size`), built from a spec, with
//! overrides for the fields a parser test needs to get wrong.
//!
//! The layout is the one UnityPy 1.25.2 reads (`files/SerializedFile.py:235-324`,
//! `files/ObjectReader.py:67-130`):
//! - the header's four `u32`s, big-endian whatever the file's byte order;
//!   from format 9 the endian byte and three reserved bytes follow at 16, and
//!   at 22 the 64-bit fields (still big-endian: UnityPy switches the byte
//!   order only after them, `SerializedFile.py:260-269`);
//! - the metadata, in the file's byte order, right after the header from
//!   format 9, and at the end of the file below 9, where its first byte is
//!   the endian byte (`:266`);
//! - the objects' data after the metadata (from format 9; below 9 after the
//!   header), at `data_offset`, each object's start aligned to 8 within it.
//!
//! Owner: package C.

use crate::blob::{NodeSpec, Strings, blob_tree, old_tree};
use crate::writer::ByteWriter;

/// One type record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeSpec {
    /// `class_id`.
    pub class_id: i32,
    /// `is_stripped_type` (format 16 and up).
    pub stripped: bool,
    /// `script_type_index` (format 17 and up).
    pub script_type_index: i16,
    /// `script_id`, written under UnityPy's condition (plan section 3.7).
    pub script_id: [u8; 16],
    /// `old_type_hash` (13 and up).
    pub old_type_hash: [u8; 16],
    /// The tree, written when type trees are enabled.
    pub nodes: Vec<NodeSpec>,
    /// Ref types at 21 and up: class, namespace, assembly.
    pub ref_names: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// Plain types at 21 and up: `type_dependencies`.
    pub dependencies: Vec<i32>,
}

/// One object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectSpec {
    /// `path_id`.
    pub path_id: i64,
    /// From 16 the index into `types`; below 16 the class id.
    pub type_id: i32,
    /// Below 16, the stored `class_id`.
    pub class_id: u16,
    /// The object's data, placed after the metadata in object order,
    /// each start aligned to 8.
    pub data: Vec<u8>,
    /// Overrides the stored `byte_size` (for mismatch tests).
    pub byte_size: Option<u32>,
    /// The object row's `script_type_index` (formats 11-16), which loading
    /// writes back into the object's type; `None` writes -1.
    pub script_type_index: Option<i16>,
    /// `is_destroyed` (below format 11); `None` writes 0.
    pub is_destroyed: Option<u16>,
    /// `is_stripped` (formats 15 and 16); `None` writes 0.
    pub is_stripped: Option<i8>,
}

/// One external.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalSpec {
    /// `temp_empty` (6 and up).
    pub temp_empty: Vec<u8>,
    /// `guid` (5 and up).
    pub guid: [u8; 16],
    /// `type` (5 and up).
    pub kind: i32,
    /// `path`.
    pub path: Vec<u8>,
}

/// A whole file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerializedSpec {
    /// The format number.
    pub format: u32,
    /// Big-endian metadata (the endian byte).
    pub big_endian: bool,
    /// The version string (7 and up).
    pub unity_version: Vec<u8>,
    /// The target platform (8 and up).
    pub platform: i32,
    /// `enable_type_tree` (13 and up).
    pub type_trees: bool,
    /// Type records.
    pub types: Vec<TypeSpec>,
    /// Objects, in table order.
    pub objects: Vec<ObjectSpec>,
    /// Script type references: `(file_index, identifier)`.
    pub scripts: Vec<(i32, i64)>,
    /// Externals.
    pub externals: Vec<ExternalSpec>,
    /// Ref types (20 and up).
    pub ref_types: Vec<TypeSpec>,
    /// `userInformation` (5 and up).
    pub user_information: Vec<u8>,
    /// `big_id_enabled` (formats 7-13): path ids as `i64` when non-zero.
    pub big_id_enabled: i32,
    /// Overrides the header's `metadata_size` (below 9 it also places the
    /// endian byte, at `file_size - metadata_size`).
    pub metadata_size: Option<u32>,
    /// Overrides the header's `file_size`.
    pub file_size: Option<i64>,
    /// Overrides the header's `data_offset`.
    pub data_offset: Option<i64>,
}

impl Default for SerializedSpec {
    fn default() -> Self {
        Self {
            format: 22,
            big_endian: false,
            unity_version: b"2022.3.0f1".to_vec(),
            platform: 13,
            type_trees: true,
            types: Vec::new(),
            objects: Vec::new(),
            scripts: Vec::new(),
            externals: Vec::new(),
            ref_types: Vec::new(),
            user_information: Vec::new(),
            big_id_enabled: 0,
            metadata_size: None,
            file_size: None,
            data_offset: None,
        }
    }
}

/// The header's length: 16 bytes below 9, 20 from 9, 48 at 22 and up.
fn header_len(format: u32) -> usize {
    match format {
        0..=8 => 16,
        9..=21 => 20,
        _ => 48,
    }
}

/// The file's bytes.
#[must_use]
pub fn build(spec: &SerializedSpec) -> Vec<u8> {
    let v = spec.format;
    let header = header_len(v);
    // The objects' data, each start aligned to 8, and each object's offset
    // in it.
    let mut data = ByteWriter::le();
    let mut starts = Vec::with_capacity(spec.objects.len());
    for o in &spec.objects {
        data.align(8);
        starts.push(data.len());
        data.bytes(&o.data);
    }
    let data = data.into_inner();

    // The metadata. Below 9 it starts with the endian byte and sits at the
    // end of the file; from 9 it follows the header, and alignments inside
    // it (`align_stream`) count from the file's start, so it is written at
    // its absolute offset.
    let meta_start = if v >= 9 { header } else { header + data.len() };
    let mut w = if spec.big_endian {
        ByteWriter::be()
    } else {
        ByteWriter::le()
    };
    w.bytes(&vec![0; meta_start]);
    if v < 9 {
        w.bool(spec.big_endian);
    }
    metadata(spec, &starts, &mut w);
    let mut file = w.into_inner();
    let meta_end = file.len();
    let metadata_size = meta_end - meta_start;

    let data_offset = if v >= 9 {
        let at = meta_end.div_ceil(16) * 16;
        file.resize(at, 0);
        file.extend_from_slice(&data);
        at
    } else {
        // Below 9 the data sits between the header and the metadata.
        if let Some(region) = file.get_mut(header..header + data.len()) {
            region.copy_from_slice(&data);
        }
        header
    };
    let file_size = spec
        .file_size
        .unwrap_or_else(|| i64::try_from(file.len()).unwrap_or(i64::MAX));
    let metadata_size = spec
        .metadata_size
        .unwrap_or_else(|| u32::try_from(metadata_size).unwrap_or(u32::MAX));
    let data_offset = spec
        .data_offset
        .unwrap_or_else(|| i64::try_from(data_offset).unwrap_or(i64::MAX));

    // The header, always big-endian.
    let mut h = ByteWriter::be();
    if v >= 22 {
        h.u32(0).u32(0).u32(v).u32(0);
    } else {
        h.u32(metadata_size)
            .u32(truncate_u32(file_size))
            .u32(v)
            .u32(truncate_u32(data_offset));
    }
    if v >= 9 {
        h.bool(spec.big_endian).bytes(&[0; 3]);
    }
    if v >= 22 {
        h.u32(metadata_size).i64(file_size).i64(data_offset).i64(0);
    }
    let h = h.into_inner();
    if let Some(region) = file.get_mut(..h.len()) {
        region.copy_from_slice(&h);
    }
    file
}

fn truncate_u32(v: i64) -> u32 {
    let b = v.to_le_bytes();
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The metadata after the endian byte, in UnityPy's reading order.
fn metadata(spec: &SerializedSpec, starts: &[usize], w: &mut ByteWriter) {
    let v = spec.format;
    if v >= 7 {
        w.cstr(&spec.unity_version);
    }
    if v >= 8 {
        w.i32(spec.platform);
    }
    let trees = v < 13 || spec.type_trees;
    if v >= 13 {
        w.bool(spec.type_trees);
    }
    w.i32(len_i32(spec.types.len()));
    for t in &spec.types {
        type_record(t, v, spec.big_endian, trees, false, w);
    }
    // UnityPy reads `big_id_enabled` at 7-13 only and uses it as stored.
    let big_id = (7..14).contains(&v) && spec.big_id_enabled != 0;
    if (7..14).contains(&v) {
        w.i32(spec.big_id_enabled);
    }
    w.i32(len_i32(spec.objects.len()));
    for (o, start) in spec.objects.iter().zip(starts) {
        if big_id {
            w.i64(o.path_id);
        } else if v < 14 {
            w.i32(truncate_i32(o.path_id));
        } else {
            w.align(4);
            w.i64(o.path_id);
        }
        let start = u32::try_from(*start).unwrap_or(u32::MAX);
        if v >= 22 {
            w.i64(i64::from(start));
        } else {
            w.u32(start);
        }
        let size = u32::try_from(o.data.len()).unwrap_or(u32::MAX);
        w.u32(o.byte_size.unwrap_or(size));
        w.i32(o.type_id);
        if v < 16 {
            w.u16(o.class_id);
        }
        if v < 11 {
            w.u16(o.is_destroyed.unwrap_or(0));
        }
        if (11..17).contains(&v) {
            w.i16(o.script_type_index.unwrap_or(-1));
        }
        if v == 15 || v == 16 {
            w.i8(o.is_stripped.unwrap_or(0));
        }
    }
    if v >= 11 {
        w.i32(len_i32(spec.scripts.len()));
        for (file_index, identifier) in &spec.scripts {
            w.i32(*file_index);
            if v < 14 {
                w.i32(truncate_i32(*identifier));
            } else {
                w.align(4);
                w.i64(*identifier);
            }
        }
    }
    w.i32(len_i32(spec.externals.len()));
    for e in &spec.externals {
        if v >= 6 {
            w.cstr(&e.temp_empty);
        }
        if v >= 5 {
            w.bytes(&e.guid);
            w.i32(e.kind);
        }
        w.cstr(&e.path);
    }
    if v >= 20 {
        w.i32(len_i32(spec.ref_types.len()));
        for t in &spec.ref_types {
            type_record(t, v, spec.big_endian, trees, true, w);
        }
    }
    if v >= 5 {
        w.cstr(&spec.user_information);
    }
}

/// One `SerializedType` (`SerializedFile.py:112-148`).
fn type_record(
    t: &TypeSpec,
    v: u32,
    big_endian: bool,
    trees: bool,
    is_ref_type: bool,
    w: &mut ByteWriter,
) {
    w.i32(t.class_id);
    if v >= 16 {
        w.bool(t.stripped);
    }
    // Below 17 the record holds -1 (`SerializedType.script_type_index`).
    let script_type_index = if v >= 17 { t.script_type_index } else { -1 };
    if v >= 17 {
        w.i16(t.script_type_index);
    }
    if v >= 13 {
        if (is_ref_type && script_type_index >= 0)
            || (v < 16 && t.class_id < 0)
            || (v >= 16 && t.class_id == 114)
        {
            w.bytes(&t.script_id);
        }
        w.bytes(&t.old_type_hash);
    }
    if trees {
        if v >= 12 || v == 10 {
            w.bytes(&blob_tree(&t.nodes, v, big_endian, Strings::Local, &[]));
        } else {
            w.bytes(&old_tree(&t.nodes, v, big_endian));
        }
        if v >= 21 {
            if is_ref_type {
                let empty = (Vec::new(), Vec::new(), Vec::new());
                let (class, ns, asm) = t.ref_names.as_ref().unwrap_or(&empty);
                w.cstr(class).cstr(ns).cstr(asm);
            } else {
                w.i32(len_i32(t.dependencies.len()));
                for d in &t.dependencies {
                    w.i32(*d);
                }
            }
        }
    }
}

fn len_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn truncate_i32(v: i64) -> i32 {
    let b = v.to_le_bytes();
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// A minimal file of one object whose class has the tree `nodes`: `data` is
/// the object's bytes, `class_id` its class. Every other field as
/// [`SerializedSpec::default`] has it (format 22, little-endian, type trees
/// on).
#[must_use]
pub fn one_object(class_id: i32, nodes: Vec<NodeSpec>, data: Vec<u8>) -> SerializedSpec {
    SerializedSpec {
        types: vec![TypeSpec {
            class_id,
            nodes,
            ..TypeSpec::default()
        }],
        objects: vec![ObjectSpec {
            path_id: 1,
            type_id: 0,
            class_id: u16::try_from(class_id).unwrap_or(0),
            data,
            ..ObjectSpec::default()
        }],
        ..SerializedSpec::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec, build, one_object};
    use crate::blob::NodeSpec;

    #[test]
    fn overrides_default_to_what_unity_writes() {
        let o = ObjectSpec::default();
        assert_eq!(
            (o.script_type_index, o.is_destroyed, o.is_stripped),
            (None, None, None)
        );
        let s = SerializedSpec::default();
        assert_eq!(s.big_id_enabled, 0);
        assert_eq!(
            (s.metadata_size, s.file_size, s.data_offset),
            (None, None, None)
        );
        assert_eq!(NodeSpec::new(0, "Base", "Base").index, None);
    }

    fn be_u32(b: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_format_22_header_is_big_endian_with_its_64_bit_fields() {
        let spec = one_object(1, vec![NodeSpec::new(0, "Base", "Base")], vec![7; 4]);
        let f = build(&spec);
        assert_eq!(be_u32(&f, 8), 22);
        assert_eq!(f[16], 0, "little-endian metadata");
        let file_size = i64::from_be_bytes(f[24..32].try_into().unwrap());
        let data_offset = i64::from_be_bytes(f[32..40].try_into().unwrap());
        assert_eq!(usize::try_from(file_size).unwrap(), f.len());
        assert_eq!(data_offset % 16, 0);
        assert_eq!(&f[usize::try_from(data_offset).unwrap()..], &[7; 4]);
        // The metadata starts with the version string.
        assert_eq!(&f[48..59], b"2022.3.0f1\0");
    }

    #[test]
    fn below_9_the_metadata_is_at_the_end_after_the_endian_byte() {
        let spec = SerializedSpec {
            format: 8,
            big_endian: true,
            externals: vec![ExternalSpec {
                path: b"x".to_vec(),
                ..ExternalSpec::default()
            }],
            objects: vec![ObjectSpec {
                path_id: 3,
                data: vec![1, 2, 3],
                ..ObjectSpec::default()
            }],
            types: vec![TypeSpec {
                class_id: 1,
                nodes: vec![NodeSpec::new(0, "Base", "Base")],
                ..TypeSpec::default()
            }],
            ..SerializedSpec::default()
        };
        let f = build(&spec);
        let metadata_size = usize::try_from(be_u32(&f, 0)).unwrap();
        let file_size = usize::try_from(be_u32(&f, 4)).unwrap();
        assert_eq!(file_size, f.len());
        assert_eq!(f[file_size - metadata_size], 1, "the endian byte");
        assert_eq!(be_u32(&f, 12), 16, "data right after the header");
        assert_eq!(&f[16..19], &[1, 2, 3]);
        assert_eq!(&f[file_size - metadata_size + 1..][..11], b"2022.3.0f1\0");
    }

    #[test]
    fn the_overrides_replace_the_header_fields() {
        let spec = SerializedSpec {
            format: 17,
            metadata_size: Some(5),
            file_size: Some(-1),
            data_offset: Some(0x1_0000_0002),
            ..SerializedSpec::default()
        };
        let f = build(&spec);
        assert_eq!(be_u32(&f, 0), 5);
        assert_eq!(be_u32(&f, 4), u32::MAX);
        assert_eq!(be_u32(&f, 12), 2);
    }
}
