// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! Reuse the core's metered LZ4 decoder for caller-supplied generator TPKs.
use crate::{errors, python as p};
use pyo3::{prelude::*, pybacked::PyBackedBytes};
use unityrs_core::{
    Bytes, LimitKind,
    budget::DecodeBuf,
    codec::{Codec, Lz4, decode_block},
    limits::{Charge, Meter},
};

#[pyfunction]
pub(crate) fn class_tpk_lz4(
    py: Python<'_>,
    data: PyBackedBytes,
    decoded: u32,
) -> PyResult<Py<PyAny>> {
    py.detach(|| {
        const CAP: u64 = 256 << 20;
        if data.len() as u64 > CAP || u64::from(decoded) > CAP {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "TPK exceeds 256 MiB",
            ));
        }
        let meter = Meter::unlimited();
        let input = Bytes::from_owner(data);
        let mut out =
            DecodeBuf::new(&meter, 0, CAP, LimitKind::BlockOutput).map_err(errors::charge)?;
        let result =
            decode_block(Codec::Lz4, &Lz4, &input, decoded, &mut out, CAP).map_err(errors::core)?;
        let bytes = result.as_deref().unwrap_or_else(|| out.as_slice());
        let mut charge = Charge::zero(&meter);
        charge.grow(bytes.len() as u64).map_err(errors::charge)?;
        p::bytes(bytes)
    })
}
