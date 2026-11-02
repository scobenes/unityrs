// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py. See THIRD_PARTY_LICENSES.
//! A file record's `l1` (`dump_records.py`'s L1 builders), copied from the
//! core's tables.
//!
//! **Memory.** Every copy whose size the input chooses (a block row per
//! block, a directory row per entry, a type and its dependencies, a path id
//! per object, a script, an external, a container row, the record's path) is
//! a vector charged to the input's meter before it is allocated
//! ([`MeteredVec`]), its charge held with the record's (`held`) until the
//! record's line is written; so is a packed version of 2^128 or more, whose
//! limbs the record copies (up to 226, 1,808 bytes: [`packed`]). A refusal
//! of the meter is the input's `InputMemory` refusal, and of the budget or
//! the allocator its `crash` record ([`super::dump_on`]), never an abort of
//! the run. Each row's own strings are the core's, shared, not copied.

use std::sync::Arc;

use unityrs_core::files::{BlockInfo, DirEntry, LayoutKind, SerializedType, WebPacker};
use unityrs_core::limits::{Charge, ChargeError, MeteredVec, heap_cost};
use unityrs_core::stream::Endian;
use unityrs_core::version::Packed;
use unityrs_core::{BundleFile, Error, RawStr, SerializedFile, WebFile};

use crate::record::{
    BlockRow, BundleL1, ContainerRow, DirRow, ExternalRow, SerializedHeaderRow, SerializedL1,
    TypeRecord, WebL1,
};

/// UnityPy's `SerializedFile.unity_version` below format 7, where the file
/// stores none (`files/SerializedFile.py:237`).
pub const DEFAULT_UNITY_VERSION: &str = "2.5.0f5";

/// An empty table with room for `n` rows, charged first to `held`'s meter.
fn table<T>(held: &Charge, n: usize) -> Result<MeteredVec<T>, ChargeError> {
    MeteredVec::with_capacity(held.meter(), n)
}

/// `table`'s rows, its charge kept with `held`.
fn keep<T>(held: &mut Charge, table: MeteredVec<T>) -> Vec<T> {
    let (rows, charge) = table.into_parts();
    held.absorb(charge);
    rows
}

/// A copy of `rows`, at most `n` of them, charged to `held` before it is
/// allocated.
pub(super) fn copy<T>(
    held: &mut Charge,
    n: usize,
    rows: impl IntoIterator<Item = T>,
) -> Result<Vec<T>, ChargeError> {
    let mut t = table(held, n)?;
    for row in rows {
        t.push(row)?;
    }
    Ok(keep(held, t))
}

/// A copy of a packed version for a record: a value of 2^128 or more owns
/// its limbs, which are charged to `held`, at [`heap_cost`], before they
/// are copied. Round 7 left the copy uncharged.
pub(super) fn packed(p: &Packed, held: &mut Charge) -> Result<Packed, ChargeError> {
    let heap = u64::try_from(p.heap_bytes()).unwrap_or(u64::MAX);
    if heap > 0 {
        held.grow(heap_cost(heap))?;
    }
    Ok(p.clone())
}

fn block_row(bi: &BlockInfo) -> BlockRow {
    BlockRow {
        uncompressed: bi.uncompressed,
        compressed: bi.bytes_read,
        flags: bi.flags,
    }
}

pub(super) fn bundle_l1(b: &BundleFile, held: &mut Charge) -> Result<BundleL1, ChargeError> {
    let h = b.header();
    let blocks = if h.layout == LayoutKind::Fs {
        Some(copy(
            held,
            b.blocks().len(),
            b.blocks().iter().map(block_row),
        )?)
    } else {
        None
    };
    Ok(BundleL1 {
        signature: h.signature.clone(),
        format: h.format,
        version_player: h.version_player.clone(),
        version_engine: h.version_engine.clone(),
        engine_version_packed: match &h.engine_version {
            Some(Ok(v)) => Some(packed(v.version.packed(), held)?),
            _ => None,
        },
        flags: h.data_flags,
        flags_class: h.flags_class,
        block_alignment: h.block_alignment,
        encrypted: h.encrypted,
        hash: h.hash,
        crc: h.crc,
        blocks_info: h.blocks_info.as_ref().map(block_row),
        blocks,
        directory: dir_rows(b.directory(), held)?,
    })
}

