// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/AudioClipConverter.py, UnityPy/export/ShaderConverter.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Current-value exporters; no operation rereads the original object payload.
use crate::{environment::NativeEnvironment, errors, object::NativeObject, python as p};
use pyo3::{
    exceptions::{PyException, PyNotImplementedError, PyValueError},
    prelude::*,
    pybacked::PyBackedBytes,
};
use std::sync::Arc;
use unityrs_core::{
    FileContext,
    budget::Reservation,
    limits::{Limits, Meter},
};

#[path = "exports_mesh.rs"]
mod mesh;

pub(crate) fn context(reader: Option<&NativeObject>) -> (Limits, Arc<Meter>) {
    if let Some(reader) = reader {
        let file = reader.inner.file();
        return (file.context().limits, file.meter().clone());
    }
    let context = FileContext::unitypy();
    let meter = Meter::for_input(&context.limits, context.budget, Reservation::Input);
    (context.limits, meter)
}

#[pyfunction]
#[pyo3(signature = (data, name, reader=None, vorbis_headers=None, vorbis_path=None))]
fn export_audio(
    py: Python<'_>,
    data: PyBackedBytes,
    name: PyBackedBytes,
    reader: Option<&NativeObject>,
    vorbis_headers: Option<PyBackedBytes>,
    vorbis_path: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = context(reader);
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            // export/AudioClipConverter.py:13-49. The facade handles current
            // inline/resource selection before this magic dispatch and decoder.
            let plan = unityrs_audio::plan(&data, true, &meter).map_err(audio_error)?;
            let headers = if plan.header().is_some_and(|header| header.mode == 15) {
                if let Some(bytes) = vorbis_headers.as_ref() {
                    Some(
                        unityrs_audio::VorbisHeaders::from_bytes(bytes, &meter)
                            .map_err(audio_error)?,
                    )
                } else if let Some(path) = vorbis_path.as_ref() {
                    Some(load_vorbis_path(path, &meter)?)
                } else {
                    None
                }
            } else {
                None
            };
            let files = unityrs_audio::decode_with_vorbis(&plan, &name, headers.as_ref(), &meter)
                .map_err(audio_error)?;
            let mut handoff = unityrs_core::limits::Charge::zero(&meter);
            handoff.grow(256).map_err(errors::charge)?;
            let result = p::dict()?;
            for file in files.as_slice() {
                handoff
                    .grow(
                        256 + (file.name.len() as u64).saturating_mul(4) + file.bytes.len() as u64,
                    )
                    .map_err(errors::charge)?;
                p::set(
                    &result,
                    &p::string(file.name.as_slice())?,
                    &p::bytes(file.bytes.as_slice())?,
                )?;
            }
            Ok(result)
        })
    })
}

fn load_vorbis_path(
    path: &Py<PyAny>,
    meter: &Arc<Meter>,
) -> PyResult<unityrs_audio::VorbisHeaders> {
    enum PathText {
        String(pyo3::pybacked::PyBackedStr),
        Bytes(PyBackedBytes),
    }
    let (path, _staging) = Python::attach(|py| -> PyResult<_> {
        let value = path.bind(py);
        let len = value.len()?;
        let staging = meter
            .charge(256 + (len as u64).saturating_mul(4))
            .map_err(errors::charge)?;
        let path = if value.is_instance_of::<pyo3::types::PyBytes>() {
            PathText::Bytes(value.extract()?)
        } else {
            PathText::String(value.extract()?)
        };
        Ok((path, staging))
    })?;
    let headers = match path {
        PathText::String(path) => {
            unityrs_audio::VorbisHeaders::load(std::path::Path::new(&*path), meter)
        }
        PathText::Bytes(path) => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                unityrs_audio::VorbisHeaders::load(
                    std::path::Path::new(std::ffi::OsStr::from_bytes(&path)),
                    meter,
                )
            }
            #[cfg(not(unix))]
            {
                let path = std::str::from_utf8(&path).map_err(|_| {
                    pyo3::exceptions::PyValueError::new_err("Vorbis header path must be UTF-8")
                })?;
                unityrs_audio::VorbisHeaders::load(std::path::Path::new(path), meter)
            }
        }
    };
    headers.map_err(audio_error)
}

