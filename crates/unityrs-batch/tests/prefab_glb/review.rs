use super::*;

#[test]
fn invalid_static_batch_range_skips_a_prefab_with_no_remaining_mesh() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::BadStaticRange, false);
    install(&scratch, &mut spec, &input, &deps);
    spec["outputs"]["glb"]["profile"] = json!({"profile_version":1,"clips":[]});
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0]["reason"], "invalid_glb");
    assert_eq!(skips[0]["detail"], "static batch submesh range");
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(scratch.join("sink/second.glb").exists());
}

#[test]
fn a_source_selector_outside_the_view_retains_its_owner_address() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::Translation, false);
    install(&scratch, &mut spec, &input, &deps);
    let mut request = selection(2, 3);
    request["source"]["owner"] = index(100, 10);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1,"clips":[], "clips_by_prefab":{"first.glb":{"a":[request]}}
    });
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0]["reason"], "outside_view(file_index=100)");
    assert_eq!(skips[0]["detail"], "path_id=10");
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(scratch.join("sink/second.glb").exists());
}

#[test]
fn explicit_selection_ignores_an_untouched_corrupt_nested_file() {
    check_nested_serialized_file(false);
}

#[test]
fn explicit_selection_fails_the_input_for_a_needed_corrupt_nested_file() {
    check_nested_serialized_file(true);
}

#[test]
fn explicit_selection_fails_the_input_for_a_bad_nested_bundle_signature() {
    // The detector accepts the legacy bundle magic, which the bundle
    // constructor rejects after reading the format and version strings.
    let mut corrupt = "úúúúúúúú".as_bytes().to_vec();
    corrupt.resize(32, 0);
    check_nested_signature(corrupt);
}

#[test]
fn explicit_selection_fails_the_input_for_a_bad_nested_web_signature() {
    // The Brotli marker is valid, but the inflated WebFile signature isn't.
    let corrupt = unityrs_testkit::web::brotli(&[0; 128]);
    check_nested_signature(corrupt);
}

fn check_nested_signature(corrupt: Vec<u8>) {
    let (scratch, mut spec) = job(&[]);
    let (input, objects) = fixture(Clip::Translation, false);
    let nested = env::bundle(&[("bad-signature", corrupt)]);
    let deps = env::bundle(&[("nested.bundle", nested), ("objects.bundle", objects)]);
    install(&scratch, &mut spec, &input, &deps);
    let mut request = selection(2, 3);
    request["source"]["owner"] = index(5, 10);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1,"clips":[],"clips_by_prefab":{"first.glb":{"a":[request]}}
    });
    let end = drive(&scratch, &spec);
    assert_eq!(end.code, Code::Incomplete, "{end:?}");
    let inputs = rows(&scratch, 1, "inputs");
    assert_eq!(inputs[0]["status"], "failed");
    assert_eq!(inputs[0]["reason"], "container_error");
    assert_eq!(inputs[0]["detail"], "malformed");
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(!scratch.join("sink/second.glb").exists());
}

