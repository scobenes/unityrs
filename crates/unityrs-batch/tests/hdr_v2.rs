//! HDR content errors through spec validation, decode and batch publication.

mod common;
mod publication_support;

use std::{fs, sync::Arc};

use serde_json::{Value, json};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    reason::Code,
    run,
};
use unityrs_testkit::{ngui, scratch::Scratch};

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}

fn rows(scratch: &Scratch, kind: &str) -> Vec<Value> {
    fs::read_to_string(scratch.join(format!("sink/_unityrs/r1/1/{kind}.0.ndjson")))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn malformed_hdr_skips_one_object_and_publishes_other_outputs_and_inputs() {
    for format in [24, 66, 67, 68, 69, 70, 71] {
        let scratch = Scratch::new("hdr-v2");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        let mut bad = vec![0; 16];
        if format == 24 {
            bad[0] = 19;
        }
        let objects = [
            ngui::texture(1, b"before", 4, (1, 1), vec![255; 4]),
            ngui::texture(2, b"bad", format, (1, 1), bad),
            ngui::texture(3, b"after", 4, (1, 1), vec![255; 4]),
        ];
        let input =
            unityrs_testkit::env::bundle(&[("CAB-hdr", ngui::file(&[], &objects, true).unwrap())]);
        fs::write(scratch.join("a.bundle"), input).unwrap();
        let other = unityrs_testkit::env::bundle(&[(
            "CAB-other",
            ngui::file(
                &[],
                &[ngui::texture(4, b"other", 4, (1, 1), vec![255; 4])],
                true,
            )
            .unwrap(),
        )]);
        fs::write(scratch.join("b.bundle"), other).unwrap();
        let mut spec = common::base();
        spec["spec_version"] = json!(2);
        spec["threads"] = json!(1);
        spec["inputs"] = json!([
            {"id": "a", "path": scratch.join("a.bundle")},
            {"id": "b", "path": scratch.join("b.bundle")}
        ]);
        spec["dependencies"] = json!([]);
        spec["spool"]["dir"] = json!(scratch.join("spool"));
        spec["sink"]["root"] = json!(scratch.join("sink"));
        spec["outputs"]["formats"] = json!([
            "RGBA32",
            "BC6H",
            "ASTC_HDR_4x4",
            "ASTC_HDR_5x5",
            "ASTC_HDR_6x6",
            "ASTC_HDR_8x8",
            "ASTC_HDR_10x10",
            "ASTC_HDR_12x12"
        ]);
        let prepared = phase1::prepare(common::accepted(&spec)).unwrap();
        let service = Service::start(
            fs::File::create(scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        let result = run::drive(
            prepared,
            123,
            &service.sender(),
            &Space,
            Arc::new(publication_support::Portable),
        )
        .unwrap();
        assert_eq!(result.code, Code::CompleteWithSkips, "format {format}");
        assert_eq!(result.status, Some("complete_with_skips"));
        let skips = rows(&scratch, "skips");
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0]["path_id"], 2);
        assert_eq!(skips[0]["reason"], "decode_error");
        assert_eq!(skips[0]["detail"], "malformed");
        assert!(!scratch.join("sink/a/bad.png").exists());
        for key in ["a/before.png", "a/after.png", "b/other.png"] {
            let bytes = fs::read(scratch.join(format!("sink/{key}"))).unwrap();
            assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
        let head: Value = serde_json::from_slice(
            &fs::read(scratch.join("sink/_unityrs/r1/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(head["exit_code"], Code::CompleteWithSkips.value());
        assert_eq!(head["stop_reason"], Value::Null);
        assert_eq!(head["counts"]["outputs_written"], 3);
        assert_eq!(head["rows"]["skips"], 1);
        let inputs = rows(&scratch, "inputs");
        let statuses: Vec<_> = inputs
            .iter()
            .map(|r| (r["id"].as_str().unwrap(), r["status"].as_str().unwrap()))
            .collect();
        assert_eq!(statuses, [("a", "complete_with_skips"), ("b", "complete")]);
    }
}
