//! Synthetic-only agent protocol tests. Goldens and documentation examples
//! are regenerated together by the command documented in docs/AGENTS.md.
#[path = "agent/contracts.rs"]
mod contracts;
#[path = "agent/edges.rs"]
mod edges;
#[path = "agent/support.rs"]
mod support;
use serde_json::json;
use support::{assert_cuts, fixture, golden, json, run};

#[test]
fn goldens() {
    let tmp = fixture();
    let inspect = json(&tmp, &["inspect", "synthetic.assets", "--name", "alpha"], 0);
    let show = json(&tmp, &["show", "synthetic.assets", "--path-id", "-2"], 0);
    let describe = json(&tmp, &["describe"], 0);
    let error = json(&tmp, &["show", "synthetic.assets", "--path-id", "999"], 1);
    for (name, value) in [
        ("inspect", &inspect),
        ("show", &show),
        ("describe", &describe),
        ("error", &error),
    ] {
        golden(name, value);
    }
    let projected = json!({"format":describe["format"],"version":describe["version"],"schemas":describe["schemas"],"batch_schema":describe["batch_schema"]});
    golden("describe-example", &projected);
    contracts::docs_examples(&[
        ("inspect", &inspect),
        ("show", &show),
        ("describe", &projected),
        ("error", &error),
    ]);
}
#[test]
fn pagination_order_and_cursor_refusals() {
    let tmp = fixture();
    let mut cursor: Option<String> = None;
    let mut got = Vec::new();
    loop {
        let mut args = vec![
            "inspect",
            "synthetic.bundle",
            "synthetic.assets",
            "--limit",
            "2",
        ];
        if let Some(ref token) = cursor {
            args.extend(["--cursor", token]);
        }
        let page = json(&tmp, &args, 0);
        assert_eq!(page["total_objects"], 9);
        for object in page["objects"].as_array().unwrap() {
            got.push((
                object["input"].clone(),
                object["file"].clone(),
                object["path_id"].clone(),
            ));
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(got.len(), 9);
    assert_eq!(
        got.iter()
            .map(|(i, f, p)| format!("{i}:{f}:{p}"))
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        9
    );
    assert_eq!(
        got.iter()
            .map(|(_, _, p)| p.as_i64().unwrap())
            .collect::<Vec<_>>(),
        [-2, 10, 30, -2, 10, 30, -2, 10, 30]
    );
    assert_eq!(got[0].1, "CAB-z");
    assert_eq!(got[3].1, "CAB-a");
    let first = json(&tmp, &["inspect", "synthetic.assets", "--limit", "1"], 0);
    let token = first["next_cursor"].as_str().unwrap();
    for args in [
        vec!["inspect", "synthetic.bundle", "--cursor", token],
        vec![
            "inspect",
            "synthetic.assets",
            "--class",
            "1",
            "--cursor",
            token,
        ],
        vec![
            "inspect",
            "synthetic.assets",
            "--name",
            "alpha",
            "--cursor",
            token,
        ],
    ] {
        assert_eq!(json(&tmp, &args, 1)["code"], "bad_cursor");
    }
    let tampered = format!("{token}x");
    assert_eq!(
        json(
            &tmp,
            &["inspect", "synthetic.assets", "--cursor", &tampered],
            1
        )["code"],
        "bad_cursor"
    );
}
#[test]
fn filters_and_input_failures() {
    let tmp = fixture();
    for class in ["TextAsset", "49"] {
        assert_eq!(
            json(&tmp, &["inspect", "synthetic.assets", "--class", class], 0)["total_objects"],
            2
        );
    }
    for (glob, n) in [("a*", 1), ("?eta", 1), ("[!a]*", 2), ("absent", 0)] {
        assert_eq!(
            json(&tmp, &["inspect", "synthetic.assets", "--name", glob], 0)["total_objects"],
            n
        );
    }
    assert_eq!(
        json(
            &tmp,
            &[
                "inspect",
                "synthetic.assets",
                "--class",
                "49",
                "--class",
                "1"
            ],
            0
        )["total_objects"],
        3
    );
    assert_eq!(
        json(
            &tmp,
            &[
                "inspect",
                "synthetic.assets",
                "--name",
                "alpha",
                "--name",
                "beta"
            ],
            0
        )["total_objects"],
        2
    );
    assert_eq!(
        json(
            &tmp,
            &[
                "inspect",
                "synthetic.assets",
                "--class",
                "1",
                "--name",
                "alpha"
            ],
            0
        )["total_objects"],
        0
    );
    let value = json(&tmp, &["inspect", "broken.bundle", "synthetic.assets"], 0);
    assert_eq!(value["inputs"][0]["error"]["code"], "input");
    assert_eq!(value["total_objects"], 3);
    assert_eq!(value["objects"][0]["input"], 1);
    assert_eq!(
        json(&tmp, &["show", "broken.bundle", "--path-id", "1"], 1)["code"],
        "input"
    );
    let ambiguous = json(&tmp, &["show", "synthetic.bundle", "--path-id", "-2"], 1);
    assert_eq!(ambiguous["code"], "ambiguous");
    assert_eq!(ambiguous["detail"].as_array().unwrap().len(), 2);
    assert_eq!(
        json(
            &tmp,
            &[
                "show",
                "synthetic.bundle",
                "--path-id",
                "-2",
                "--file",
                "CAB-a"
            ],
            0
        )["object"]["file"],
        "CAB-a"
    );
    assert_eq!(
        json(
            &tmp,
            &[
                "show",
                "synthetic.assets",
                "--path-id",
                "-2",
                "--file",
                "missing"
            ],
            1
        )["code"],
        "not_found"
    );
}
#[test]
fn bounded_values_and_binary_modes() {
    let tmp = fixture();
    for extra in [
        vec![],
        vec!["--max-items", "2"],
        vec!["--max-depth", "1"],
        vec!["--max-depth", "0"],
        vec!["--max-bytes", "700"],
        vec!["--max-items", "0"],
        vec!["--bytes", "full"],
    ] {
        let mut args = vec!["show", "rich.assets", "--path-id", "7"];
        args.extend(extra);
        let v = json(&tmp, &args, 0);
        assert_cuts(&v);
    }
    let default = json(&tmp, &["show", "rich.assets", "--path-id", "7"], 0);
    assert_eq!(default["truncated"]["binary"], 2);
    let bytes = &default["value"]["v"][3][1];
    assert_eq!(bytes["length"], 100);
    assert_eq!(bytes["prefix_hex"].as_str().unwrap().len(), 128);
    assert_eq!(
        bytes["sha256"],
        unityrs_dump::json::sha256_hex(&(0..100).collect::<Vec<u8>>())
    );
    assert_eq!(
        default["value"]["v"][7][1],
        serde_json::json!({"t":"str","hex":"ff61"})
    );
    let full = json(
        &tmp,
        &["show", "rich.assets", "--path-id", "7", "--bytes", "full"],
        0,
    );
    assert_eq!(full["truncated"]["cuts"], 0);
    assert_eq!(full["value"]["v"][3][1]["hex"].as_str().unwrap().len(), 200);
    let limited = json(
        &tmp,
        &["show", "rich.assets", "--path-id", "7", "--max-items", "2"],
        0,
    );
    assert!(limited["truncated"]["items"].as_u64().unwrap() > 0);
    let depth = json(
        &tmp,
        &["show", "rich.assets", "--path-id", "7", "--max-depth", "1"],
        0,
    );
    assert!(depth["truncated"]["depth"].as_u64().unwrap() > 0);
    let size = json(
        &tmp,
        &[
            "show",
            "rich.assets",
            "--path-id",
            "7",
            "--max-bytes",
            "700",
        ],
        0,
    );
    assert!(size["truncated"]["bytes"].as_u64().unwrap() > 0);
    let output = run(
        &tmp,
        &[
            "show",
            "rich.assets",
            "--path-id",
            "7",
            "--max-bytes",
            "700",
        ],
    );
    assert!(output.stdout.len() <= 700);
}
#[test]
fn budgets_and_usage_are_structured() {
    let tmp = fixture();
    for option in [
        "--max-input-memory",
        "--max-input-bytes",
        "--max-decode-work",
        "--max-tree-depth",
    ] {
        let status = i32::from(option == "--max-input-memory");
        let value = json(&tmp, &["inspect", "synthetic.assets", option, "0"], status);
        if status == 1 {
            assert_eq!(value["code"], "budget");
            continue;
        }
        assert_eq!(
            value["inputs"][0]["error"]["code"], "budget",
            "{option}: {value}"
        );
        assert_eq!(
            json(
                &tmp,
                &["show", "synthetic.assets", "--path-id", "-2", option, "0"],
                1
            )["code"],
            "budget"
        );
    }
    assert_eq!(
        json(
            &tmp,
            &[
                "show",
                "synthetic.assets",
                "--path-id",
                "-2",
                "--max-bytes",
                "1"
            ],
            1
        )["code"],
        "budget"
    );
    for args in [
        vec!["inspect"],
        vec!["inspect", "synthetic.assets", "--limit", "10001"],
        vec!["inspect", "synthetic.assets", "--limit", "0"],
        vec!["inspect", "synthetic.assets", "--class", "bad"],
        vec!["show", "synthetic.assets"],
        vec!["show", "synthetic.assets", "--path-id", "oops"],
        vec![
            "show",
            "synthetic.assets",
            "--path-id",
            "1",
            "--bytes",
            "invalid",
        ],
        vec!["describe", "input"],
        vec!["describe", "--schema", "batch"],
        vec!["inspect", "synthetic.assets", "--max-input-bytes", "x"],
        vec!["describe", "--bogus"],
        vec!["inspect", "synthetic.assets", "--limit"],
    ] {
        assert_eq!(json(&tmp, &args, 2)["code"], "usage");
    }
}
