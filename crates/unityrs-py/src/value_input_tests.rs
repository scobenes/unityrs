//! Python snapshots must preserve edited values and bound hostile graphs.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use pyo3::prelude::*;
use unityrs_core::limits::{Limits, Meter};

#[test]
fn shader_int_properties_keep_the_binding_value_error() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let locals = pyo3::types::PyDict::new(py);
        py.run(
            c"
from types import SimpleNamespace as N
prop = N(m_Attributes=['HDR'], m_Name='_Count', m_Description='Count',
         m_Type=5, m_Flags=0, m_DefValue_0_=1.0)
props = N(m_Props=[prop])
form = N(m_Name='Synthetic/Int', m_PropInfo=props, m_SubShaders=[],
         m_FallbackName='', m_CustomEditorName='')
fields = dict(compressedBlob=b'x', platforms=[], m_ParsedForm=form)
",
            None,
            Some(&locals),
        )?;
        for (stage, name) in [
            ("property", "prop"),
            ("properties", "props"),
            ("shader", "form"),
        ] {
            let input = pyo3::types::PyDict::new(py);
            input.set_item("value", locals.get_item(name)?.unwrap())?;
            input.set_item("platforms", pyo3::types::PyList::empty(py))?;
            input.set_item("program_blobs", pyo3::types::PyList::empty(py))?;
            let error =
                crate::crypto_shader::shader_stage(py, input.as_any(), stage, None).unwrap_err();
            assert!(error.is_instance_of::<pyo3::exceptions::PyValueError>(py));
            assert_eq!(error.value(py).str()?.to_str()?, "5");
        }
        let native = PyModule::new(py, "int_property_binding_test")?;
        crate::native(&native)?;
        let error = native
            .getattr("export_shader")?
            .call1((locals.get_item("fields")?.unwrap(), (2021, 3)))
            .unwrap_err();
        assert!(error.is_instance_of::<pyo3::exceptions::PyValueError>(py));
        assert_eq!(error.value(py).str()?.to_str()?, "5");
        Ok(())
    })
}

#[test]
fn shader_stage_omits_none_clip_and_distinguishes_missing() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py.eval(
            c"(lambda N: {'value': N(m_Name='', m_LOD=0, m_Tags=N(tags=[]), alphaToMask=N(val=0), zClip=None, zTest=N(val=4), zWrite=N(val=1), culling=N(val=2), offsetFactor=N(val=0), offsetUnits=N(val=0), lighting=False, gpuProgramID=0), 'platforms': [], 'program_blobs': []})(__import__('types').SimpleNamespace)",
            None,
            None,
        )?;
        let result = crate::crypto_shader::shader_stage(py, &input, "state", None)?;
        assert_eq!(result.extract::<String>(py)?, "  GpuProgramID 0\n");
        input.get_item("value")?.delattr("zClip")?;
        let error = crate::crypto_shader::shader_stage(py, &input, "state", None).unwrap_err();
        assert!(error.is_instance_of::<pyo3::exceptions::PyAttributeError>(py));
        Ok(())
    })
}

#[test]
fn writer_getter_reenters_owning_environment_without_deadlocking() -> PyResult<()> {
    let _serial = crate::tests::serial();
    unityrs_testkit::guard::finishes("writer getter reentering its owning environment", || {
        Python::attach(|py| {
            let module = PyModule::new(py, "unityrs._native")?;
            crate::native(&module)?;
            let sys = py.import("sys")?;
            sys.getattr("modules")?
                .set_item("unityrs._native", module)?;
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            for directory in [root.join("python"), root.join("../../tools/parity")] {
                sys.getattr("path")?
                    .call_method1("insert", (0, directory.to_string_lossy().as_ref()))?;
            }
            let result = py
                .import("binding_y4_review_object")?
                .getattr("_reentrant_writer")?
                .call1((py.import("unityrs")?, false))?;
            assert_eq!(result.get_item("bytes")?.extract::<String>()?, "0b000000");
            assert_eq!(result.get_item("reads")?.extract::<Vec<i32>>()?, [7]);
            assert_eq!(
                result
                    .get_item("saved")?
                    .get_item("value")?
                    .extract::<i32>()?,
                11
            );
            Ok(())
        })
    })
}

