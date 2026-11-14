//! Independent checks of raw §7.3 facts, before schedule-dependent figures
//! and resume statuses are removed for comparison with oracle-derived pins.
use serde_json::{Map, Value};

pub(super) fn validate(
    head: &Value,
    rows: &Map<String, Value>,
    expected: &Value,
    spec: &Value,
    case: &Value,
) {
    let parsed = unityrs_batch::spec::parse(serde_json::to_vec(spec).unwrap()).unwrap();
    let spec = &parsed.spec;
    for (field, limit) in [
        (
            "max_inflight_uncompressed_bytes",
            spec.max_inflight_uncompressed_bytes,
        ),
        ("max_decoded_bytes", spec.max_decoded_bytes),
        ("dep_cache_bytes", spec.dep_cache_bytes),
        (
            "max_attempt_evictions",
            u64::from(spec.max_attempt_evictions),
        ),
        ("max_export_work", spec.max_export_work),
    ] {
        assert_eq!(head["run"][field], limit, "reported budget {field}");
    }
    let inputs = rows["inputs"].as_array().unwrap();
    let bundles = rows["bundles"].as_array().unwrap();
    for row in inputs {
        let pinned = expected["rows"]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == row["id"])
            .unwrap();
        // The hand rules pin null evictions for failure before dispatch. A
        // dispatched failed input still carries all of its attempt figures.
        let dispatched = !pinned["evictions"].is_null();
        for field in [
            "input_peak",
            "dependency_peak",
            "evictions",
            "export_work",
            "ms",
        ] {
            assert_eq!(row[field].is_u64(), dispatched, "{} {field}", row["id"]);
            assert_eq!(row[field].is_null(), !dispatched, "{} {field}", row["id"]);
        }
        if dispatched {
            assert!(
                number(row, "input_peak") > 0,
                "attempt loaded a synthetic bundle"
            );
            assert!(number(row, "input_peak") <= spec.max_decoded_bytes);
            assert!(number(row, "dependency_peak") <= spec.dep_cache_bytes);
            assert!(number(row, "evictions") <= u64::from(spec.max_attempt_evictions));
            assert!(
                number(row, "export_work") > 0,
                "attempt writes records or refuses export work"
            );
        }
        let skips = rows["skips"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|skip| skip["input"] == row["id"])
            .count();
        assert_eq!(number(row, "skips"), u64::try_from(skips).unwrap());
    }
    for row in bundles {
        let peak = number(row, "peek_peak");
        number(row, "peek_work");
        assert!(peak <= spec.max_decoded_bytes);
        if row["status"] == "staged" {
            assert!(peak > 0, "staged synthetic bundle was peeked");
        } else {
            assert_eq!(peak, 0);
            assert_eq!(number(row, "peek_work"), 0);
            assert!(row["peek"].is_null());
        }
    }
    aggregates(head, rows, spec, case);
    counts(head, rows);
}

fn aggregates(
    head: &Value,
    rows: &Map<String, Value>,
    spec: &unityrs_batch::spec::model::Spec,
    case: &Value,
) {
    let bundles = rows["bundles"].as_array().unwrap();
    let inputs = rows["inputs"].as_array().unwrap();
    for (run, family, field, sum) in [
        ("input_peak", "inputs", "input_peak", false),
        ("dependency_peak", "inputs", "dependency_peak", false),
        ("export_work_peak", "inputs", "export_work", false),
        ("evictions", "inputs", "evictions", true),
        ("peek_peak", "bundles", "peek_peak", false),
        ("peek_work_peak", "bundles", "peek_work", false),
    ] {
        let values = rows[family]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row[field].as_u64());
        let aggregate = if sum {
            values.sum()
        } else {
            values.max().unwrap_or(0)
        };
        assert_eq!(head["run"][run], aggregate, "raw aggregate {run}");
    }
    let staged = bundles
        .iter()
        .filter(|b| b["status"] == "staged")
        .map(|b| number(b, "size"))
        .sum::<u64>();
    assert!(number(&head["run"], "spool_peak") >= staged);
    assert!(number(&head["run"], "spool_peak") <= spec.spool.size_bytes);
    // Every input, including an input failed before dispatch, commits a
    // records file. Part accounting rounds each file to the spool's 4 KiB unit.
    assert!(number(&head["run"], "spooled_parts") >= u64::try_from(inputs.len()).unwrap() * 4096);
    assert!(number(&head["run"], "spooled_parts").is_multiple_of(4096));
    let output_bytes = case["outputs"]
        .as_object()
        .unwrap()
        .keys()
        .map(|key| {
            rows["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["key"] == *key)
                .unwrap()["size"]
                .as_u64()
                .unwrap()
        })
        .sum::<u64>();
    assert!(number(&head["run"], "spooled_parts") >= output_bytes);
}

