//! The two containers every fixture is written in: a `SerializedFile` whose
//! objects of one class and shape share one type record (as Unity writes
//! them), and a `UnityFS` bundle of entries.

use std::cell::Cell;

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
/// into blocks of `block_size` and each stored with `codec` by the
/// testkit's encoders; blocks-info in LZ4. Not for LZ4HC, which the
/// testkit writes as plain LZ4: [`bundle_lz4hc`] encodes it.
#[must_use]
pub fn bundle(
    version: &str,
    entries: Vec<(String, Vec<u8>, u32)>,
    codec: TestCodec,
    block_size: usize,
) -> Vec<u8> {
    bundle::build(&spec(version, entries, codec, block_size))
}

/// [`bundle`] with LZ4HC data blocks encoded as Unity encodes them: liblz4
/// 1.9.4's HC at level 9, the encoder unityrs's writer links
/// (`unityrs-compress-sys`).
pub fn bundle_lz4hc(
    version: &str,
    entries: Vec<(String, Vec<u8>, u32)>,
    block_size: usize,
) -> Result<Vec<u8>, String> {
    let failed = Cell::new(None);
    let bytes = bundle::build_with_encoder(
        &spec(version, entries, TestCodec::Lz4Hc, block_size),
        b"UnityFS",
        |codec, data| match codec {
            TestCodec::Lz4Hc => lz4hc(data).unwrap_or_else(|e| {
                failed.set(Some(e));
                Vec::new()
            }),
            _ => bundle::compress(codec, data),
        },
    );
    match failed.get() {
        Some(e) => Err(format!("LZ4HC encoder: {e:?}")),
        None => Ok(bytes),
    }
}

/// One LZ4 block of `data` by liblz4's HC compressor.
fn lz4hc(data: &[u8]) -> Result<Vec<u8>, unityrs_compress_sys::Error> {
    let mut out = vec![0; unityrs_compress_sys::lz4_bound(data.len())?];
    let mut state = vec![0; unityrs_compress_sys::lz4_state_words()?];
    let written = unityrs_compress_sys::lz4_compress(data, &mut out, &mut state)?;
    out.truncate(written);
    Ok(out)
}

fn spec(
    version: &str,
    entries: Vec<(String, Vec<u8>, u32)>,
    codec: TestCodec,
    block_size: usize,
) -> BundleSpec {
    BundleSpec {
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
    }
}
