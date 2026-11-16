// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Charged snapshots of current Python save metadata, taken on the caller's
//! thread before dispatching the core writer. No Python getters run on workers.
use crate::{errors, files::NativeFile, object::NativeObject, python as p, save};
use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
};
use unityrs_core::{
    File, RawStr,
    files::{External, SaveLayout, SaveOptions, serialized::SerializedSaveSnapshot},
    limits::MeteredVec,
    stream::{Endian, ShortArray},
};

type Metadata = (
    (u32, char, Option<Py<PyAny>>, i64),
    Option<Py<PyAny>>,
    Option<i32>,
    bool,
    i32,
    Option<Py<PyAny>>,
);
type ObjectRow = (
    Py<NativeObject>,
    i64,
    i32,
    i32,
    Option<u16>,
    Option<i8>,
    Option<i16>,
    Py<PyAny>,
);
type ExternalRow = (Option<Py<PyAny>>, Option<Py<PyAny>>, Option<i32>, Py<PyAny>);

pub(crate) fn serialized(
    py: Python<'_>,
    file: &NativeFile,
    metadata: &Bound<'_, PyAny>,
    rows: &Bound<'_, PyAny>,
    externals: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let File::Serialized(source) = &file.inner else {
        return Err(PyTypeError::new_err("not a SerializedFile"));
    };
    let meter = source.meter();
    // The Python facade charges its lists and encodings before construction;
    // these native rows are separately charged before retaining handles.
    let ((version, endian, reserved, unknown), unity, platform, enabled, big_id, user) =
        metadata.extract::<Metadata>()?;
    let snapshot =
        |value: Py<PyAny>| crate::buffer_snapshot::BufferSnapshot::new(value.bind(py), meter);
    let reserved = reserved.map(snapshot).transpose()?;
    let unity = unity.map(snapshot).transpose()?;
    let user = user.map(snapshot).transpose()?;
    let mut objects = MeteredVec::with_capacity(meter, rows.len()?).map_err(errors::charge)?;
    for row in rows.try_iter()? {
        let (object, path_id, type_id, class_id, destroyed, stripped, script, data) =
            row?.extract::<ObjectRow>()?;
        let data = crate::buffer_snapshot::BufferSnapshot::writer(data.bind(py), meter)?;
        let mut info = object.borrow(py).inner.info().clone();
        info.path_id = path_id;
        info.type_id = type_id;
        info.class_id = class_id;
        info.is_destroyed = destroyed;
        info.is_stripped = stripped;
        info.script_type_index = script;
        objects.push((info, data)).map_err(errors::charge)?;
    }
    let external_rows = snapshot_externals(py, externals, meter)?;
    py.detach(move || {
        crate::worker::run(crate::worker::load_stack(&source.context().limits), || {
            let mut entries =
                MeteredVec::with_capacity(meter, objects.len()).map_err(errors::charge)?;
            for (info, payload) in objects.as_slice() {
                let mut bytes = unityrs_core::budget::DecodeBuf::new(
                    meter,
                    0,
                    u64::MAX,
                    unityrs_core::error::LimitKind::InputMemory,
                )
                .map_err(errors::charge)?;
                meter
                    .count_decode(payload.as_ref().len() as u64)
                    .map_err(errors::charge)?;
                bytes
                    .extend_from_slice(payload.as_ref())
                    .map_err(errors::charge)?;
                entries
                    .push((info.clone(), bytes.into_bytes().map_err(errors::charge)?))
                    .map_err(errors::charge)?;
            }
            let external_entries = external_snapshot(external_rows.as_slice(), meter)?;
            let mut header = source.header().clone();
            header.version = version;
            header.endian = match endian {
                '<' => Endian::Little,
                '>' => Endian::Big,
                _ => return Err(PyValueError::new_err("invalid endian")),
            };
            header.reserved = reserved
                .as_ref()
                .map(|value| {
                    value
                        .as_ref()
                        .try_into()
                        .map_err(|_| PyValueError::new_err("reserved must contain three bytes"))
                })
                .transpose()?;
            header.unknown = unknown;
            let snapshot = SerializedSaveSnapshot {
                header,
                unity_version: unity
                    .as_ref()
                    .map(|v| RawStr::charged(v.as_ref(), meter))
                    .transpose()
                    .map_err(errors::charge)?,
                target_platform: platform,
                enable_type_tree: enabled,
                big_id_enabled: big_id,
                objects: entries.as_slice(),
                externals: external_entries.as_slice(),
                user_information: user
                    .as_ref()
                    .map(|v| RawStr::charged(v.as_ref(), meter))
                    .transpose()
                    .map_err(errors::charge)?,
            };
            let bytes = source
                .save_with_snapshot(
                    SaveOptions {
                        layout: SaveLayout::UnityPy,
                    },
                    &snapshot,
                )
                .map_err(save::error)?;
            let _copy = meter
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                .map_err(errors::charge)?;
            p::bytes(&bytes)
        })
    })
}

type ExternalSnapshot = (
    Option<crate::buffer_snapshot::BufferSnapshot>,
    Option<crate::buffer_snapshot::BufferSnapshot>,
    Option<i32>,
    crate::buffer_snapshot::BufferSnapshot,
);

fn snapshot_externals(
    py: Python<'_>,
    externals: &Bound<'_, PyAny>,
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
) -> PyResult<MeteredVec<ExternalSnapshot>> {
    let snapshot =
        |value: Py<PyAny>| crate::buffer_snapshot::BufferSnapshot::new(value.bind(py), meter);
    let mut external_rows =
        MeteredVec::with_capacity(meter, externals.len()?).map_err(errors::charge)?;
    for row in externals.try_iter()? {
        let (empty, guid, kind, path) = row?.extract::<ExternalRow>()?;
        external_rows
            .push((
                empty.map(snapshot).transpose()?,
                guid.map(snapshot).transpose()?,
                kind,
                snapshot(path)?,
            ))
            .map_err(errors::charge)?;
    }
    Ok(external_rows)
}

fn external_snapshot(
    rows: &[ExternalSnapshot],
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
) -> PyResult<MeteredVec<External>> {
    let mut entries = MeteredVec::with_capacity(meter, rows.len()).map_err(errors::charge)?;
    for (empty, guid, kind, path) in rows {
        let mut entry =
            External::new(RawStr::charged(path.as_ref(), meter).map_err(errors::charge)?);
        entry.temp_empty = empty
            .as_ref()
            .map(|v| RawStr::charged(v.as_ref(), meter))
            .transpose()
            .map_err(errors::charge)?;
        entry.guid = guid.as_ref().map(|v| ShortArray::from_slice(v));
        entry.kind = *kind;
        entries.push(entry).map_err(errors::charge)?;
    }
    Ok(entries)
}
