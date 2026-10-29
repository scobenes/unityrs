//! `SerializedFile`s, every format from 9 to 22, built from a spec.
//!
//! Owner: package C.

// STUB(C): delete this allow when the builder is implemented.
#![allow(
    clippy::todo,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    unused_variables
)]

use crate::blob::NodeSpec;

/// One type record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeSpec {
    /// `class_id`.
    pub class_id: i32,
    /// `is_stripped_type` (format 16 and up).
    pub stripped: bool,
    /// `script_type_index` (format 17 and up).
    pub script_type_index: i16,
    /// `script_id`, written when the class is `MonoBehaviour` (or negative) at 13 and up.
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
        }
    }
}

/// The file's bytes.
pub fn build(spec: &SerializedSpec) -> Vec<u8> {
    todo!()
}