#[test]
fn current_python_values_roundtrip_without_generated_schema() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py.eval(
            c"{'name': 'edited\\udcff', 'pixels': b'\\x01\\x02', 'numbers': [True, -7, 2**63, 1.5], 'pair': (2, 3)}",
            None,
            None,
        )?.unbind();
        let parsed = crate::value_input::parsed(&input, &Meter::unlimited(), &Limits::default())?;
        let result = crate::value::convert(&parsed, &Meter::unlimited())?;
        assert!(result.bind(py).eq(input.bind(py))?);
        Ok(())
    })
}

#[test]
fn nested_lists_remap_current_fields_when_native_rows_move() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py.eval(
            c"{'items': [{'rows': [{'value': 1}, {'value': 2}], 'last': 11}, {'rows': [{'value': 3}, {'value': 4}], 'last': 12}]}",
            None, None,
        )?.unbind();
        let meter = Meter::unlimited();
        let parsed = crate::value_input::parsed(&input, &meter, &Limits::default())?;
        let result = crate::value::convert(&parsed, &meter)?;
        assert!(result.bind(py).eq(input.bind(py))?);
        let nodes = &parsed.lists()[0];
        for (index, row) in nodes.rows().iter().enumerate() {
            if row.name.as_bytes() == b"data" && row.ty.as_bytes() == b"dict" {
                let children: Vec<_> = nodes
                    .children(unityrs_core::typetree::NodeId(u32::try_from(index)?))
                    .iter()
                    .map(|&id| nodes.name(id))
                    .collect();
                assert!(!children.contains(&b"data".as_slice()));
            }
        }
        Ok(())
    })
}

#[test]
fn cyclic_and_deep_python_values_raise_before_unbounded_conversion() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py.eval(c"[[]]", None, None)?.unbind();
        input
            .bind(py)
            .get_item(0)?
            .call_method1("append", (&input,))?;
        let error = crate::value_input::parsed(&input, &Meter::unlimited(), &Limits::default())
            .unwrap_err();
        assert!(error.is_instance_of::<pyo3::exceptions::PyValueError>(py));
        let input = py.eval(c"[[[1]]]", None, None)?.unbind();
        let mut limits = Limits::default();
        limits.max_value_depth = 2;
        let error = crate::value_input::parsed(&input, &Meter::unlimited(), &limits).unwrap_err();
        assert!(error.is_instance_of::<crate::errors::LimitError>(py));
        Ok(())
    })
}

#[test]
fn python_value_snapshot_refuses_budget_and_recovers() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py.eval(c"{'data': b'x' * 8192}", None, None)?.unbind();
        let mut limits = Limits::default();
        limits.max_input_memory = 4096;
        let meter = Meter::for_input(
            &limits,
            std::sync::Arc::new(unityrs_core::budget::Unbounded),
            unityrs_core::budget::Reservation::Input,
        );
        let error = crate::value_input::parsed(&input, &meter, &limits).unwrap_err();
        assert!(error.is_instance_of::<pyo3::exceptions::PyMemoryError>(py));
        assert_eq!(meter.used(), 0);
        assert!(
            crate::value_input::parsed(&input, &Meter::unlimited(), &Limits::default()).is_ok()
        );
        Ok(())
    })
}

#[test]
fn explicit_none_in_dictionary_and_object_uses_the_numeric_writer_error() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let rows = unityrs_core::typetree::NodeList::trusted(vec![
            unityrs_core::typetree::NodeRow::new(0, "Synthetic", "Base"),
            unityrs_core::typetree::NodeRow::new(1, "int", "number"),
        ])
        .unwrap()
        .into_shared()
        .unwrap();
        let tree = unityrs_core::typetree::Tree::compile(rows, &Limits::default()).unwrap();
        for code in [
            c"{'number': None}",
            c"__import__('types').SimpleNamespace(number=None)",
        ] {
            let input = py.eval(code, None, None)?.unbind();
            let error = crate::value_input::for_tree(
                &input,
                &Meter::unlimited(),
                &Limits::default(),
                &tree,
            )
            .unwrap_err();
            assert_eq!(error.get_type(py).name()?, "error");
            assert_eq!(
                error.value(py).str()?.to_str()?,
                "required argument is not an integer"
            );
        }
        Ok(())
    })
}

