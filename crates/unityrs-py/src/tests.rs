//! Replay recorded API observations through the installed-style Python facade.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use pyo3::{prelude::*, types::PyBytes};
use serde_json::Value;
use std::path::Path;

#[test]
fn replays_unitypy_api_observations() -> PyResult<()> {
    let records: Value = serde_json::from_str(include_str!("../tests/fixtures/api.json")).unwrap();
    Python::attach(|py| {
        let module = PyModule::new(py, "unityrs._native")?;
        super::native(&module)?;
        py.import("sys")?
            .getattr("modules")?
            .set_item("unityrs._native", module)?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let paths = py.import("sys")?.getattr("path")?;
        paths.call_method1(
            "insert",
            (0, root.join("python").to_string_lossy().as_ref()),
        )?;
        paths.call_method1(
            "insert",
            (
                0,
                root.join("../../tools/parity").to_string_lossy().as_ref(),
            ),
        )?;
        let observer = py.import("binding_cases")?;
        let binding = py.import("unityrs")?;
        for record in records.as_array().unwrap() {
            let hex = record["hex"].as_str().unwrap().as_bytes();
            let bytes: Vec<u8> = hex
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
                .collect();
            let scratch = unityrs_testkit::scratch::Scratch::new("py-api");
            let got = if record["name"] == "paths" {
                observer.getattr("observe_paths")?.call1((
                    &binding,
                    PyBytes::new(py, &bytes),
                    scratch.path().to_string_lossy().as_ref(),
                ))?
            } else {
                observer
                    .getattr("observe")?
                    .call1((&binding, PyBytes::new(py, &bytes)))?
            };
            let encoded: String = py
                .import("json")?
                .getattr("dumps")?
                .call1((got,))?
                .extract()?;
            let got: Value = serde_json::from_str(&encoded).unwrap();
            if let Some(difference) = first_difference(&got, &record["expected"], "$") {
                panic!("{}: {difference}", record["name"]);
            }
        }
        Ok(())
    })
}

fn first_difference(got: &Value, want: &Value, path: &str) -> Option<String> {
    if got == want {
        return None;
    }
    match (got, want) {
        (Value::Object(g), Value::Object(w)) if g.len() == w.len() => {
            for (key, v) in w {
                if let Some(found) = first_difference(&g[key], v, &format!("{path}.{key}")) {
                    return Some(found);
                }
            }
        }
        (Value::Array(g), Value::Array(w)) if g.len() == w.len() => {
            for (i, (g, w)) in g.iter().zip(w).enumerate() {
                if let Some(found) = first_difference(g, w, &format!("{path}[{i}]")) {
                    return Some(found);
                }
            }
        }
        _ => return Some(format!("{path}: got {got}, want {want}")),
    }
    Some(format!("{path}: keys differ"))
}
