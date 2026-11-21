// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! Reuse the core's metered LZ4 decoder for caller-supplied generator TPKs.
use crate::{errors, python as p};
use pyo3::prelude::*;
use unityrs_core::{
    Bytes, LimitKind,
    budget::DecodeBuf,
    codec::{Codec, Lz4, decode_block},
    limits::{Charge, Meter},
};

#[pyfunction]
pub(crate) fn class_tpk_lz4(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    decoded: u32,
) -> PyResult<Py<PyAny>> {
    const CAP: u64 = 256 << 20;
    let meter = Meter::unlimited();
    let _view_charge = meter.charge(256).map_err(errors::charge)?;
    let view = pyo3::types::PyMemoryView::from(data)?;
    if view
        .getattr(crate::direct_keys::name!(py, c"nbytes")?)?
        .extract::<u64>()?
        > CAP
        || u64::from(decoded) > CAP
    {
        return Err(crate::errors::new::<pyo3::exceptions::PyValueError>(
            "TPK exceeds 256 MiB",
        ));
    }
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    py.detach(|| {
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
