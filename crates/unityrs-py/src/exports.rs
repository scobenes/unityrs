// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/AudioClipConverter.py, UnityPy/export/ShaderConverter.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Current-value exporters; no operation rereads the original object payload.
use crate::{environment::NativeEnvironment, errors, object::NativeObject, python as p};
use pyo3::{
    exceptions::{PyException, PyNotImplementedError, PyValueError},
    prelude::*,
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
#[pyo3(signature = (data, name, reader=None, vorbis_headers=None, vorbis_path=None, convert_pcm_float=true))]
fn export_audio(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    name: &Bound<'_, PyAny>,
    reader: Option<&NativeObject>,
    vorbis_headers: Option<&Bound<'_, PyAny>>,
    vorbis_path: Option<&Bound<'_, PyAny>>,
    convert_pcm_float: bool,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = context(reader);
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    let name = crate::buffer_snapshot::BufferSnapshot::new(name, &meter)?;
    let vorbis_headers = vorbis_headers
        .map(|v| crate::buffer_snapshot::BufferSnapshot::new(v, &meter))
        .transpose()?;
    let path = vorbis_path
        .map(|path| crate::path_snapshot::snapshot(path, &meter))
        .transpose()?;
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            // export/AudioClipConverter.py:13-49. The facade handles current
            // inline/resource selection before this magic dispatch and decoder.
            let plan =
                unityrs_audio::plan(&data, convert_pcm_float, &meter).map_err(audio_error)?;
            let headers = if plan.header().is_some_and(|header| header.mode == 15) {
                if let Some(bytes) = vorbis_headers.as_ref() {
                    Some(
                        unityrs_audio::VorbisHeaders::from_bytes(bytes, &meter)
                            .map_err(audio_error)?,
                    )
                } else if let Some((path, _charge)) = path.as_ref() {
                    Some(unityrs_audio::VorbisHeaders::load(path, &meter).map_err(audio_error)?)
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
    input: &Bound<'_, PyAny>,
    version: (u32, u32),
    reader: Option<&NativeObject>,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = context(reader);
    let snapshot = crate::value_input::export_snapshot(input.as_unbound(), &meter, &limits)?;
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            let parsed = snapshot.build()?;
            let bytes = unityrs_export::shader::export_with_cap(
                &parsed,
                version,
                &meter,
                limits.max_block_output,
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyBytes, PyDict};

    #[test]
    fn raw_float_wav_bytes_and_default_conversion() -> PyResult<()> {
        let _serial = crate::tests::serial();
        Python::attach(|py| {
            let module = PyModule::new(py, "export_adapter_test")?;
            register(&module)?;
            let scope = PyDict::new(py);
            scope.set_item("native", &module)?;
            let data = py.eval(
                c"b'FSB5' + __import__('struct').pack('<6I', 1, 1, 8, 0, 8, 5) + bytes(32) + __import__('struct').pack('<Q', (8 << 1) | (2 << 34)) + __import__('struct').pack('<2f', .5, -.5)",
                None,
                None,
            )?;
            scope.set_item("data", data)?;
            let raw = py.eval(
                c"native.export_audio(data, b'tone', convert_pcm_float=False)['tone.wav']",
                Some(&scope),
                None,
            )?;
            let converted = py.eval(
                c"native.export_audio(data, b'tone')['tone.wav']",
                Some(&scope),
                None,
            )?;
            let raw = raw.cast::<PyBytes>()?.as_bytes();
            let converted = converted.cast::<PyBytes>()?.as_bytes();
            assert_eq!(raw.get(20..24), Some([3, 0, 1, 0].as_slice()));
            assert_eq!(raw.get(34..36), Some([32, 0].as_slice()));
            assert_eq!(raw.get(44..), Some([0, 0, 0, 63, 0, 0, 0, 191].as_slice()));
            assert_eq!(converted.get(20..24), Some([1, 0, 1, 0].as_slice()));
            assert_eq!(converted.get(34..36), Some([16, 0].as_slice()));
            assert_eq!(converted.get(44..), Some([0, 64, 0, 192].as_slice()));
            Ok(())
        })
    }
}
