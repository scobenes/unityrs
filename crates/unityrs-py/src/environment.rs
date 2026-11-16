// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! Eager Environment entry points (environment.py:37-73, environment.py:100-160).
use crate::{
    errors,
    files::NativeFile,
    python as p,
    settings::{Budget, Limits},
};
use pyo3::prelude::*;
use std::sync::{Arc, Mutex};
use unityrs_core::{
    FileContext, RawStr,
    env::{EnvOptions, Environment, Source},
};

#[pyclass(frozen, module = "unityrs._native")]
pub(crate) struct NativeEnvironment {
    pub(crate) inner: Environment,
    config: Arc<unityrs_core::config::RuntimeConfig>,
    fallback_warnings: Arc<FallbackWarning>,
}

#[derive(Debug, Default)]
struct FallbackWarning {
    error: Mutex<Option<PyErr>>,
    config: Arc<unityrs_core::config::RuntimeConfig>,
}
impl unityrs_core::config::FallbackWarningSink for FallbackWarning {
    fn warn(&self, value: &RawStr) -> Result<(), unityrs_core::limits::ChargeError> {
        self.warn_value(value).map(|_| ())
    }

    fn warn_value(&self, value: &RawStr) -> Result<RawStr, unityrs_core::limits::ChargeError> {
        self.warn_value_at(value, false)
    }

