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

/// Serializes the tests that run Python. They share one embedded interpreter,
/// and first imports racing on several test threads can hand one of them a
/// partially initialized module: `CPython` breaks a cycle between two threads'
/// import locks that way. Taken before the GIL, so the order is always this
/// lock, then the GIL.
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static PYTHON: std::sync::Mutex<()> = std::sync::Mutex::new(());
    PYTHON
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn version_errors_decode_raw_bytes_with_surrogateescape() -> PyResult<()> {
    let _serial = serial();
    Python::attach(|py| {
        let raw = unityrs_core::RawStr::from(unityrs_core::Bytes::from_static(b"invalid\xff"));
        let error = super::errors::core(unityrs_core::version::VersionError::NoMatch(raw).into());
        let message: Vec<u8> = error
            .value(py)
            .str()?
            .call_method1("encode", ("utf-8", "surrogateescape"))?
            .extract()?;
        assert_eq!(message, b"Invalid version string: invalid\xff");
        Ok(())
    })
}

#[test]
fn string_staging_memory_refusal_preserves_work() -> PyResult<()> {
    staging_memory_refusal_preserves_work("string")
}

#[test]
fn bytes_staging_memory_refusal_preserves_work() -> PyResult<()> {
    staging_memory_refusal_preserves_work("bytes")
}

#[test]
fn nodes_staging_memory_refusal_preserves_work() -> PyResult<()> {
    staging_memory_refusal_preserves_work("nodes")
}

#[test]
fn references_staging_memory_refusal_preserves_work() -> PyResult<()> {
    staging_memory_refusal_preserves_work("references")
}

fn staging_memory_refusal_preserves_work(stage: &str) -> PyResult<()> {
    use std::sync::Arc;
    use unityrs_core::{
        Value as CoreValue,
        budget::{Counting, Reservation},
        limits::{Charge, Meter},
        typetree::{NodeList, NodeRow},
        value::ValueBuilder,
    };
    let _serial = serial();

    let payload = vec![b'x'; 2 * 1024 * 1024];
    let row = if stage == "nodes" {
        NodeRow::new(0, "Object", payload.clone())
    } else {
        NodeRow::new(0, "Object", "Base")
    };
    let nodes = NodeList::trusted(vec![row]).unwrap().into_shared().unwrap();
    let builder = ValueBuilder::new(&Meter::unlimited(), nodes).unwrap();
    let value = match stage {
        "string" => CoreValue::Str(payload.into()),
        "bytes" => CoreValue::Bytes(payload.into()),
        _ => CoreValue::Int(7),
    };
    let built = builder.scalar(value).unwrap();
    let parsed = builder.finish(built, 0);
    let budget = Arc::new(Counting::new(64 * 1024 * 1024));
    let meter = Meter::with_decode_work(
        64 * 1024 * 1024,
        12 * 1024 * 1024,
        budget.clone(),
        Reservation::Input,
    );
    let other = Meter::new(64 * 1024 * 1024, budget, Reservation::Input);
    let available = if stage == "references" { 128 } else { 1024 };
    let contention = other.charge(64 * 1024 * 1024 - available).unwrap();
    let convert = || {
        let mut charge = Charge::zero(&meter);
        match stage {
            "nodes" => super::object_nodes::convert(&parsed, &mut charge),
            "references" => super::object_nodes::references(&parsed, None, &mut charge),
            _ => super::value::convert(&parsed, &meter),
        }
    };
    let error = convert().unwrap_err();
    Python::attach(|py| {
        assert!(error.is_instance_of::<pyo3::exceptions::PyMemoryError>(py));
    });
    assert_eq!(meter.decoded(), 0, "{stage}: refused staging spent work");
    drop(contention);
    let result = convert()?;
    assert!(meter.decoded() > 0, "{stage}: retry must charge work");
    Python::attach(|py| -> PyResult<()> {
        match stage {
            "string" => assert_eq!(result.extract::<String>(py)?, "x".repeat(2 * 1024 * 1024)),
            "bytes" => assert_eq!(result.extract::<Vec<u8>>(py)?, vec![b'x'; 2 * 1024 * 1024]),
            _ => {}
        }
        Ok(())
    })?;
    Ok(())
}

