// Ported from UnityPy 1.25.2 (MIT): helpers/CompressionHelper.py:83-105. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Standalone codec adapters over the metered core implementations.
use crate::worker_error::Failure;
use crate::{errors, python as p};
use pyo3::prelude::*;
use unityrs_core::{
    Bytes, LimitKind,
    budget::DecodeBuf,
    codec::{Codec, Lz4, Lzma, decode_block, encode},
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
    let native = py.detach(|| {
        crate::worker::run(limits.stack_needed(), || {
            let input = Bytes::from_owner(snapshot);
            if which == 64 {
                let result = if compress {
                    unityrs_core::files::web::compress_brotli(&input, &meter)
                        .map_err(Failure::save)?
                } else {
                    let context = unityrs_core::FileContext::unitypy();
                    unityrs_core::files::web::decompress_brotli(&input, &meter, &context).map_err(
                        |error| {
                            if error.is_meter_refusal() {
                                Failure::core(error)
                            } else {
                                Failure::Static("brotli.error", "brotli: decoder failed")
                            }
                        },
                    )?
                };
                return Ok(result);
            }
            let which = Codec::from_flags(which);
            if compress {
                let result = encode::compress(&input, which, &meter).map_err(Failure::save)?;
                return Ok(result);
            }
            let mut out =
                DecodeBuf::new(&meter, 0, limits.max_block_output, LimitKind::BlockOutput)
                    .map_err(Failure::charge)?;
            let implementation: &dyn unityrs_core::codec::BlockCodec = match which {
                Codec::Lz4 | Codec::Lz4Hc => &Lz4,
                Codec::Lzma => &Lzma,
                _ => {
                    return Err(Failure::Static(
                        "NotImplementedError",
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
            .map_err(Failure::core)?;
            result
                .map_or_else(|| out.into_bytes(), Ok)
                .map_err(Failure::charge)
        })
    })?;
    let native = native.map_err(|error| match error {
        Failure::Core(unityrs_core::Error::Decompress { detail, .. }) => {
            let (name, text) = detail.split_once(": ").unwrap_or(("ValueError", &detail));
            crate::exports::named_error(name, errors::message(&text))
        }
        error => error.python(),
    })?;

    let _copy = meter.charge(native.len() as u64).map_err(errors::charge)?;
    p::bytes(&native)
}
