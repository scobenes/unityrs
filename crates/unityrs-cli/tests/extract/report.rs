//! Compare raw JSON too: `serde_json` refuses the lone surrogate escapes that
//! correctly preserve non-UTF-8 filenames. Fixture paths are byte hex.
use serde_json::Value;
use std::{os::unix::ffi::OsStrExt, path::Path};
use unityrs_export::pyjson;
struct Json(Vec<u8>);
impl pyjson::Sink for Json {
    fn put(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}
fn string(bytes: &[u8]) -> String {
    let mut out = Json(Vec::new());
    pyjson::string(bytes, &mut out);
    String::from_utf8(out.0).unwrap()
}
pub(super) fn json(report: &Value, input: &Path) -> Vec<u8> {
    let rows =
        |key: &str| {
            report[key].as_array().unwrap().iter().map(|row| {
        let path = super::unhex(row["path"].as_str().unwrap());
        assert!(unityrs_export::extractor::paths::validate(&path).is_ok());
        let tail = if key == "written" { format!("\"size\":{}",row["size"]) }
            else { format!("\"reason\":{}",row["reason"]) };
        let file = row["serialized_file"].as_str().unwrap().as_bytes();
        let file = if file == b"synthetic.assets" { input.as_os_str().as_bytes() } else { file };
        format!("{{\"path\":{},\"serialized_file\":{},\"path_id\":{},\"class_id\":{},{tail}}}",
            string(&path), string(file),
            row["path_id"], row["class_id"])
    }).collect::<Vec<_>>().join(",")
        };
    format!(
        "{{\"written\":[{}],\"failures\":[{}]}}\n",
        rows("written"),
        rows("failures")
    )
    .into_bytes()
}