fn audio_error(error: unityrs_audio::Error) -> PyErr {
    match error {
        unityrs_audio::Error::Charge(error) => errors::charge(error),
        // Unsupported codecs stay explicit gaps; FMOD is never bundled.
        unityrs_audio::Error::Unsupported { .. }
        | unityrs_audio::Error::UnsupportedContainer
        | unityrs_audio::Error::EmptyFirstSubsound
        | unityrs_audio::Error::VorbisHeadersRequired
        | unityrs_audio::Error::VorbisShortSubsound => {
            PyNotImplementedError::new_err(errors::message(&error))
        }
        _ => PyValueError::new_err(errors::message(&error)),
    }
}

#[pyfunction]
#[pyo3(signature = (input, version=(0,0), reader=None))]
fn export_shader(
    py: Python<'_>,
    input: Py<PyAny>,
    version: (u32, u32),
    reader: Option<&NativeObject>,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = context(reader);
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            let parsed = crate::value_input::parsed(&input, &meter, &limits)?;
            let bytes = unityrs_export::shader::export_parsed(&parsed, version, &meter)
                .map_err(|error| shader_error(error, &meter))?;
            let _handoff = meter
                .charge((bytes.len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            p::string(bytes.as_slice())
        })
    })
}

fn shader_error(error: unityrs_export::shader::Error, meter: &Arc<Meter>) -> PyErr {
    use unityrs_export::shader::Error;
    match error {
        Error::Charge(error) => errors::charge(error),
        Error::Core(unityrs_core::Error::Decompress { detail, .. }) => {
            let (name, message) = detail.split_once(": ").unwrap_or(("ValueError", &detail));
            named_error(name, errors::message(&message))
        }
        Error::Core(error) => errors::core(error),
        Error::Parse(error) => errors::parse(error),
        Error::Utf8Bytes(data) => {
            let handoff = meter.charge(256 + (data.len() as u64).saturating_mul(5));
            if let Err(error) = handoff {
                return errors::charge(error);
            }
            match p::bytes(data.as_ref()) {
                Ok(bytes) => {
                    Python::attach(
                        |py| match bytes.bind(py).call_method1("decode", ("utf8",)) {
                            Err(error) => error,
                            Ok(_) => {
                                PyValueError::new_err("invalid UTF-8 error contained valid bytes")
                            }
                        },
                    )
                }
                Err(error) => error,
            }
        }
        _ => named_error(
            error.exception().unwrap_or("NotImplementedError"),
            errors::message(&error),
        ),
    }
}

pub(super) fn named_error(name: &str, message: String) -> PyErr {
    Python::attach(|py| {
        let class = match name {
            "error" => py.import("struct").and_then(|m| m.getattr("error")),
            "LZ4BlockError" => py
                .import("unityrs._native")
                .and_then(|m| m.getattr("LZ4BlockError")),
            _ => py.import("builtins").and_then(|m| m.getattr(name)),
        };
        match class.and_then(|class| class.cast_into::<pyo3::types::PyType>().map_err(Into::into)) {
            Ok(class) => PyErr::from_type(class, (message,)),
            Err(error) => error,
        }
    })
}

pyo3::create_exception!(unityrs, LZ4BlockError, PyException);

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(export_audio, m)?)?;
    m.add_function(wrap_pyfunction!(export_shader, m)?)?;
    m.add_function(wrap_pyfunction!(mesh::export_mesh, m)?)?;
    m.add_function(wrap_pyfunction!(mesh::export_material, m)?)?;
    m.add("LZ4BlockError", m.py().get_type::<LZ4BlockError>())?;
    Ok(())
}
