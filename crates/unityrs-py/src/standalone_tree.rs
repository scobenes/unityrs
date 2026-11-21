// Ported from UnityPy 1.25.2 (MIT): helpers/TypeTreeHelper.py:123-158. See THIRD_PARTY_LICENSES.
//! Explicit helper reads share the bounded parser and Python construction policy.
use crate::{
    errors,
    settings::{Budget, Limits},
};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{Meter, MeteredVec},
};

#[pyfunction]
#[pyo3(signature = (data, rows, endian, as_dict=true, assetsfile=None, limits=None, budget=None, owner=None, references=None, guard=None))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn standalone_tree_read(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    rows: &Bound<'_, PyAny>,
    endian: &str,
    as_dict: bool,
    assetsfile: Option<&crate::files::NativeFile>,
    limits: Option<&Limits>,
    budget: Option<&Budget>,
    owner: Option<&crate::files::NativeFile>,
    references: Option<&Bound<'_, PyAny>>,
    guard: Option<Py<crate::traversal::TreeTraversal>>,
) -> PyResult<Py<PyAny>> {
    // Copy the account, then release the Python borrow before resolver callbacks
    // re-enter the same traversal guard on the caller.
    let guard_context = guard
        .map(|guard| {
            let guard = guard.try_borrow(py)?;
            Ok::<_, PyErr>((guard.limits, guard.meter.clone()))
        })
        .transpose()?;
    let file = assetsfile.and_then(|v| match &v.inner {
        unityrs_core::File::Serialized(file) => Some(file.clone()),
        _ => None,
    });
    let source = file.clone().or_else(|| {
        owner.and_then(|v| match &v.inner {
            unityrs_core::File::Serialized(file) => Some(file.clone()),
            _ => None,
        })
    });
    let inherit_meter = limits.is_none() && budget.is_none();
    let limits = guard_context.as_ref().map_or_else(
        || {
            limits.map_or_else(
                || {
                    source
                        .as_ref()
                        .map_or_else(unityrs_core::limits::Limits::default, |f| {
                            f.context().limits
                        })
                },
                |v| v.inner,
            )
        },
        |(limits, _)| *limits,
    );
    let meter = if let Some((_, meter)) = &guard_context {
        Some(meter.clone())
    } else if inherit_meter {
        source.as_ref().map(|f| f.meter().clone())
    } else {
        None
    }
    .unwrap_or_else(|| {
        let budget: Arc<dyn unityrs_core::budget::MemoryBudget> = budget.map_or_else(
            || Arc::new(Unbounded) as Arc<dyn unityrs_core::budget::MemoryBudget>,
            |b| b.inner.clone(),
        );
        Meter::for_input(&limits, budget, Reservation::Input)
    });
    let endian = match endian {
        "<" => unityrs_core::stream::Endian::Little,
        ">" => unityrs_core::stream::Endian::Big,
        _ => {
            return Err(crate::errors::new::<pyo3::exceptions::PyValueError>(
                "Invalid endian",
            ));
        }
    };
    let data =
        crate::buffer_snapshot::BufferSnapshot::source(data, &meter, limits.max_input_bytes)?;
    let tree = crate::tree::compile_rows_with(&meter, &limits, rows.as_unbound())?;
    let references = references
        .map(crate::reference_input::ReferenceInput::snapshot)
        .transpose()?;
    py.detach(|| {
        crate::worker::run(limits.stack_needed(), || {
            // DecodeBuf makes the input copy fallible and keeps its charge live.
            let mut copied = unityrs_core::budget::DecodeBuf::new(
                &meter,
                0,
                limits.max_input_memory,
                unityrs_core::LimitKind::InputMemory,
            )
            .map_err(errors::charge)?;
            meter
                .count_decode(data.len() as u64)
                .map_err(errors::charge)?;
            copied.extend_from_slice(&data).map_err(errors::charge)?;
            let input = copied.into_bytes().map_err(errors::charge)?;
            let refs = references.as_ref().map_or_else(
                || {
                    file.as_ref()
                        .map_or(unityrs_core::walk::RefTypes::NoFile, |f| f.ref_types_view())
                },
                crate::reference_input::ReferenceInput::view,
            );
            let options = unityrs_core::walk::ReadOptions {
                limits,
                ..Default::default()
            };
            let ctx = unityrs_core::walk::ReadCtx {
                endian,
                ref_types: refs,
                options: &options,
                meter: &meter,
            };
            parse(&input, &tree, &ctx, references.as_ref(), as_dict)
        })
    })?
}

