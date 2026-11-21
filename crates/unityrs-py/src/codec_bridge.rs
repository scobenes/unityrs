// Ported from UnityPy 1.25.2 (MIT): helpers/CompressionHelper.py:158-231, helpers/CompressionHelper.py:238-258, files/BundleFile.py:489-521. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Live codec-map lookups relayed to the calling Python thread, without core locks.
use crate::direct_keys::name;
use crate::{
    buffer_snapshot::BufferSnapshot,
    errors,
    python::{
        self as p,
        relay::{self, Request, Response},
    },
};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    Bytes, Error,
    budget::DecodeBuf,
    codec::{BlockCodec, Codec, Codecs, Decoded},
    files::SaveError,
    limits::{Charge, Meter},
};

pub(crate) struct Call {
    kind: u8,
    which: Codec,
    flags: u32,
    data: Bytes,
    size: u32,
    meter: Arc<Meter>,
    selected: Option<Py<PyAny>>,
}

pub(crate) fn invoke(call: &Call) -> PyResult<Response> {
    crate::python::relay::attach(|py| {
        // Every name interned (`name!`) and every call's arguments made
        // fallibly: a failed allocation raises, never panics.
        let module = PyModule::import(py, name!(py, c"unityrs.helpers.CompressionHelper")?)?;
        let id = call.which.id();
        if call.kind == 2 {
            return encoder(py, &module, id);
        }
        let (map_name, defaults) = if call.kind == 0 {
            (
                name!(py, c"DECOMPRESSION_MAP")?,
                name!(py, c"_DEFAULT_DECOMPRESSION_MAP")?,
            )
        } else {
            (
                name!(py, c"COMPRESSION_MAP")?,
                name!(py, c"_DEFAULT_COMPRESSION_MAP")?,
            )
        };
        let map = if call.selected.is_none() {
            Some(module.getattr(map_name)?)
        } else {
            None
        };
        if let Some(map) = &map
            && !map.contains(id)?
        {
            return if call.kind == 0 {
                Err(crate::errors::new::<pyo3::exceptions::PyValueError>(
                    errors::message(&format_args!(
                        "Unknown compression! flag: {}, compression flag: {id}",
                        call.flags
                    )),
                ))
            } else {
                Err(
                    crate::errors::new::<pyo3::exceptions::PyNotImplementedError>(errors::message(
                        &format_args!(
                            "No compression function in the CompressionHelper.COMPRESSION_MAP for {id}"
                        ),
                    )),
                )
            };
        }
        let selected = match (&call.selected, map) {
            (Some(selected), _) => selected.bind(py).clone(),
            (_, Some(map)) => map.get_item(id)?,
            _ => {
                return Err(crate::errors::new::<pyo3::exceptions::PyRuntimeError>(
                    "missing codec selection",
                ));
            }
        };
        if call.selected.is_none()
            && module
                .getattr(defaults)?
                .get_item(id)
                .is_ok_and(|default| default.is(&selected))
            && id < 4
        {
            return Ok(Response::Codec(None));
        }
        let mut staging = Charge::zero(&call.meter);
        staging
            .grow(256 + call.data.len() as u64)
            .map_err(errors::charge)?;
        let input = p::bytes(&call.data)?;
        let result = if call.kind == 0 {
            p::held::call(
                &selected,
                &[input.into_bound(py), p::held::uint(py, call.size.into())?],
            )?
        } else {
            p::held::call1(&selected, input.bind(py))?
        };
        Ok(Response::Codec(Some(BufferSnapshot::new(
            &result,
            &call.meter,
        )?)))
    })
}

/// The encoder for `id` and its chunk size (`call.kind == 2`).
fn encoder(py: Python<'_>, module: &Bound<'_, PyModule>, id: u8) -> PyResult<Response> {
    // Validate the compressor before the chunk map, as UnityPy does.
    if !module
        .getattr(name!(py, c"COMPRESSION_MAP")?)?
        .contains(id)?
    {
        return Err(
            crate::errors::new::<pyo3::exceptions::PyNotImplementedError>(errors::message(
                &format_args!(
                    "No compression function in the CompressionHelper.COMPRESSION_MAP for {id}"
                ),
            )),
        );
    }
    let map = module.getattr(name!(py, c"COMPRESSION_CHUNK_SIZE_MAP")?)?;
    if !map.contains(id)? {
        return Err(
            crate::errors::new::<pyo3::exceptions::PyNotImplementedError>(errors::message(
                &format_args!(
                    "No chunk size in the CompressionHelper.COMPRESSION_CHUNK_SIZE_MAP for {id}"
                ),
            )),
        );
    }
    let size = map.get_item(id)?.extract::<usize>()?;
    if size == 0 {
        return Err(crate::errors::new::<pyo3::exceptions::PyValueError>(
            "codec chunk size must be positive",
        ));
    }
    let selected = module
        .getattr(name!(py, c"COMPRESSION_MAP")?)?
        .get_item(id)?;
    let builtin = id < 4
        && module
            .getattr(name!(py, c"_DEFAULT_COMPRESSION_MAP")?)?
            .get_item(id)
            .is_ok_and(|default| default.is(&selected));
    Ok(Response::Encoder(selected.unbind(), size, builtin))
}

struct Bridge(Codec);

fn failure(error: PyErr) -> Error {
    relay::fail(error);
    Error::Malformed {
        what: "Python codec callback",
        detail: "callback failed".into(),
    }
}

