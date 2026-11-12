//! Run the independent PNG checker's malformed-data regressions in X1's gate.
#[test]
fn png_checker_rejects_incomplete_and_trailing_image_data() {
    let result = unityrs_testkit::child::output(
        std::process::Command::new("python3").arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/attempt_support/test_check.py"),
        ),
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
