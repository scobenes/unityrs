// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py, UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! Runtime-only TPK data (helpers/TypeTreeNode.py:308-328).
use crate::{errors, python as p, traversal::TreeTraversal};
use pyo3::{exceptions::PyMemoryError, prelude::*};
use std::sync::Arc;
use unityrs_core::provider::{ProviderChain, TypeTreeProvider};
use unityrs_tpk::TpkProvider;

#[pyclass(frozen, module = "unityrs._native")]
pub(crate) struct NativeTpk {
    provider: Arc<TpkProvider>,
}

impl NativeTpk {
    pub(crate) fn chain(&self) -> PyResult<Arc<ProviderChain>> {
        let mut providers: Vec<Arc<dyn TypeTreeProvider>> = Vec::new();
        providers
            .try_reserve_exact(1)
            .map_err(|_| PyMemoryError::new_err("provider allocation refused"))?;
        providers.push(self.provider.clone());
        Ok(Arc::new(ProviderChain::new(providers)))
    }
}

#[pymethods]
impl NativeTpk {
    #[new]
    fn new(py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<Self> {
        let ctx = unityrs_core::FileContext::unitypy();
        let meter = unityrs_core::limits::Meter::for_input(
            &ctx.limits,
            ctx.budget,
            unityrs_core::budget::Reservation::Input,
        );
        let (path, _charge) = crate::path_snapshot::snapshot(path, &meter)?;
        py.detach(|| {
            let provider = TpkProvider::open(path)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(errors::message(&e)))?;
            Ok(Self {
                provider: Arc::new(provider),
            })
        })
    }

    #[pyo3(signature = (traversal=None))]
    fn common_strings(
        &self,
        py: Python<'_>,
        traversal: Option<Py<TreeTraversal>>,
    ) -> PyResult<Py<PyAny>> {
        let traversal = TreeTraversal::provided(py, traversal)?;
        py.detach(|| {
            let out = p::dict()?;
            if let Some(strings) = self.provider.common_strings() {
                for (offset, text) in strings.iter() {
                    let bytes = text.len() as u64;
                    TreeTraversal::shared_step(
                        &traversal,
                        512_u64.saturating_add(bytes.saturating_mul(4)),
                        1_u64.saturating_add(bytes),
                    )?;
                    p::set(&out, &p::uint(offset.into())?, &p::string(text.as_bytes())?)?;
                }
            }
            Ok(out)
        })
    }

    #[pyo3(signature = (traversal=None))]
    fn common_string_versions(
        &self,
        py: Python<'_>,
        traversal: Option<Py<TreeTraversal>>,
    ) -> PyResult<Py<PyAny>> {
        let traversal = TreeTraversal::provided(py, traversal)?;
        py.detach(|| {
            let out = p::list()?;
            for &(version, count) in self.provider.common_string_versions() {
                TreeTraversal::shared_step(&traversal, 512, 1)?;
                let pair = p::list()?;
                p::push(&pair, &p::uint(version)?)?;
                p::push(&pair, &p::uint(count.into())?)?;
                p::push(&out, &p::tuple(&pair)?)?;
            }
            Ok(out)
        })
    }
}

#[pyfunction]
#[pyo3(signature = (traversal=None))]
pub(crate) fn mit_common_strings(
    py: Python<'_>,
    traversal: Option<Py<TreeTraversal>>,
) -> PyResult<Py<PyAny>> {
    let traversal = TreeTraversal::provided(py, traversal)?;
    py.detach(|| {
        let out = p::dict()?;
        for &(offset, text) in unityrs_core::typetree::commonstring::COMMON_STRINGS {
            let bytes = text.len() as u64;
            TreeTraversal::shared_step(
                &traversal,
                512_u64.saturating_add(bytes.saturating_mul(4)),
                1_u64.saturating_add(bytes),
            )?;
            p::set(&out, &p::uint(offset.into())?, &p::string(text)?)?;
        }
        Ok(out)
    })
}