fn dir_rows(dir: &[DirEntry], held: &mut Charge) -> Result<Vec<DirRow>, ChargeError> {
    copy(
        held,
        dir.len(),
        dir.iter().map(|d| DirRow {
            offset: d.offset,
            size: d.size,
            flags: d.flags,
            path: d.path.clone(),
        }),
    )
}

pub(super) fn web_l1(w: &WebFile, held: &mut Charge) -> Result<WebL1, ChargeError> {
    Ok(WebL1 {
        signature: w.signature().clone(),
        packer: match w.packer() {
            WebPacker::None => "none",
            WebPacker::Gzip => "gzip",
            WebPacker::Brotli => "brotli",
        },
        directory: dir_rows(w.directory(), held)?,
    })
}

fn type_record(t: &SerializedType, held: &mut Charge) -> Result<TypeRecord, ChargeError> {
    let type_dependencies = match &t.type_dependencies {
        Some(d) => Some(copy(held, d.len(), d.iter().copied())?),
        None => None,
    };
    Ok(TypeRecord {
        class_id: t.class_id,
        is_stripped_type: t.is_stripped_type,
        script_type_index: t.script_type_index,
        script_id: t.script_id,
        old_type_hash: t.old_type_hash,
        nodes: t.nodes.clone(),
        type_dependencies,
        class_name: t.class_name.clone(),
        namespace: t.namespace.clone(),
        assembly: t.assembly.clone(),
    })
}

fn type_records(
    types: &[SerializedType],
    held: &mut Charge,
) -> Result<Vec<TypeRecord>, ChargeError> {
    let mut t = table(held, types.len())?;
    for st in types {
        t.push(type_record(st, held)?)?;
    }
    Ok(keep(held, t))
}

/// `serialized_l1`.
pub(super) fn serialized_l1(
    sf: &Arc<SerializedFile>,
    held: &mut Charge,
) -> Result<SerializedL1, Error> {
    let h = sf.header();
    let infos = sf.object_infos();
    let order = sf.object_order();
    let object_order = copy(
        held,
        order.len(),
        order
            .iter()
            .filter_map(|i| usize::try_from(*i).ok().and_then(|i| infos.get(i)))
            .map(|o| o.path_id),
    )?;
    let scripts = sf.script_types();
    let script_types = copy(
        held,
        scripts.len(),
        scripts.iter().map(|s| (s.file_index, s.identifier)),
    )?;
    let externals = sf.externals();
    let externals = copy(
        held,
        externals.len(),
        externals.iter().map(|e| ExternalRow {
            temp_empty: e.temp_empty.clone(),
            guid: e.guid,
            kind: e.kind,
            path: e.path.clone(),
        }),
    )?;
    let ref_types = match sf.ref_types() {
        Some(t) => Some(type_records(t, held)?),
        None => None,
    };
    let rows = sf.container()?.rows();
    let container = copy(
        held,
        rows.len(),
        rows.iter().map(|r| ContainerRow {
            path: r.path.clone(),
            preload_index: r.preload_index,
            preload_size: r.preload_size,
            file_id: r.file_id,
            path_id: r.path_id,
        }),
    )?;
    Ok(SerializedL1 {
        header: SerializedHeaderRow {
            metadata_size: h.metadata_size,
            file_size: h.file_size,
            version: h.version,
            data_offset: h.data_offset,
            big_endian: h.endian == Endian::Big,
            reserved: h.reserved,
            unknown: h.unknown,
        },
        unity_version: sf
            .unity_version()
            .cloned()
            .unwrap_or_else(|| RawStr::from(DEFAULT_UNITY_VERSION)),
        version_packed: match sf.version() {
            Some(v) => Some(packed(v.version.packed(), held)?),
            None => None,
        },
        target_platform: sf.target_platform(),
        enable_type_tree: sf.enable_type_tree(),
        big_id_enabled: sf.big_id_enabled(),
        types: type_records(sf.types(), held)?,
        object_order,
        script_types,
        externals,
        ref_types,
        user_information: sf.user_information().cloned(),
        container,
    })
}
