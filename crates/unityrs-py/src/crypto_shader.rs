// Ported from UnityPy 1.25.2 (MIT): helpers/ArchiveStorageManager.py:33-37, helpers/ArchiveStorageManager.py:96-107; export/ShaderConverter.py:92-372. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Bounded standalone decryptor and shader stage adapters.
use crate::{errors, python as p};
use pyo3::{exceptions::PyValueError, prelude::*};
use unityrs_core::{
    codec::{
        BlockDecryptor,
        unitycn::{UnityCnDecryptor, decrypt_key_bytes},
    },
    limits::MeteredVec,
};

#[pyfunction]
pub(crate) fn cn_key(
    py: Python<'_>,
    vector: &Bound<'_, PyAny>,
    data: &Bound<'_, PyAny>,
    key: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let (_, meter) = crate::exports::context(None);
    let vector = crate::buffer_snapshot::BufferSnapshot::new(vector, &meter)?;
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    let key = crate::buffer_snapshot::BufferSnapshot::new(key, &meter)?;
    if ![16, 24, 32].contains(&key.len()) {
        return Err(PyValueError::new_err(errors::message(&format_args!(
            "Incorrect AES key length ({} bytes)",
            key.len()
        ))));
    }
    if !vector.len().is_multiple_of(16) {
        return Err(PyValueError::new_err(
            "Data must be aligned to block boundary in ECB mode",
        ));
    }
    py.detach(|| {
        let out = decrypt_key_bytes(&vector, &data, &key, &meter).map_err(errors::core)?;
        let _handoff = meter.charge(out.len() as u64).map_err(errors::charge)?;
        p::bytes(out.as_slice())
    })
}

#[pyfunction]
pub(crate) fn cn_block(
    py: Python<'_>,
    index: [u8; 16],
    substitute: [u8; 16],
    data: &Bound<'_, PyAny>,
    block_index: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    // The accelerated oracle uses PyLong_AsUnsignedLongLongMask.
    let block_index = unsafe { pyo3::ffi::PyLong_AsUnsignedLongLongMask(block_index.as_ptr()) };
    if unsafe { !pyo3::ffi::PyErr_Occurred().is_null() } {
        return Err(PyErr::fetch(py));
    }
    // Only the low byte affects the cipher, including on 32-bit platforms.
    let block_index = (block_index & 255) as usize;
    let (_, meter) = crate::exports::context(None);
    let input = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    py.detach(|| {
        meter
            .count_decode(input.len() as u64)
            .map_err(errors::charge)?;
        let mut out = MeteredVec::new(&meter);
        out.extend_from_slice(&input).map_err(errors::charge)?;
        UnityCnDecryptor::from_tables(index, substitute)
            .decrypt(out.as_mut_slice(), block_index)
            .map_err(errors::core)?;
        let _handoff = meter.charge(out.len() as u64).map_err(errors::charge)?;
        p::bytes(out.as_slice())
    })
}

#[pyfunction]
pub(crate) fn shader_stage(
    py: Python<'_>,
    input: &Bound<'_, PyAny>,
    stage: &str,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = crate::exports::context(None);
    let snapshot = crate::value_input::shader_snapshot(input.as_unbound(), &meter, &limits)?;
    py.detach(|| {
        crate::worker::run(limits.stack_needed(), || {
            let parsed = snapshot.build()?;
            let bytes = unityrs_export::shader::direct_stage(&parsed, stage, &meter)
                .map_err(|error| crate::exports::shader_error(error, &meter))?;
            let _handoff = meter
                .charge((bytes.len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            p::string(bytes.as_slice())
        })
    })?
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(cn_key, m)?)?;
    m.add_function(wrap_pyfunction!(cn_block, m)?)?;
    m.add_function(wrap_pyfunction!(shader_stage, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyBytes;

    #[test]
    fn standalone_aes_known_vector_and_mutated_table_bytes() -> PyResult<()> {
        let _serial = crate::tests::serial();
        crate::python::relay::attach(|py| {
            let vector = PyBytes::new(
                py,
                &[
                    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
                    0xdd, 0xee, 0xff,
                ],
            );
            let key = PyBytes::new(py, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
            let data = PyBytes::new(py, &[0; 16]);
            let got = cn_key(py, vector.as_any(), data.as_any(), key.as_any())?;
            assert_eq!(
                got.bind(py).extract::<Vec<u8>>()?,
                [
                    0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70,
                    0xb4, 0xc5, 0x5a,
                ]
            );
            let block = PyBytes::new(py, &[0]);
            let got = cn_block(
                py,
                [255; 16],
                [255; 16],
                block.as_any(),
                p::uint(0)?.bind(py),
            )?;
            assert_eq!(got.bind(py).extract::<Vec<u8>>()?, [0x33]);
            Ok(())
        })
    }

    #[test]
    fn standalone_key_refusal_releases_output_charge() {
        use std::sync::Arc;
        use unityrs_core::{
            budget::{Counting, Reservation},
            limits::Meter,
        };
        let budget = Arc::new(Counting::new(1));
        let meter = Meter::new(1024, budget.clone(), Reservation::Input);
        let got = decrypt_key_bytes(&[0; 16], &[0; 16], &[0; 16], &meter);
        assert!(got.is_err());
        assert_eq!(budget.used(), 0);
        assert_eq!(meter.used(), 0);
    }
}
