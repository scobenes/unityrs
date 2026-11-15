// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! Eager Environment entry points (environment.py:37-73, environment.py:100-160).
use crate::{
    errors,
    files::NativeFile,
    python as p,
    settings::{Budget, Limits},
};
use pyo3::{prelude::*, pybacked::PyBackedBytes};
use std::{path::PathBuf, sync::Arc};
use unityrs_core::{
    FileContext, RawStr,
    env::{EnvOptions, Environment, Source},
};

#[pyclass(frozen, module = "unityrs._native")]
pub(crate) struct NativeEnvironment {
    pub(crate) inner: Environment,
}
#[pymethods]
impl NativeEnvironment {
    #[new]
    #[pyo3(signature = (limits=None, budget=None))]
    fn new(py: Python<'_>, limits: Option<&Limits>, budget: Option<&Budget>) -> Self {
        py.detach(|| {
            let mut context = FileContext::unitypy();
            if let Some(limits) = limits {
                context.limits = limits.inner;
            }
            if let Some(budget) = budget {
                context.budget = budget.inner.clone();
            }
            let mut options = EnvOptions::unitypy();
            options.context = Arc::new(context);
            Self {
                inner: Environment::new(options),
            }
        })
    }
    fn load_bytes(
        &self,
        py: Python<'_>,
        data: PyBackedBytes,
        name: PyBackedBytes,
        dependency: bool,
        stream: bool,
    ) -> PyResult<Option<NativeFile>> {
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || {
                    let ctx = &self.inner.options().context;
                    if data.len() as u64 > ctx.limits.max_input_bytes {
                        return Err(errors::core(
                            unityrs_core::LimitError::refused(
                                unityrs_core::LimitKind::InputBytes,
                                data.len() as u64,
                                ctx.limits.max_input_bytes,
                            )
                            .into(),
                        ));
                    }
                    // Retain the caller's immutable Python buffer without a Rust
                    // input-sized copy or an independent input meter. Core tables and
                    // decoded buffers are all charged to the load's single meter.
                    let reader = if stream {
                        unityrs_core::stream::ReaderKind::Stream
                    } else {
                        unityrs_core::stream::ReaderKind::Memory
                    };
                    let file = self
                        .inner
                        .load_reader(
                            RawStr::from(unityrs_core::Bytes::from_owner(name)),
                            unityrs_core::Bytes::from_owner(data),
                            reader,
                            dependency,
                        )
                        .map_err(errors::core)?;
                    Ok(file.map(NativeFile::new))
                },
            )
        })
    }
    #[pyo3(signature = (path, constructor, dependency, root=None))]
    fn load_path(
        &self,
        py: Python<'_>,
        path: PathBuf,
        constructor: bool,
        dependency: bool,
        root: Option<PathBuf>,
    ) -> PyResult<Option<NativeFile>> {
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || {
                    if constructor {
                        self.inner.load_path(&path).map_err(errors::core)?;
                        Ok(None)
                    } else {
                        let source = if let Some(root) = root {
                            Source::ConfinedPath { path, root }
                        } else {
                            Source::Path(path)
                        };
                        let file = if dependency {
                            self.inner.load_dependency(source).map(Some)
                        } else {
                            self.inner.load_file(source)
                        }
                        .map_err(errors::core)?;
                        Ok(file.map(NativeFile::new))
                    }
                },
            )
        })
    }
    fn load_files(&self, py: Python<'_>, path: PathBuf) -> PyResult<()> {
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || self.inner.load_files([path]).map_err(errors::core),
            )
        })
    }
    fn load_zip_path(&self, py: Python<'_>, path: PathBuf) -> PyResult<()> {
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || {
                    self.inner
                        .load_zip_file(Source::Path(path))
                        .map_err(errors::core)
                },
            )
        })
    }
    fn load_zip_bytes(&self, py: Python<'_>, data: PyBackedBytes) -> PyResult<()> {
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || {
                    self.inner
                        .load_zip_file(Source::Bytes {
                            name: "".into(),
                            data: unityrs_core::Bytes::from_owner(data),
                        })
                        .map_err(errors::core)
                },
            )
        })
    }
    #[allow(clippy::needless_pass_by_value, reason = "PyO3 extracts an owned path")]
    fn load_folder(&self, py: Python<'_>, path: PathBuf) -> PyResult<()> {
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || self.inner.load_folder(&path).map_err(errors::core),
            )
        })
    }
    fn files(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let context = &self.inner.options().context;
            let meter = unityrs_core::limits::Meter::for_input(
                &context.limits,
                context.budget.clone(),
                unityrs_core::budget::Reservation::Input,
            );
            let mut staging = unityrs_core::limits::Charge::zero(&meter);
            staging.grow(64).map_err(errors::charge)?;
            let result = p::list()?;
            for entry in self.inner.files() {
                staging
                    .grow(256 + (entry.name.as_bytes().len() as u64).saturating_mul(4))
                    .map_err(errors::charge)?;
                let row = p::list()?;
                p::push(&row, &p::string(entry.name.as_bytes())?)?;
                let file = Python::attach(|py| {
                    Py::new(py, NativeFile::new(entry.file)).map(Py::into_any)
                })?;
                p::push(&row, &file)?;
                p::push(&row, &p::boolean(entry.is_dependency)?)?;
                p::push(&result, &row)?;
            }
            Ok(result)
        })
    }
    fn cabs(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || {
                    use unityrs_core::{
                        File,
                        env::{Cab, RawCab},
                    };
                    let ctx = &self.inner.options().context;
                    let meter = unityrs_core::limits::Meter::for_input(
                        &ctx.limits,
                        ctx.budget.clone(),
                        unityrs_core::budget::Reservation::Input,
                    );
                    let mut staging = unityrs_core::limits::Charge::zero(&meter);
                    staging.grow(64).map_err(errors::charge)?;
                    let result = p::list()?;
                    for key in self.inner.registry().keys() {
                        staging
                            .grow(256 + (key.as_bytes().len() as u64).saturating_mul(4))
                            .map_err(errors::charge)?;
                        let Some(cab) = self.inner.get_cab(&key, &meter).map_err(errors::core)?
                        else {
                            continue;
                        };
                        let file = match cab {
                            Cab::Serialized(f) => File::Serialized(f),
                            Cab::Raw(RawCab::File(f)) => File::Raw(f),
                            Cab::Raw(RawCab::InBundle { bundle, dir_index }) => {
                                bundle.entry(dir_index).map_err(errors::core)?
                            }
                        };
                        let file = NativeFile::new(file);
                        let row = p::list()?;
                        p::push(&row, &p::string(key.as_bytes())?)?;
                        let file = Python::attach(|py| Py::new(py, file).map(Py::into_any))?;
                        p::push(&row, &file)?;
                        p::push(&result, &row)?;
                    }
                    Ok(result)
                },
            )
        })
    }
}