    fn warn_value_at(
        &self,
        value: &RawStr,
        bundle: bool,
    ) -> Result<RawStr, unityrs_core::limits::ChargeError> {
        // Version resolution runs before registry publication and outside
        // core locks; lazy ResultCell computations likewise run unlocked.
        // Relay onto the original caller thread so its Python RLock remains
        // reentrant. Never hold our pending-error mutex during the warning.
        let result = crate::worker::python_warning(value, bundle);
        match result {
            Ok((value, parse_typetree)) => {
                self.config.set(Some(value.clone()), parse_typetree);
                Ok(value)
            }
            Err(error) => {
                *self
                    .error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                // Private transient sentinel: stop parsing before publication.
                // config._load's finally retrieves the original Python exception.
                Err(unityrs_core::budget::AllocError {
                    requested: 0,
                    what: unityrs_core::budget::Reservation::Input,
                }
                .into())
            }
        }
    }
}
impl NativeEnvironment {
    fn input_meter(&self) -> Arc<unityrs_core::limits::Meter> {
        let ctx = &self.inner.options().context;
        unityrs_core::limits::Meter::for_input(
            &ctx.limits,
            ctx.budget.clone(),
            unityrs_core::budget::Reservation::Input,
        )
    }
}
#[pymethods]
impl NativeEnvironment {
    fn resource(
        &self,
        py: Python<'_>,
        file: &NativeFile,
        path: &Bound<'_, PyAny>,
        offset: i128,
        size: i128,
    ) -> PyResult<Py<PyAny>> {
        let unityrs_core::File::Serialized(file) = &file.inner else {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "resource owner is not a SerializedFile",
            ));
        };
        let path = crate::buffer_snapshot::BufferSnapshot::new(path, file.meter())?;
        py.detach(move || {
            crate::worker::run(crate::worker::load_stack(&file.context().limits), || {
                let path = RawStr::charged(&path, file.meter()).map_err(errors::charge)?;
                let stream = unityrs_texture::fetch::Stream { path, offset, size };
                let (offset, size, slice) = match stream.span() {
                    unityrs_texture::fetch::Span::Range(offset, size) => (offset, size, None),
                    unityrs_texture::fetch::Span::Slice(start, stop) => {
                        (0, u64::MAX, Some((start, stop)))
                    }
                };
                let bytes = self
                    .inner
                    .resource(file, &stream.path, offset, size)
                    .map_err(|error| errors::resource(error, &stream.path))?;
                let bytes = if let Some((start, stop)) = slice {
                    bytes.slice(unityrs_texture::fetch::python_slice(
                        bytes.len(),
                        start,
                        stop,
                    ))
                } else {
                    bytes
                };
                let _copy = file
                    .meter()
                    .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                    .map_err(errors::charge)?;
                p::bytes(&bytes)
            })
        })
    }
    #[new]
    #[pyo3(signature = (limits=None, budget=None, tpk=None))]
    fn new(
        py: Python<'_>,
        limits: Option<&Limits>,
        budget: Option<&Budget>,
        tpk: Option<&crate::tpk::NativeTpk>,
    ) -> PyResult<Self> {
        py.detach(|| {
            let mut context = FileContext::unitypy();
            let config = Arc::new(unityrs_core::config::RuntimeConfig::unitypy());
            let fallback_warnings = Arc::new(FallbackWarning {
                error: Mutex::new(None),
                config: config.clone(),
            });
            context.runtime_config = Some(config.clone());
            context.version.fallback_warnings = Some(fallback_warnings.clone());
            if let Some(tpk) = tpk {
                context.providers = tpk.chain()?;
            }
            if let Some(limits) = limits {
                context.limits = limits.inner;
            }
            if let Some(budget) = budget {
                context.budget = budget.inner.clone();
            }
            let mut options = EnvOptions::unitypy();
            options.context = Arc::new(context);
            Ok(Self {
                inner: Environment::new(options),
                config,
                fallback_warnings,
            })
        })
    }
    #[pyo3(signature = (fallback, parse_typetree, decrypt_key=None))]
    fn configure(
        &self,
        py: Python<'_>,
        fallback: Option<&Bound<'_, PyAny>>,
        parse_typetree: bool,
        decrypt_key: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let meter = self.input_meter();
        let fallback = fallback
            .map(|v| crate::buffer_snapshot::BufferSnapshot::source(v, &meter, u64::MAX))
            .transpose()?;
        let decrypt_key = decrypt_key
            .map(|value| {
                let snapshot = crate::buffer_snapshot::BufferSnapshot::new(value, &meter)?;
                <[u8; 16]>::try_from(snapshot.as_ref()).map_err(|_| {
                    pyo3::exceptions::PyValueError::new_err("AssetBundle Key length is wrong")
                })
            })
            .transpose()?;
        py.detach(|| {
            self.config.set_decrypt_key(decrypt_key);
            self.config.set(
                fallback.map(|data| RawStr::from(unityrs_core::Bytes::from_owner(data))),
                parse_typetree,
            );
        });
        Ok(())
    }
    fn take_fallback_warnings(&self, py: Python<'_>) -> PyResult<u64> {
        let error = py.detach(|| {
            self.fallback_warnings
                .error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        });
        error.map_or(Ok(0), Err)
    }
    fn load_bytes(
        &self,
        py: Python<'_>,
        data: &Bound<'_, PyAny>,
        name: &Bound<'_, PyAny>,
        dependency: bool,
        stream: bool,
    ) -> PyResult<Option<NativeFile>> {
        let meter = self.input_meter();
        let data = crate::buffer_snapshot::BufferSnapshot::source(
            data,
            &meter,
            self.inner.options().context.limits.max_input_bytes,
        )?;
        let name = crate::buffer_snapshot::BufferSnapshot::source(name, &meter, u64::MAX)?;
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
        path: &Bound<'_, PyAny>,
        constructor: bool,
        dependency: bool,
        root: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Option<NativeFile>> {
        let meter = self.input_meter();
        let (path, _path_charge) = crate::path_snapshot::snapshot(path, &meter)?;
        let (root, _root_charge) = root
            .map(|root| crate::path_snapshot::snapshot(root, &meter))
            .transpose()?
            .unzip();
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
    fn load_files(&self, py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<()> {
        let (path, _charge) = crate::path_snapshot::snapshot(path, &self.input_meter())?;
        py.detach(|| {
            crate::worker::run(
                crate::worker::load_stack(&self.inner.options().context.limits),
                || self.inner.load_files([path]).map_err(errors::core),
            )
        })
    }
    fn load_zip_path(&self, py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<()> {
        let (path, _charge) = crate::path_snapshot::snapshot(path, &self.input_meter())?;
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
    fn load_zip_bytes(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let meter = self.input_meter();
        let data = crate::buffer_snapshot::BufferSnapshot::source(
            data,
            &meter,
            self.inner.options().context.limits.max_input_bytes,
        )?;
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
    fn load_folder(&self, py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<()> {
        let (path, _charge) = crate::path_snapshot::snapshot(path, &self.input_meter())?;
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