#[test]
fn replays_unitypy_api_observations() -> PyResult<()> {
    let _serial = serial();
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
            if record.get("observer").is_some() {
                continue;
            }
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

#[test]
fn replays_unitypy_helper_observations() -> PyResult<()> {
    replay_groups("binding_y3_", 10)
}

#[test]
fn replays_unitypy_export_observations() -> PyResult<()> {
    replay_groups("binding_y4_", 70)
}

#[test]
fn replays_unitypy_tools_observations() -> PyResult<()> {
    replay_groups("binding_y5_", 29)
}

#[test]
fn replays_unitypy_residue_observations() -> PyResult<()> {
    replay_groups("binding_y6_", 20)
}

#[test]
fn replays_unitypy_internal_helper_observations() -> PyResult<()> {
    unityrs_testkit::guard::finishes(
        "caller-thread helper callbacks and local update peer",
        || replay_groups("binding_y6b_", 28),
    )
}

#[test]
fn replays_runtime_class_map_without_persisting_tpk_data() -> PyResult<()> {
    let _serial = serial();
    Python::attach(|py| {
        let module = PyModule::new(py, "unityrs._native")?;
        super::native(&module)?;
        py.import("sys")?
            .getattr("modules")?
            .set_item("unityrs._native", module)?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let paths = py.import("sys")?.getattr("path")?;
        for directory in [root.join("python"), root.join("../../tools/parity")] {
            paths.call_method1("insert", (0, directory.to_string_lossy().as_ref()))?;
        }
        let observer = py
            .import("binding_y5_modules")?
            .getattr("runtime_class_map")?;
        let json = py.import("json")?.getattr("dumps")?;
        // The oracle record exists only in this process. Q2 forbids committing
        // a class map or even a digest derived from an external TPK.
        let recorded: String = json
            .call1((observer.call1((py.import("UnityPy")?,))?,))?
            .extract()?;
        let expected: Value = serde_json::from_str(&recorded).unwrap();
        let replayed: String = json
            .call1((observer.call1((py.import("unityrs")?,))?,))?
            .extract()?;
        let actual: Value = serde_json::from_str(&replayed).unwrap();
        assert_eq!(actual, expected);
        Ok(())
    })
}

fn replay_groups(prefix: &str, expected_count: usize) -> PyResult<()> {
    let _serial = serial();
    let records: Value = serde_json::from_str(include_str!("../tests/fixtures/api.json")).unwrap();
    Python::attach(|py| {
        let module = PyModule::new(py, "unityrs._native")?;
        super::native(&module)?;
        py.import("sys")?
            .getattr("modules")?
            .set_item("unityrs._native", module)?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let paths = py.import("sys")?.getattr("path")?;
        for directory in [root.join("python"), root.join("../../tools/parity")] {
            paths.call_method1("insert", (0, directory.to_string_lossy().as_ref()))?;
        }
        let binding = py.import("unityrs")?;
        let mut count = 0;
        let mut corrected = 0;
        for record in records.as_array().unwrap() {
            let Some(observer) = record.get("observer").and_then(Value::as_str) else {
                continue;
            };
            if !observer.starts_with(prefix) {
                continue;
            }
            if let Some(oracle) = record.get("oracle") {
                assert_ne!(
                    oracle, &record["expected"],
                    "a corrected reference must preserve the original difference"
                );
                assert!(
                    record["reference"]
                        .as_str()
                        .unwrap()
                        .starts_with("PARITY.md ")
                );
                corrected += 1;
            }
            let function = record["function"].as_str().unwrap();
            let got = py.import(observer)?.getattr(function)?.call1((&binding,))?;
            let encoded: String = py
                .import("json")?
                .getattr("dumps")?
                .call1((got,))?
                .extract()?;
            let got: Value = serde_json::from_str(&encoded).unwrap();
            if let Some(difference) = first_difference(&got, &record["expected"], "$") {
                panic!("{}: {difference}", record["name"]);
            }
            count += 1;
        }
        assert_eq!(
            count, expected_count,
            "every recorded observation group must replay"
        );
        assert_eq!(
            corrected,
            match prefix {
                "binding_y4_" => 18,
                "binding_y5_" => 3,
                "binding_y6_" => 8,
                "binding_y6b_" => 2,
                _ => 0,
            }
        );
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
                let Some(got) = g.get(key) else {
                    return Some(format!("{path}.{key}: missing key"));
                };
                if let Some(found) = first_difference(got, v, &format!("{path}.{key}")) {
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

#[test]
fn reader_exception_categories_survive_the_binding_boundary() -> PyResult<()> {
    use unityrs_core::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};
    let _serial = serial();
    Python::attach(|py| {
        for (detail, expected) in [
            (ReadDetail::Python("IndexError"), "IndexError"),
            (ReadDetail::Python("AttributeError"), "AttributeError"),
            (ReadDetail::Python("KeyError: 'type'"), "KeyError"),
            (
                ReadDetail::Python("TypeError: malformed pointer"),
                "TypeError",
            ),
            (
                ReadDetail::BoostEof {
                    method: "read_bool",
                    need: 1,
                    exception: "ValueError",
                },
                "ValueError",
            ),
            (
                ReadDetail::BoostEof {
                    method: "read_length",
                    need: 4,
                    exception: "EOFError",
                },
                "EOFError",
            ),
        ] {
            let error = ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail,
            });
            let error = super::errors::parse_reader(&error, false);
            assert_eq!(error.get_type(py).name()?.to_str()?, expected);
        }
        Ok(())
    })
}

#[test]
fn replays_object_mode_observations_from_synthetic_inputs() -> PyResult<()> {
    let _serial = serial();
    let records: Value =
        serde_json::from_str(include_str!("../tests/fixtures/object_api.json")).unwrap();
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
        let observer = py.import("object_cases")?;
        let binding = py.import("unityrs")?;
        for record in records.as_array().unwrap() {
            let hex = record["hex"].as_str().unwrap().as_bytes();
            let bytes: Vec<u8> = hex
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
                .collect();
            let got = observer
                .getattr("observe")?
                .call1((&binding, PyBytes::new(py, &bytes)))?;
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