fn request(
    kind: u8,
    which: Codec,
    flags: u32,
    data: Bytes,
    size: u32,
    meter: &Arc<Meter>,
) -> PyResult<Response> {
    relay::request(Request::Codec(Call {
        kind,
        which,
        flags,
        data,
        size,
        meter: meter.clone(),
        selected: None,
    }))
}

impl BlockCodec for Bridge {
    fn decode(&self, input: &Bytes, size: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
        self.decode_with_flags(input, size, u32::from(self.0.id()), out)
    }

    fn decode_with_flags(
        &self,
        input: &Bytes,
        size: u32,
        flags: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        match request(0, self.0, flags, input.clone(), size, out.meter()).map_err(failure)? {
            Response::Codec(Some(bytes)) => {
                out.extend_from_slice(&bytes)?;
                Ok(Decoded::Written)
            }
            Response::Codec(None) => match self.0 {
                Codec::None => unityrs_core::codec::Stored.decode(input, size, out),
                Codec::Lzma => unityrs_core::codec::Lzma.decode(input, size, out),
                _ => unityrs_core::codec::Lz4.decode(input, size, out),
            },
            _ => Err(failure(crate::errors::new::<
                pyo3::exceptions::PyRuntimeError,
            >("invalid codec response"))),
        }
    }

    fn compress(&self, data: &[u8], which: Codec, meter: &Arc<Meter>) -> Result<Bytes, SaveError> {
        let mut input = DecodeBuf::new(meter, 0, u64::MAX, unityrs_core::LimitKind::InputMemory)?;
        input.extend_from_slice(data)?;
        let input = input.into_bytes()?;
        match request(1, which, u32::from(which.id()), input, 0, meter)
            .map_err(|error| SaveError::Read(failure(error)))?
        {
            Response::Codec(Some(bytes)) => {
                let mut out =
                    DecodeBuf::new(meter, 0, u64::MAX, unityrs_core::LimitKind::InputMemory)?;
                out.extend_from_slice(&bytes)?;
                Ok(out.into_bytes()?)
            }
            Response::Codec(None) => unityrs_core::codec::encode::compress(data, which, meter),
            _ => Err(SaveError::Read(failure(crate::errors::new::<
                pyo3::exceptions::PyRuntimeError,
            >(
                "invalid compressor response"
            )))),
        }
    }

    fn compressor(
        &self,
        which: Codec,
        meter: &Arc<Meter>,
    ) -> Result<Option<Arc<dyn BlockCodec>>, SaveError> {
        match request(2, which, u32::from(which.id()), Bytes::new(), 0, meter)
            .map_err(|error| SaveError::Read(failure(error)))?
        {
            Response::Encoder(selected, chunk, builtin) => Ok(Some(Arc::new(Captured {
                selected: Some(selected),
                chunk,
                builtin,
            }))),
            _ => Err(SaveError::Read(failure(crate::errors::new::<
                pyo3::exceptions::PyRuntimeError,
            >(
                "invalid chunk response"
            )))),
        }
    }
}

struct Captured {
    selected: Option<Py<PyAny>>,
    chunk: usize,
    builtin: bool,
}

impl Drop for Captured {
    fn drop(&mut self) {
        // A callback object may have a Python finalizer. Release its last
        // reference on the caller as well, outside all native locks.
        if let Some(value) = self.selected.take() {
            let _ = relay::request(Request::Release(value));
        }
    }
}

impl BlockCodec for Captured {
    fn decode(&self, _: &Bytes, _: u32, _: &mut DecodeBuf) -> Result<Decoded, Error> {
        Err(Error::Malformed {
            what: "encoder",
            detail: "encoder cannot decode".into(),
        })
    }
    fn chunk_size(&self, _: Codec, _: &Arc<Meter>) -> Result<usize, SaveError> {
        Ok(self.chunk)
    }
    fn compress(&self, data: &[u8], which: Codec, meter: &Arc<Meter>) -> Result<Bytes, SaveError> {
        if self.builtin {
            return unityrs_core::codec::encode::compress(data, which, meter);
        }
        let mut input = DecodeBuf::new(meter, 0, u64::MAX, unityrs_core::LimitKind::InputMemory)?;
        input.extend_from_slice(data)?;
        let result = relay::request(Request::Codec(Call {
            kind: 1,
            which,
            flags: u32::from(which.id()),
            data: input.into_bytes()?,
            size: 0,
            meter: meter.clone(),
            selected: self
                .selected
                .as_ref()
                .map(|value| crate::python::relay::attach(|py| value.clone_ref(py))),
        }))
        .map_err(|error| SaveError::Read(failure(error)))?;
        match result {
            Response::Codec(Some(bytes)) => {
                let mut out =
                    DecodeBuf::new(meter, 0, u64::MAX, unityrs_core::LimitKind::InputMemory)?;
                out.extend_from_slice(&bytes)?;
                Ok(out.into_bytes()?)
            }
            _ => Err(SaveError::Read(failure(crate::errors::new::<
                pyo3::exceptions::PyRuntimeError,
            >(
                "invalid captured compressor response",
            )))),
        }
    }
}

pub(crate) fn registry() -> Arc<Codecs> {
    let mut registry = Codecs::empty();
    // A fixed 64-slot bridge; no input-dependent allocation during setup.
    for id in 0..64 {
        let which = Codec::from_flags(id);
        let _ = registry.set(which, Arc::new(Bridge(which)));
    }
    Arc::new(registry)
}
