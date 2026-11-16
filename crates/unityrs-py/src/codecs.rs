// Ported from UnityPy 1.25.2 (MIT): helpers/CompressionHelper.py:83-105. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Standalone codec adapters over the metered core implementations.
use crate::{errors, python as p};
use pyo3::prelude::*;
use unityrs_core::{
    Bytes, LimitKind,
    budget::DecodeBuf,
    codec::{Codec, Lz4, Lzma, decode_block, encode},
    limits::Charge,
    once::CellError,
};

#[pyfunction]
pub(crate) fn codec(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    which: u32,
    compress: bool,
    decoded: u32,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = crate::exports::context(None);
    let snapshot = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    py.detach(|| {
        crate::worker::run(limits.stack_needed(), || {
            let input = Bytes::from_owner(snapshot);
            if which == 64 {
                let result = if compress {
                    unityrs_core::files::web::compress_brotli(&input, &meter)
                        .map_err(crate::save::error)?
                } else {
                    let context = unityrs_core::FileContext::unitypy();
                    unityrs_core::files::web::decompress_brotli(&input, &meter, &context).map_err(
                        |error| {
                            if error.is_meter_refusal() {
                                errors::core(error)
                            } else {
                                Python::attach(|py| {
                                    let class = py
                                        .import("unityrs.helpers.CompressionHelper")?
                                        .getattr("_BrotliError")?
                                        .cast_into::<pyo3::types::PyType>()?;
                                    Ok::<_, PyErr>(PyErr::from_type(
                                        class,
                                        ("brotli: decoder failed",),
                                    ))
                                })
                                .unwrap_or_else(|error| error)
                            }
                        },
                    )?
                };
                let mut charge = Charge::zero(&meter);
                charge.grow(result.len() as u64).map_err(errors::charge)?;
                return p::bytes(&result);
            }
            let which = Codec::from_flags(which);
            if compress {
                let result = encode::compress(&input, which, &meter).map_err(crate::save::error)?;
                let mut charge = Charge::zero(&meter);
                charge.grow(result.len() as u64).map_err(errors::charge)?;
                return p::bytes(&result);
            }
            let mut out =
                DecodeBuf::new(&meter, 0, limits.max_block_output, LimitKind::BlockOutput)
                    .map_err(errors::charge)?;
            let implementation: &dyn unityrs_core::codec::BlockCodec = match which {
                Codec::Lz4 | Codec::Lz4Hc => &Lz4,
                Codec::Lzma => &Lzma,
                _ => {
                    return Err(pyo3::exceptions::PyNotImplementedError::new_err(
                        "unsupported standalone codec",
                    ));
                }
            };
            let result = decode_block(
                which,
                implementation,
                &input,
                decoded,
                &mut out,
                limits.max_block_output,
            )
            .map_err(|error| match error {
                unityrs_core::Error::Decompress { detail, .. } => {
                    let (name, text) = detail.split_once(": ").unwrap_or(("ValueError", &detail));
                    crate::exports::named_error(name, errors::message(&text))
                }
                error => errors::core(error),
            })?;
            let result = result.as_deref().unwrap_or_else(|| out.as_slice());
            let mut charge = Charge::zero(&meter);
            charge.grow(result.len() as u64).map_err(errors::charge)?;
            p::bytes(result)
        })
    })
}
