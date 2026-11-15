//! Synthetic container inputs shared by the `WebFile` oracle recorder and replay.
use serde_json::{Value, json};
use unityrs_testkit::env::{Obj, bundle, serialized, web_file};

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

pub fn inputs() -> Vec<Value> {
    let asset = serialized(&[], &[Obj::named(1, "synthetic", &[])]);
    let mut cases = Vec::new();
    for kind in ["web", "web_in_bundle", "bundle_in_web", "bundle"] {
        for outer_only in [false, true] {
            if outer_only && matches!(kind, "web" | "bundle") {
                continue;
            }
            let mut rows = vec![("CAB-test", asset.clone())];
            if !outer_only {
                rows.push(("r.resS", b"inner resource".to_vec()));
            }
            let inner = if matches!(kind, "web" | "web_in_bundle") {
                web_file(&rows)
            } else {
                bundle(&rows)
            };
            let outer = [
                ("r.resS", b"outer resource".to_vec()),
                ("nested", inner.clone()),
            ];
            let data = match kind {
                "web_in_bundle" => bundle(&outer),
                "bundle_in_web" => web_file(&outer),
                _ => inner,
            };
            for shadow_first in [false, true] {
                cases.push(json!({
                    "id": format!("{kind}-outer_only={outer_only}-shadow_first={shadow_first}"),
                    "kind": kind,
                    "outer_only": outer_only,
                    "shadow_first": shadow_first,
                    "hex": hex(&data),
                    "path": "archive:/r.resS",
                    "offset": 1,
                    "size": 5,
                    "shadow_hex": hex(b"registry shadow"),
                }));
            }
        }
    }
    cases
}