#[test]
fn nested_gzip_probe_decode_work_discards_the_attempt() {
    let (scratch, mut spec) = job(&[]);
    let (input, objects) = fixture(Clip::Translation, false);
    let web = unityrs_testkit::web::plain("UnityWebData1.0", &[("leaf", b"raw")]);
    let mut gzip = unityrs_testkit::web::gzip(&web);
    // Each of these gzip members decodes to 64 KiB of zeros. Padding the
    // first member this way hits the decode-work cap during detection.
    let mut zeros = vec![
        0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0x02, 0x03, 0xed, 0xc1, 0x01, 0x01, 0, 0, 0, 0x80, 0x90,
        0xfe, 0xaf, 0xee, 0x08, 0x0a,
    ];
    zeros.resize(87, 0);
    zeros.extend_from_slice(&[0x6a, 0xeb, 0x8e, 0x97, 0xd7, 0, 0, 1, 0]);
    for _ in 0..65 {
        gzip.extend_from_slice(&zeros);
    }
    let deps = env::bundle(&[("web.gz", gzip), ("objects.bundle", objects)]);
    install(&scratch, &mut spec, &input, &deps);
    spec["max_decoded_bytes"] = json!(1 << 20);
    spec["publish_incomplete"] = json!(true);
    // The first prefab is staged before the second walks through the gzip
    // container and its leaf to locate the controller at index 7.
    let mut request = selection(12, 13);
    request["source"]["owner"] = index(7, 10);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1,"clips":[],"clips_by_prefab":{"second.glb":{"a":[request]}}
    });
    let end = drive(&scratch, &spec);
    assert_eq!(end.code, Code::Budget, "{end:?}");
    let inputs = rows(&scratch, 1, "inputs");
    assert_eq!(inputs[0]["status"], "failed");
    assert_eq!(inputs[0]["reason"], "limit(decode_work, 4194304)");
    assert!(!scratch.join("sink/first.glb").exists());
    assert!(!scratch.join("sink/second.glb").exists());
}

fn check_nested_serialized_file(needed: bool) {
    let (scratch, mut spec) = job(&[]);
    let (input, objects) = fixture(
        if needed {
            Clip::Outside
        } else {
            Clip::Translation
        },
        false,
    );
    let corrupt = corrupt_serialized_file();
    let nested = env::bundle(&[("CAB-outside", corrupt)]);
    let deps = env::bundle(&[("nested.bundle", nested), ("objects.bundle", objects)]);
    install(&scratch, &mut spec, &input, &deps);
    let mut request = selection(2, 3);
    request["source"]["owner"] = index(7, 10);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1,"clips":[],"clips_by_prefab":{"first.glb":{"a":[request]}}
    });
    let end = drive(&scratch, &spec);
    assert_eq!(
        end.code,
        if needed {
            Code::Incomplete
        } else {
            Code::Complete
        },
        "{end:?}"
    );
    let inputs = rows(&scratch, 1, "inputs");
    if needed {
        assert_eq!(end.code, Code::Incomplete, "{inputs:?}");
        assert_eq!(inputs[0]["status"], "failed");
        assert_eq!(inputs[0]["reason"], "container_error");
        assert_eq!(inputs[0]["detail"], "out_of_bounds");
    } else {
        assert_eq!(end.code, Code::Complete, "{inputs:?}");
    }
    assert_eq!(scratch.join("sink/first.glb").exists(), !needed);
    assert_eq!(scratch.join("sink/second.glb").exists(), !needed);
}

fn corrupt_serialized_file() -> Vec<u8> {
    // Unity 2022 (format 22), with version metadata but no type table.
    let mut bytes = vec![0; 128];
    bytes[11] = 22;
    bytes[23] = 80;
    bytes[31] = 128;
    bytes[39] = 128;
    bytes[48..59].copy_from_slice(b"2022.3.0f1\0");
    bytes[64] = 100;
    bytes
}

#[test]
fn invalid_static_batch_range_degrades_a_prefab_with_a_remaining_mesh() {
    let (scratch, mut spec) = job(&[]);
    let (input, deps) = fixture(Clip::BadStaticRangeWithMesh, false);
    install(&scratch, &mut spec, &input, &deps);
    spec["outputs"]["glb"]["profile"] = json!({"profile_version":1,"clips":[]});
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    assert_eq!(rows(&scratch, 1, "inputs")[0]["status"], "complete");
    assert_eq!(head(&scratch)["run"]["glb_degraded"], 1);
    let diagnostics = rows(&scratch, 1, "diagnostics");
    let issue = diagnostics
        .iter()
        .find(|row| row["kind"] == "glb_degraded")
        .unwrap();
    assert_eq!(issue["key"], "first.glb");
    assert_eq!(issue["reason"], "static batch submesh range");
    assert!(scratch.join("sink/first.glb").exists());
    assert!(scratch.join("sink/second.glb").exists());
}
