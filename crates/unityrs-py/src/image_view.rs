//! A read-only `memoryview` of pixels Rust owns, for Pillow to copy from
//! (`images::_image_frombytes`) without a `bytes` made of them first.
use pyo3::{ffi, prelude::*};

/// `PyBUF_READ` (Include/pybuffer.h; stable ABI since 3.3), which
/// `pyo3::ffi` does not name.
const PYBUF_READ: std::ffi::c_int = 0x100;

/// The view, released before the pixels it shows can be freed.
pub(crate) struct View<'py> {
    view: Bound<'py, PyAny>,
}

impl<'py> View<'py> {
    /// A `memoryview` of `pixels`, read-only (`PyBUF_READ`).
    pub(crate) fn new(py: Python<'py>, pixels: &[u8]) -> PyResult<Self> {
        let len = isize::try_from(pixels.len())
            .map_err(|_| pyo3::exceptions::PyOverflowError::new_err("image too large"))?;
        // SAFETY: `pixels` is valid for `len` bytes, and the view is only
        // read through (`PyBUF_READ`). The caller keeps `pixels` alive and
        // unmoved until `release` answers whether the view still exposes
        // them, and never frees them otherwise.
        let view = unsafe {
            Bound::from_owned_ptr_or_err(
                py,
                ffi::PyMemoryView_FromMemory(pixels.as_ptr().cast_mut().cast(), len, PYBUF_READ),
            )
        }?;
        Ok(Self { view })
    }

    pub(crate) fn as_any(&self) -> &Bound<'py, PyAny> {
        &self.view
    }

    /// `memoryview.release()`: `true` once nothing can reach the pixels
    /// through the view (a released view raises on any use), `false` while
    /// a buffer exported from it is still held, when the pixels must stay.
    pub(crate) fn release(self) -> bool {
        match self.view.call_method0("release") {
            Ok(_) => true,
            Err(error) => {
                drop(error);
                false
            }
        }
    }
}
