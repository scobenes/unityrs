//! X1's synthetic path: pinned decoder/selection cases wrapped in staged bundles.
mod attempt_support;
use attempt_support::*;
use serde_json::{Value as J, json};
use std::fs;
use unityrs_batch::attempt::State;
use unityrs_export::ngui::NoHooks;
use unityrs_testkit::{
    Rng,
    ngui::{V, file_on, set_field, texture, texture_streamed},
    scratch::Scratch,
};
fn build(spec: &J) -> Vec<u8> {
    if let Some(hex) = spec["hex"].as_str() {
        return (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
    }
    if let Some(n) = spec["zeros"].as_u64() {
        return vec![0; usize::try_from(n).unwrap()];
    }
    let n = usize::try_from(spec["len"].as_u64().unwrap()).unwrap();
    let mut rng = Rng::new(spec["seed"].as_u64().unwrap());
    let mut out = Vec::new();
    while out.len() < n {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(n);
    if let Some(mode) = spec["bc7_mode"].as_u64() {
        let full = out.len() - out.len() % 16;
        for b in out[..full].iter_mut().step_by(16) {
            *b = if mode == 8 {
                0
            } else {
                (*b & !u8::try_from((2u16 << mode) - 1).unwrap()) | (1 << mode)
            };
        }
    }
    out
}
fn scene(case: &J, data: Vec<u8>, streamed: bool) -> unityrs_testkit::ngui::AtlasScene {
    let size = (
        i32::try_from(case["width"].as_i64().unwrap()).unwrap(),
        i32::try_from(case["height"].as_i64().unwrap()).unwrap(),
    );
    let fmt = i32::try_from(case["format"].as_i64().unwrap()).unwrap();
    let obj = if streamed {
        texture_streamed(
            1,
            b"image",
            fmt,
            size,
            "pixels.resS",
            0,
            u32::try_from(data.len()).unwrap(),
        )
    } else {
        texture(1, b"image", fmt, size, data.clone())
    };
    let obj = if let Some(blob) = case["blob"].as_array() {
        set_field(
            obj,
            "m_PlatformBlob",
            unityrs_testkit::ngui::vector(
                V::UInt8(0),
                blob.iter()
                    .map(|b| V::UInt8(u8::try_from(b.as_u64().unwrap()).unwrap()))
                    .collect(),
            ),
            None,
        )
    } else {
        obj
    };
    let file = file_on(
        Some(i32::try_from(case["platform"].as_i64().unwrap()).unwrap()),
        &[],
        &[obj],
        true,
    )
    .unwrap();
    let mut scene = simple(&[]);
    let mut entries = vec![("CAB-x", file)];
    if streamed {
        entries.push(("pixels.resS", data));
    }
    scene.bundles[0].data = unityrs_testkit::env::bundle(&entries);
    scene
}
fn detail(case: &J) -> &'static str {
    match case["message"].as_str().unwrap() {
        "Texture2D has no image data" => "no_image_data",
        "not enough image data" => "not_enough_data",
        "" => "pillow_memory_error",
        s if s.starts_with("Image size (") => "decompression_bomb",
        _ => "negative_size",
    }
}
#[test]
fn x1_decode_cases_through_disk() {
    let scratch = Scratch::new("texture-check");
    let mut jobs = Vec::new();
    let decode = fixture("../unityrs-texture/tests/fixtures/decode.json");
    let mut images = 0;
    let mut errors = 0;
    let mut empty = 0;
    // Verification copies keep only the small encoded files, allowing huge
    // error-case bundles and decoded images to drop after every attempt.
    for (i, case) in decode["cases"].as_array().unwrap().iter().enumerate() {
        let data = build(&case["data"]);
        let source = scratch.join(format!("data{i}"));
        fs::write(&source, &data).unwrap();
        jobs.push(json!({"file":source,"want":{"sha256":case["data_sha256"]}}));
        for streamed in [false, true] {
            let harness = Harness::new(&scene(case, data.clone(), streamed), "x");
            let mut cap = limits();
            cap.m = 2 << 30;
            cap.d = 2 << 30;
            cap.max_export_work = u64::MAX;
            let end = harness.run(cap, &NoHooks);
            clear(&end);
            assert_eq!(
                end.state,
                State::Done,
                "{} / streamed={streamed}: {end:?}",
                case["id"]
            );
            let rows = harness.rows();
            if case["width"] == 0 || case["height"] == 0 {
                let skip = rows.iter().find(|r| r["type"] == "skip").unwrap();
                assert_eq!(skip["reason"], "empty_image");
                empty += 1;
            } else if !case["raises"].is_null() {
                let skip = rows.iter().find(|r| r["type"] == "skip").unwrap();
                assert_eq!(skip["reason"], "decode_error", "{}", case["id"]);
                assert_eq!(skip["detail"], detail(case), "{}", case["id"]);
                errors += 1;
                assert_eq!(end.outputs, 0);
            } else {
                let row = rows
                    .iter()
                    .find(|r| r["type"] == "candidate")
                    .unwrap_or_else(|| panic!("{}: {rows:?}", case["id"]));
                let output = scratch.join(format!("image{i}-{}", u8::from(streamed)));
                fs::write(&output, harness.output(row)).unwrap();
                let mut want =
                    json!({"mode":case["mode"],"size":case["size"],"pixels_sha256":case["sha256"]});
                if let Some(pixels) = case.get("pixels") {
                    want["pixels"] = pixels.clone();
                }
                jobs.push(json!({"file":output,"want":want}));
                images += 1;
            }
        }
    }
    assert_eq!((images, errors, empty), (514, 412, 120));
    check_files(&scratch, &jobs);
}

#[test]
fn x1_selection_cases_through_disk() {
    let select = fixture("../unityrs-texture/tests/fixtures/select.json");
    for case in select["cases"].as_array().unwrap() {
        // Give newly admitted Switch layouts complete GOB storage. The
        // fixture's bounded exponents are 0, 1 and 4; the others are hostile.
        let hostile_gobs = case["platform"] == 38
            && case["blob"].as_array().is_some_and(|b| {
                b.get(8..12).is_some_and(|v| {
                    u32::from_le_bytes(std::array::from_fn(|i| {
                        u8::try_from(v[i].as_u64().unwrap()).unwrap()
                    })) >= 32
                })
            });
        let harness = Harness::new(&scene(case, vec![0; 8192], false), "x");
        let end = harness.run(limits(), &NoHooks);
        clear(&end);
        assert_eq!(end.state, State::Done, "{case}: {end:?}");
        let rows = harness.rows();
        let selected = case["selected"].as_str().unwrap();
        if selected.starts_with("export(")
            && (case["width"].as_i64().unwrap() < 0 || case["height"].as_i64().unwrap() < 0)
        {
            let skip = rows.iter().find(|r| r["type"] == "skip").unwrap();
            assert_eq!(skip["reason"], "decode_error");
            assert_eq!(skip["detail"], "negative_size");
        } else if selected.starts_with("export(") && hostile_gobs {
            let skip = rows.iter().find(|r| r["type"] == "skip").unwrap();
            assert_eq!(skip["reason"], "decode_error");
            assert_eq!(skip["detail"], "read");
        } else if selected.starts_with("export(") {
            assert_eq!(end.outputs, 1, "{case}");
        } else {
            assert_eq!(
                rows.iter().find(|r| r["type"] == "skip").unwrap()["reason"],
                selected,
                "{case}"
            );
        }
    }
    assert_eq!(select["cases"].as_array().unwrap().len(), 723);
}

#[test]
fn x1_platform_images_flow_through_the_shared_decoder() {
    let doc = fixture("../unityrs-texture/tests/fixtures/platform.json");
    let scratch = Scratch::new("platform-images");
    let mut jobs = Vec::new();
    for (i, row) in doc["textures"].as_array().unwrap().iter().enumerate() {
        if ![3, 4, 10, 12, 25].contains(&row["format"].as_i64().unwrap()) || row["mode"].is_null() {
            continue;
        }
        let mut case = row.clone();
        if let Some(blob) = row["blob"].as_str() {
            case["blob"] = json!(build(&json!({"hex":blob})));
        }
        let data = build(&json!({"hex":row["data"]}));
        let harness = Harness::new(&scene(&case, data, false), "x");
        let end = harness.run(limits(), &NoHooks);
        clear(&end);
        assert_eq!(end.state, State::Done, "{}", row["id"]);
        assert_eq!(end.outputs, 1, "{}", row["id"]);
        let rows = harness.rows();
        let made = rows.iter().find(|r| r["type"] == "candidate").unwrap();
        let output = scratch.join(format!("image{i}"));
        fs::write(&output, harness.output(made)).unwrap();
        jobs.push(json!({"file":output, "want":{"mode":row["mode"],"size":row["size"],"pixels_sha256":row["sha256"]}}));
    }
    assert_eq!(jobs.len(), 44);
    check_files(&scratch, &jobs);
}

#[test]
fn a_malformed_pvrtc_grid_skips_one_texture_and_exports_the_next() {
    let scene = simple(&[
        texture(1, b"invalid-grid", 32, (12, 12), vec![0; 72]),
        texture(2, b"next", 4, (1, 1), vec![1, 2, 3, 4]),
    ]);
    let mut harness = Harness::new(&scene, "x");
    harness.options.formats = unityrs_export::ngui::Formats::DECODED;
    let end = harness.run(limits(), &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Done, "{end:?}");
    assert_eq!(end.outputs, 1);
    let rows = harness.rows();
    let skip = rows.iter().find(|r| r["type"] == "skip").unwrap();
    assert_eq!(skip["reason"], "decode_error");
    assert_eq!(skip["detail"], "read");
    let output = rows.iter().find(|r| r["type"] == "candidate").unwrap();
    assert_eq!(output["key"], "next.png");
    assert!(harness.output(output).starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[cfg(feature = "test-hooks")]
#[test]
fn injected_decoder_panic_is_contained_and_next_output_succeeds() {
    use std::cell::Cell;
    use unityrs_batch::{attempt::Hooks, reason::Code};
    struct Panic(Cell<bool>);
    impl unityrs_export::ngui::Hooks for Panic {
        fn decoder_fault(&self) {
            assert!(self.0.replace(true), "{}é", "a".repeat(1023));
        }
    }
    impl Hooks for Panic {}
    let scene = simple(&[
        texture(1, b"first", 4, (1, 1), vec![1, 2, 3, 4]),
        texture(2, b"second", 4, (1, 1), vec![5, 6, 7, 8]),
    ]);
    let h = Harness::new(&scene, "x");
    let end = h.run(limits(), &Panic(Cell::new(false)));
    clear(&end);
    assert_eq!(end.state, State::Done);
    assert_eq!(end.code, Code::CompleteWithSkips);
    let rows = h.rows();
    let skip = rows.iter().find(|r| r["type"] == "skip").unwrap();
    assert_eq!(skip["reason"], "decoder_panic");
    assert_eq!(skip["detail"], "a".repeat(1023));
    assert_eq!(skip["cut"], true);
    let output = rows.iter().find(|r| r["type"] == "candidate").unwrap();
    assert_eq!(output["key"], "second.png");
    assert_eq!(output["file"], "o1");
    assert!(!h.directory.path().join("o0").exists());
    assert_ne!(h.output(output), Vec::<u8>::new());
}