fn parse(
    input: &unityrs_core::Bytes,
    tree: &Arc<unityrs_core::typetree::Tree>,
    ctx: &unityrs_core::walk::ReadCtx<'_>,
    references: Option<&crate::reference_input::ReferenceInput>,
    as_dict: bool,
) -> PyResult<Py<PyAny>> {
    let meter = ctx.meter;
    let limits = ctx.options.limits;
    let mut selected = MeteredVec::new(meter);
    let resolver = references.and_then(crate::reference_input::ReferenceInput::resolver);
    let mut resolve =
        |cls: &unityrs_core::Value, ns: &unityrs_core::Value, asm: &unityrs_core::Value, at| {
            match resolver {
                Some(resolver) => crate::reference_input::resolve(
                    resolver,
                    meter,
                    limits,
                    [cls, ns, asm],
                    at,
                    &mut selected,
                ),
                None => Ok(None),
            }
        };
    let mut progress = unityrs_core::walk::FieldProgress::default();
    let parsed = if resolver.is_some() {
        unityrs_core::walk::boost::read_resolved(
            input,
            tree,
            ctx,
            if as_dict { None } else { Some(&mut progress) },
            Some(&mut resolve),
        )
    } else if as_dict {
        unityrs_core::walk::boost::read(input, tree, ctx)
    } else {
        unityrs_core::walk::boost::read_object_fields(input, tree, ctx).map(|(parsed, fields)| {
            progress = fields;
            parsed
        })
    }
    .map_err(|e| errors::parse_reader(&e.into(), true))?;
    let refs = if resolver.is_some() {
        Some(selected.as_slice())
    } else {
        match ctx.ref_types {
            unityrs_core::walk::RefTypes::List(refs) => Some(refs),
            _ => None,
        }
    };
    convert(parsed, progress, refs, as_dict, meter)
}

/// The result, staged detached; each part is built with one attachment. The
/// parsed tree is dropped (on the worker's stack) once the value is built,
/// before the node metadata adds its staging; only its node lists stay.
fn convert(
    parsed: unityrs_core::Parsed,
    progress: unityrs_core::walk::FieldProgress,
    refs: Option<&[unityrs_core::files::SerializedType]>,
    as_dict: bool,
    meter: &Arc<Meter>,
) -> PyResult<Py<PyAny>> {
    use crate::python::held as h;
    let mut charge = unityrs_core::limits::Charge::zero(meter);
    charge.grow(512).map_err(errors::charge)?;
    let value = crate::value_render::convert(&parsed, meter, &mut charge, !as_dict)?;
    let mut lists =
        MeteredVec::with_capacity(meter, parsed.lists().len()).map_err(errors::charge)?;
    for list in parsed.lists() {
        lists.push(list.clone()).map_err(errors::charge)?;
    }
    let bytes_read = parsed.bytes_read;
    drop(parsed);
    let nodes = crate::object_nodes::convert(lists.as_slice(), &mut charge)?;
    let references = crate::object_nodes::references(lists.as_slice(), refs, &mut charge)?;
    let error = progress
        .error
        .map(|error| errors::parse_reader(&error.into(), true));
    crate::python::relay::attach(move |py| {
        let error = error.map_or_else(|| h::none(py), |error| error.value(py).clone().into_any());
        let construction = Bound::new(py, crate::construction::Construction { charge })?;
        h::tuple_of(
            py,
            &[
                value.into_bound(py),
                nodes.into_bound(py),
                references.into_bound(py),
                h::int(py, bytes_read)?,
                h::uint(py, progress.completed as u64)?,
                error,
                construction.into_any(),
            ],
        )
        .map(Bound::unbind)
    })
}

#[cfg(test)]
mod tests {
    use pyo3::prelude::*;

    #[test]
    fn detached_schema_retains_asset_reader_account() -> PyResult<()> {
        let _serial = crate::tests::serial();
        crate::python::relay::attach(|py| {
            let native = PyModule::new(py, "unityrs._native")?;
            crate::native(&native)?;
            py.import("sys")?
                .getattr("modules")?
                .set_item("unityrs._native", native)?;
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            let paths = py.import("sys")?.getattr("path")?;
            for directory in [root.join("python"), root.join("python/tests")] {
                paths.call_method1("insert", (0, directory.to_string_lossy().as_ref()))?;
            }
            let case = py.import("test_y6b_meter")?.getattr("TreeMeter")?.call0()?;
            for name in [
                "test_detached_schema_refuses_count_expansion_on_reader_budget",
                "test_detached_schema_inherits_reader_value_limits_in_both_parsers",
                "test_accelerated_detached_schema_keeps_guard_budget_inside_native_parser",
                "test_reader_account_precedes_a_different_schema_owner",
            ] {
                case.call_method0(name)?;
            }
            Ok(())
        })
    }
}