fn counts(head: &Value, rows: &Map<String, Value>) {
    for (field, family, status) in [
        ("bundles", "bundles", None),
        ("bundles_failed", "bundles", Some("failed")),
        ("inputs", "inputs", None),
        ("inputs_complete", "inputs", Some("complete")),
        ("inputs_with_skips", "inputs", Some("complete_with_skips")),
        ("inputs_failed", "inputs", Some("failed")),
        ("inputs_not_run", "inputs", Some("not_run")),
        ("outputs_written", "outputs", Some("written")),
        ("outputs_present", "outputs", Some("present")),
        ("outputs_not_written", "outputs", Some("not_written")),
        ("outputs_withheld", "outputs", Some("withheld")),
        ("outputs_failed", "outputs", Some("failed")),
        ("outputs_unwritable", "outputs", Some("unwritable_key")),
        ("skips", "skips", None),
        ("archives_written", "archives", Some("written")),
        ("archives_present", "archives", Some("present")),
    ] {
        let count = rows[family]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| status.is_none_or(|status| row["status"] == status))
            .count();
        assert_eq!(head["counts"][field], count, "raw count {field}");
    }
    for (field, kind) in [
        ("same_named_cabs", "same_named_cab"),
        ("undeclared_dependencies", "undeclared_dependency"),
        ("nested_containers", "nested_container"),
    ] {
        let count = rows["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == kind)
            .count();
        assert_eq!(head["run"][field], count);
    }
}

fn number(row: &Value, field: &str) -> u64 {
    row[field]
        .as_u64()
        .unwrap_or_else(|| panic!("{field} must be an integer: {}", row[field]))
}

/// Prove the pre-normalization checks reject the review's concrete regressions
/// on a real emitted synthetic run, including internally consistent zeroing.
pub(super) fn reject_corruption(
    head: &Value,
    rows: &Map<String, Value>,
    expected: &Value,
    spec: &Value,
    case: &Value,
) {
    for mutation in [
        "null_attempt",
        "zero_accounting",
        "wrong_budget",
        "null_peek",
        "wrong_count",
    ] {
        let mut head = head.clone();
        let mut rows = rows.clone();
        match mutation {
            "null_attempt" => {
                rows.get_mut("inputs").unwrap()[0]["input_peak"] = Value::Null;
            }
            "zero_accounting" => {
                for field in ["input_peak", "export_work"] {
                    rows.get_mut("inputs").unwrap()[0][field] = 0.into();
                }
                for field in [
                    "input_peak",
                    "export_work_peak",
                    "spooled_parts",
                    "spool_peak",
                ] {
                    head["run"][field] = 0.into();
                }
            }
            "wrong_budget" => head["run"]["max_export_work"] = 0.into(),
            "null_peek" => rows.get_mut("bundles").unwrap()[0]["peek_peak"] = Value::Null,
            "wrong_count" => head["counts"]["outputs_written"] = 0.into(),
            _ => unreachable!(),
        }
        assert!(
            std::panic::catch_unwind(|| validate(&head, &rows, expected, spec, case)).is_err(),
            "raw accounting accepted {mutation}"
        );
    }
}
