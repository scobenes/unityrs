//! Charged immutable buffer snapshots, read on the calling Python thread.
use crate::errors;
use pyo3::{
    prelude::*,
    pybacked::PyBackedBytes,
    types::{PyBytes, PyMemoryView},
};
use std::sync::Arc;
use unityrs_core::limits::{Charge, Meter};

pub(crate) struct BufferSnapshot {
    bytes: PyBackedBytes,
    _charge: Charge,
}

impl BufferSnapshot {
    pub(crate) fn new(input: &Bound<'_, PyAny>, meter: &Arc<Meter>) -> PyResult<Self> {
        let charge = meter.charge(256).map_err(errors::charge)?;
        let view = PyMemoryView::from(input)?;
        Self::from_view(input, &view, charge)
    }

    pub(crate) fn from_view(
        input: &Bound<'_, PyAny>,
        view: &Bound<'_, PyMemoryView>,
        mut charge: Charge,
    ) -> PyResult<Self> {
        let meter = charge.meter().clone();
        let size = view.getattr("nbytes")?.extract::<u64>()?;
        charge.grow(size).map_err(errors::charge)?;
        // Retain immutable bytes directly; every mutable or indirect buffer is
        // copied while attached. No Python storage is read on the worker.
        let bytes = if input.is_instance_of::<PyBytes>() {
            input.extract()?
        } else {
            meter.count_decode(size).map_err(errors::charge)?;
            view.call_method0("tobytes")?.extract()?
        };
        Ok(Self {
            bytes,
            _charge: charge,
        })
    }
}

impl AsRef<[u8]> for BufferSnapshot {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutable_strided_and_multidimensional_snapshots_are_immutable() -> PyResult<()> {
        let _serial = crate::tests::serial();
        Python::attach(|py| {
            for expression in [
                c"memoryview(data)",
                c"memoryview(data)[::2]",
                c"memoryview(data).cast('B', (1, 6))",
            ] {
                let scope = pyo3::types::PyDict::new(py);
                let source = pyo3::types::PyByteArray::new(py, b"abcdef");
                scope.set_item("data", &source)?;
                let input = py.eval(expression, Some(&scope), None)?;
                let meter = Meter::unlimited();
                let snapshot = BufferSnapshot::new(&input, &meter)?;
                let expected = input.call_method0("tobytes")?.extract::<Vec<u8>>()?;
                source.set_item(0, 255)?;
                assert_eq!(snapshot.as_ref(), expected);
                assert!(meter.used() >= expected.len() as u64);
                drop(snapshot);
                assert_eq!(meter.used(), 0);
            }
            Ok(())
        })
    }

    #[test]
    fn snapshot_refuses_nbytes_before_copying_and_releases_charge() -> PyResult<()> {
        let _serial = crate::tests::serial();
        Python::attach(|py| {
            let input = py.eval(
                c"memoryview(bytearray(2_000_000)).cast('B', (1, 2_000_000))",
                None,
                None,
            )?;
            let meter = Meter::new(
                1_000_000,
                Arc::new(unityrs_core::budget::Unbounded),
                unityrs_core::budget::Reservation::Input,
            );
            let result = BufferSnapshot::new(&input, &meter);
            assert!(
                matches!(result, Err(error) if error.is_instance_of::<pyo3::exceptions::PyMemoryError>(py))
            );
            assert_eq!(meter.used(), 0);
            assert_eq!(meter.decoded(), 0);
            Ok(())
        })
    }
}
