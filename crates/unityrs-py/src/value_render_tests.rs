//! Rendering stays sound when Python code runs in the middle of it.
#![allow(clippy::panic, reason = "a malformed fixture fails the test")]
use pyo3::{prelude::*, types::PyDict};
use unityrs_core::{
    PrimArray, Value,
    limits::Meter,
    typetree::{NodeId, NodeList, NodeRow},
    value::{PrimRun, ValueBuilder},
};

/// A list of `n` dicts `{"x": i}`, each beside a list of floats.
fn lists_of_dicts(n: i64) -> unityrs_core::Parsed {
    let rows = vec![
        NodeRow::new(0, "Base", "Object"),
        NodeRow::new(1, "int", "x"),
    ];
    let nodes = NodeList::trusted(rows)
        .and_then(NodeList::into_shared)
        .unwrap_or_else(|error| std::panic::panic_any(format!("{error:?}")));
    let builder = ValueBuilder::new(&Meter::unlimited(), nodes)
        .unwrap_or_else(|error| std::panic::panic_any(format!("{error:?}")));
    let mut items = builder.vec();
    for i in 0..n {
        let mut entries = builder.vec();
        let mut floats = builder.vec();
        for x in 0..4_u32 {
            let _ = floats.push(f64::from(x).to_bits());
        }
        let floats = builder.prims(PrimRun::F64(floats));
        let mut pair = builder.vec();
        if let Ok(floats) = floats {
            let _ = pair.push_built(floats);
        }
        let _ = entries.push((NodeId(1), Value::Int(i)));
        let _ = pair.push_built(builder.map(0, entries));
        let _ = items.push_built(builder.list(pair));
    }
    let root = builder.list(items);
    builder.finish(root, 0)
}

#[test]
fn collections_during_rendering_never_see_unfilled_lists() -> PyResult<()> {
    let _serial = crate::tests::serial();
    // Past the dict and list free lists, so allocations count toward collections.
    let parsed = lists_of_dicts(2000);
    assert!(matches!(
        parsed.value(),
        Value::List(xs) if matches!(xs.first(), Some(Value::List(pair)) if matches!(pair.first(), Some(Value::Prims(PrimArray::F64(_)))))
    ));
    let scope = Python::attach(|py| -> PyResult<Py<PyDict>> {
        let scope = PyDict::new(py);
        // Every list a collection can reach is read through, as any code a
        // finalizer or collection callback runs could read it.
        py.run(
            c"
import gc
walks = [0]
def walk(phase, info):
    if phase == 'start':
        for o in gc.get_objects():
            if type(o) is list:
                for item in o:
                    type(item)
        walks[0] += 1
threshold = gc.get_threshold()
gc.callbacks.append(walk)
gc.set_threshold(50)
",
            Some(&scope),
            None,
        )?;
        Ok(scope.unbind())
    })?;
    let rendered = super::convert(
        &parsed,
        &Meter::unlimited(),
        &mut unityrs_core::limits::Charge::zero(&Meter::unlimited()),
        false,
    );
    Python::attach(|py| -> PyResult<()> {
        let scope = scope.bind(py);
        py.run(
            c"gc.set_threshold(*threshold); gc.callbacks.remove(walk)",
            Some(scope),
            None,
        )?;
        let value = rendered?;
        scope.set_item("value", value)?;
        py.run(
            c"
assert len(value) == 2000, len(value)
assert value[5] == [[0.0, 1.0, 2.0, 3.0], {'x': 5}], value[5]
assert walks[0] > 0, 'no collection ran while rendering'
",
            Some(scope),
            None,
        )
    })
}
