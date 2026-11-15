//! Replay C2 L0/L1/L2 against UnityPy 1.25.2 and brotli 1.2.0.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "support/web.rs"]
mod support;
#[test]
fn web_replays_pinned_unitypy_with_gzip_as_tier_b() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/web/oracle.json");
    if !path.exists() {
        assert!(
            std::env::var_os("CI").is_none(),
            "generate tools/fixtures/web.py"
        );
        eprintln!("SKIP: C2 oracle awaits coordinator generation");
        return;
    }
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(fixture["inputs"], support::inputs());
    for (case, answer) in fixture["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .zip(fixture["answers"].as_array().unwrap())
    {
        for stream in [false, true] {
            let got = support::summary(support::unhex(case["hex"].as_str().unwrap()), stream);
            let want = &answer[if stream { "stream" } else { "memory" }];
            if case["tier"] == "U" {
                assert_eq!(got, serde_json::json!({"error":true}));
            } else {
                assert_eq!(&got, want, "{} stream={stream}", case["name"]);
            }
        }
        if case["tier"] == "B" {
            assert_eq!(answer["gzip_actual"], serde_json::json!({"error":true}));
        }
    }
}
