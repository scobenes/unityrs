// Ported from UnityPy 1.25.2 (MIT): helpers/TypeTreeHelper.py:123-158. See THIRD_PARTY_LICENSES.
//! Explicit helper reads share the bounded parser and Python construction policy.
use crate::{
    errors, python as p,
    settings::{Budget, Limits},
};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{Meter, MeteredVec},
};

#[pyfunction]
#[pyo3(signature = (data, rows, endian, as_dict=true, assetsfile=None, limits=None, budget=None, owner=None, references=None))]
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
) -> PyResult<Py<PyAny>> {
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
    let limits = limits.map_or_else(
        || {
            source
                .as_ref()
                .map_or_else(unityrs_core::limits::Limits::default, |f| {
                    f.context().limits
                })
        },
        |v| v.inner,
    );
    let meter = if inherit_meter {
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
        _ => return Err(pyo3::exceptions::PyValueError::new_err("Invalid endian")),
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
    convert(&parsed, progress, refs, as_dict, meter)
}

fn convert(
    parsed: &unityrs_core::Parsed,
    progress: unityrs_core::walk::FieldProgress,
    refs: Option<&[unityrs_core::files::SerializedType]>,
    as_dict: bool,
    meter: &Arc<Meter>,
) -> PyResult<Py<PyAny>> {
    let mut plan = p::plan::Plan::new(meter);
    plan.charge.grow(512).map_err(errors::charge)?;
    plan.root = crate::value_plan::convert(parsed, meter, &mut plan, !as_dict)?;
    let (value, mut charge) = plan.handoff()?;
    let result = p::list()?;
    p::push(&result, &value)?;
    p::push(
        &result,
        &crate::object_nodes::convert(parsed.lists(), &mut charge)?,
    )?;
    p::push(
        &result,
        &crate::object_nodes::references(parsed.lists(), refs, &mut charge)?,
    )?;
    p::push(&result, &p::int(parsed.bytes_read)?)?;
    p::push(&result, &p::uint(progress.completed as u64)?)?;
    let error = progress.error.map_or_else(p::none, |error| {
        let error = errors::parse_reader(&error.into(), true);
        crate::python::relay::attach(|py| error.value(py).clone().into_any().unbind())
    });
    p::push(&result, &error)?;
    let construction = crate::python::relay::attach(|py| {
        Py::new(py, crate::construction::Construction { charge }).map(Py::into_any)
    })?;
    p::push(&result, &construction)?;
    p::tuple(&result)
}
