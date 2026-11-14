//! Testkit-built synthetic files for `tools/fixtures/obj_export.py`; no TPK.
#![allow(clippy::format_collect, reason = "test protocol hex encoding")]
#[path = "../tests/support/export_cases.rs"]
mod cases;
fn main() {
    let cases:Vec<_>=cases::cases().into_iter().map(|c| {
        let bytes=unityrs_testkit::scene3d::file(unityrs_testkit::scene3d::Layout::Unity6000,&[],&c.objects).unwrap();
        let hex=|bytes:&[u8]|bytes.iter().map(|b|format!("{b:02x}")).collect::<String>();
        serde_json::json!({"case":c.name,"kind":c.kind,"input":hex(&bytes),"target":c.target,"materials":c.names.map(|ns|ns.into_iter().map(|n|n.map(|n|hex(&n))).collect::<Vec<_>>())})
    }).collect();
    println!("{}", serde_json::to_string(&cases).unwrap());
}
