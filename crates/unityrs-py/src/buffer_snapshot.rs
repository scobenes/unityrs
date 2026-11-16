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
    /// Loaded immutable input is charged by the core's source owner. Only a
    /// mutable snapshot needs an additional payload allocation at this layer.
    pub(crate) fn source(input: &Bound<'_, PyAny>, meter: &Arc<Meter>, cap: u64) -> PyResult<Self> {
        if input.is_exact_instance_of::<PyBytes>() {
            let bytes = input.cast::<PyBytes>()?;
            check_size(bytes.as_bytes().len() as u64, cap)?;
            // Retaining an immutable owner only increments its reference count.
            return Ok(Self {
                bytes: input.extract()?,
                _charge: Charge::zero(meter),
            });
        }
        let charge = meter.charge(256).map_err(errors::charge)?;
        let view = PyMemoryView::from(input)?;
        let size = view.getattr("nbytes")?.extract::<u64>()?;
        check_size(size, cap)?;
        Self::from_view(input, &view, charge)
    }

    pub(crate) fn new(input: &Bound<'_, PyAny>, meter: &Arc<Meter>) -> PyResult<Self> {
        let charge = meter.charge(256).map_err(errors::charge)?;
        let view = PyMemoryView::from(input)?;
        Self::from_view(input, &view, charge)
    }

    /// `BytesIO` writers require C-contiguous storage. Validate before making
    /// an immutable copy, retaining Python's writer-specific exceptions.
    pub(crate) fn writer(input: &Bound<'_, PyAny>, meter: &Arc<Meter>) -> PyResult<Self> {
        let charge = meter.charge(256).map_err(errors::charge)?;
        let view = match PyMemoryView::from(input) {
            Ok(view) if view.getattr("c_contiguous")?.is_truthy()? => view,
            Err(error) if !error.is_instance_of::<pyo3::exceptions::PyTypeError>(input.py()) => {
                return Err(error);
            }
            _ => {
                input
                    .py()
                    .import("io")?
                    .getattr("BytesIO")?
                    .call0()?
                    .call_method1("write", (input,))?;
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "invalid writer buffer",
                ));
            }
        };
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
        // Only exact bytes have no caller-defined finalizer. Normalize buffer
        // subclasses while attached, so their finalizers also run on the
        // caller rather than during a later worker-side Python handoff.
        let bytes = if input.is_exact_instance_of::<PyBytes>() {
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

fn check_size(size: u64, cap: u64) -> PyResult<()> {
    if size > cap {
        return Err(errors::core(
            unityrs_core::LimitError::refused(unityrs_core::LimitKind::InputBytes, size, cap)
                .into(),
        ));
    }
    Ok(())
}

impl AsRef<[u8]> for BufferSnapshot {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl std::ops::Deref for BufferSnapshot {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutable_strided_and_multidimensional_snapshots_are_immutable() -> PyResult<()> {
        let _serial = crate::tests::serial();
        crate::python::relay::attach(|py| {
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
        crate::python::relay::attach(|py| {
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
