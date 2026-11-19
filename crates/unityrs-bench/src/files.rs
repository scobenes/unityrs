//! The two containers every fixture is written in: a `SerializedFile` whose
//! objects of one class and shape share one type record (as Unity writes
//! them), and a `UnityFS` bundle of entries.

use unityrs_testkit::bundle::{self, BundleSpec, EntrySpec, TestCodec};
use unityrs_testkit::ngui::{Object, nodes, write};
use unityrs_testkit::serialized::{self, ObjectSpec, SerializedSpec, TypeSpec};
use unityrs_testkit::writer::ByteWriter;

pub use unityrs_testkit::ngui::BuildError;

/// `StandaloneWindows64`, every file's target platform.
const PLATFORM: i32 = 19;

/// A little-endian `SerializedFile` of `format` (22 for 2022.3, 21 for
/// 2019.4) and engine `version`, holding `objects` in order with their
/// typetrees: one type record for each distinct class and tree.
pub fn serialized(format: u32, version: &str, objects: &[Object]) -> Result<Vec<u8>, BuildError> {
    let mut types: Vec<TypeSpec> = Vec::new();
    let mut rows = Vec::with_capacity(objects.len());
    for o in objects {
        let tree = nodes(0, "Base", &o.value)?;
        let found = types
            .iter()
            .position(|t| t.class_id == o.class_id && t.nodes == tree);
        let at = if let Some(at) = found {
            at
        } else {
            types.push(TypeSpec {
                class_id: o.class_id,
                script_type_index: -1,
                nodes: tree,
                ..TypeSpec::default()
            });
            types.len() - 1
        };
        let mut w = ByteWriter::le();
        write(&mut w, &o.value)?;
        rows.push(ObjectSpec {
            path_id: o.path_id,
            type_id: i32::try_from(at).map_err(|_| BuildError::TooManyObjects(at))?,
            class_id: u16::try_from(o.class_id).unwrap_or(0),
            data: w.into_inner(),
            ..ObjectSpec::default()
        });
    }
    Ok(serialized::build(&SerializedSpec {
        format,
        unity_version: version.as_bytes().to_vec(),
        platform: PLATFORM,
        types,
        objects: rows,
        ..SerializedSpec::default()
    }))
}

/// A format-8 `UnityFS` bundle of engine `version`: `entries` as
/// `(name, bytes, flags)` (flags 4 for a `SerializedFile`), the data cut
/// into blocks of `block_size` and each stored with `codec`; blocks-info
/// in LZ4.
#[must_use]
pub fn bundle(
    version: &str,
    entries: Vec<(String, Vec<u8>, u32)>,
    codec: TestCodec,
    block_size: usize,
) -> Vec<u8> {
    bundle::build(&BundleSpec {
        format: 8,
        version_engine: version.as_bytes().to_vec(),
        data_codec: codec,
        block_size,
        entries: entries
            .into_iter()
            .map(|(name, data, flags)| EntrySpec {
                name: name.into_bytes(),
                data,
                flags,
            })
            .collect(),
        ..BundleSpec::default()
    })
}