#[test]
fn class_lists_keep_a_vector_shape_and_each_elements_current_fields() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py.eval(c"(lambda C, V: (lambda obj: (obj.__dict__.update(m_DataSize=b'', m_VertexCount=3, m_Channels=[(lambda c,i:(c.__dict__.update(dimension=3,format=0,offset=i,stream=0),c)[1])(C(),i) for i in range(3)]), obj)[1])(V()))(type('ChannelInfo',(),{}),type('VertexData',(),{}))", None, None)?.unbind();
        let meter = Meter::unlimited();
        let parsed = crate::value_input::parsed(&input, &meter, &Limits::default())?;
        let root =
            unityrs_core::objmode::Obj::root(&parsed, &unityrs_core::objmode::VERTEX_DATA, &meter)
                .unwrap();
        let channels = root
            .list("m_Channels", &unityrs_core::objmode::CHANNEL_INFO)
            .unwrap()
            .unwrap();
        assert_eq!(channels.len(), 3);
        for (i, channel) in channels.iter().enumerate() {
            assert!(channel.as_ref().unwrap().built());
            assert_eq!(
                channel.unwrap().uint("offset", 255).unwrap(),
                Some(i as u64)
            );
        }
        Ok(())
    })
}

#[test]
fn writer_snapshot_visits_only_schema_fields_and_preserves_clean_name_collisions() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let rows = unityrs_core::typetree::NodeList::trusted(vec![
            unityrs_core::typetree::NodeRow::new(0, "Synthetic", "Base"),
            unityrs_core::typetree::NodeRow::new(1, "int", "number.value"),
            unityrs_core::typetree::NodeRow::new(1, "int", "number-value"),
        ])
        .unwrap()
        .into_shared()
        .unwrap();
        let tree = unityrs_core::typetree::Tree::compile(rows, &Limits::default()).unwrap();
        let input = py
            .eval(
                c"__import__('types').SimpleNamespace(number_value=7, unused=None)",
                None,
                None,
            )?
            .unbind();
        let parsed =
            crate::value_input::for_tree(&input, &Meter::unlimited(), &Limits::default(), &tree)?;
        assert_eq!(
            parsed
                .get(&[b"number.value"])
                .and_then(unityrs_core::Value::as_i64),
            Some(7)
        );
        assert_eq!(
            parsed
                .get(&[b"number-value"])
                .and_then(unityrs_core::Value::as_i64),
            Some(7)
        );
        assert!(parsed.get(&[b"unused"]).is_none());
        let input = py
            .eval(
                c"{'number.value': 8, 'number-value': 9, 'unused': None}",
                None,
                None,
            )?
            .unbind();
        let parsed =
            crate::value_input::for_tree(&input, &Meter::unlimited(), &Limits::default(), &tree)?;
        assert_eq!(
            parsed
                .get(&[b"number.value"])
                .and_then(unityrs_core::Value::as_i64),
            Some(8)
        );
        assert_eq!(
            parsed
                .get(&[b"number-value"])
                .and_then(unityrs_core::Value::as_i64),
            Some(9)
        );
        Ok(())
    })
}

#[test]
fn mutable_and_strided_buffers_are_copied_under_the_gil() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py
            .eval(
                c"{'bytes': bytearray(b'abc'), 'view': memoryview(b'abcdef')[::2]}",
                None,
                None,
            )?
            .unbind();
        let parsed = crate::value_input::parsed(&input, &Meter::unlimited(), &Limits::default())?;
        input.bind(py).get_item("bytes")?.set_item(0, 255)?;
        assert_eq!(
            parsed
                .get(&[b"bytes"])
                .and_then(unityrs_core::Value::as_bytes),
            Some(b"abc".as_slice())
        );
        assert_eq!(
            parsed
                .get(&[b"view"])
                .and_then(unityrs_core::Value::as_bytes),
            Some(b"ace".as_slice())
        );
        Ok(())
    })
}

#[test]
fn optional_attribute_probes_propagate_allocator_failures() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let input = py.eval(c"type('Broken', (), {'__dict__': property(lambda self: (_ for _ in ()).throw(MemoryError('probe allocation')))})()", None, None)?.unbind();
        let meter = Meter::unlimited();
        let error = crate::value_input::parsed(&input, &meter, &Limits::default()).unwrap_err();
        assert!(error.is_instance_of::<pyo3::exceptions::PyMemoryError>(py));
        assert_eq!(error.value(py).str()?.to_str()?, "probe allocation");
        assert_eq!(meter.used(), 0);
        Ok(())
    })
}
