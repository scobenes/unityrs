mod archive_support;
use archive_support::Scene;
use std::{fmt::Write, process::Command};
#[cfg(feature = "test-hooks")]
use unityrs_batch::phase::Account;
use unityrs_batch::{archive, digest::Hex};
use unityrs_testkit::child;

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    write!(&mut out, "{}", Hex(bytes)).unwrap();
    out
}
fn oracle(
    scene: &Scene,
    members: &[(&str, &[u8])],
    origin: u64,
    data: &[u8],
    summary: archive::Summary,
) {
    let archive = scene.scratch.join("archive.zip");
    let description = scene.scratch.join("members.json");
    let result = scene.scratch.join("summary.json");
    std::fs::write(&archive, data).unwrap();
    std::fs::write(
        &description,
        serde_json::to_vec(
            &members
                .iter()
                .map(|(name, b)| (*name, hex(b)))
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::write(&result, serde_json::to_vec(&serde_json::json!({"size":summary.size,"emitted_size":summary.emitted_size,"md5":hex(&summary.md5),"etag":summary.etag.to_string()})).unwrap()).unwrap();
    let out = child::output(
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/archive_support/check.py"
            ))
            .arg(archive)
            .arg(description)
            .arg(origin.to_string())
            .arg(result),
    )
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
fn independent_struct_encoder_and_zipfile_read_every_member() {
    let members: &[(&str, &[u8])] = &[("a/leaf", b"hello"), ("a!", b""), ("é\\\"", b"world")];
    let scene = Scene::new(members);
    let (bytes, summary) = scene.generate(7).unwrap();
    oracle(&scene, members, 0, &bytes, summary);
}
#[test]
fn central_metadata_crosses_a_digest_part_boundary() {
    // The local name+header puts the payload's end just below the part edge,
    // so replay of the central entry crosses it.
    let payload = vec![b'x'; unityrs_batch::digest::PART - 35];
    let members = [("a", payload.as_slice())];
    let scene = Scene::new(&members);
    let (bytes, summary) = scene.generate(65_533).unwrap();
    assert!(summary.etag.is_multipart());
    oracle(&scene, &members, 0, &bytes, summary);
}
#[cfg(feature = "test-hooks")]
#[test]
fn zip_offset_roundtrips_through_a_real_logical_prefix() {
    use unityrs_batch::winners::layout::{Layout, ZIP32};
    let members: &[(&str, &[u8])] = &[("a", b"x"), ("b", b"yz")];
    let scene = Scene::new(members);
    for origin in [
        ZIP32 - 66,
        ZIP32 - 65,
        ZIP32 - 64,
        ZIP32 - 33,
        ZIP32 - 32,
        ZIP32 - 31,
        ZIP32 - 1,
        ZIP32,
        ZIP32 + 1,
    ] {
        let account = Account::new(1 << 20);
        let mut layout = Layout::seed(origin, 0, 0).unwrap();
        for (name, bytes) in members {
            layout
                .member(
                    u64::try_from(name.len()).unwrap(),
                    u64::try_from(bytes.len()).unwrap(),
                )
                .unwrap();
        }
        let mut input = scene.input();
        input.size = layout.finish().unwrap().size;
        let mut stream =
            archive::open_at(scene.context(&account), input, origin, &|| false).unwrap();
        let mut bytes = Vec::new();
        let mut buffer = vec![0; 17];
        loop {
            let n = stream.read(&mut buffer, &|| false).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..n]);
        }
        let summary = stream.finish(&|| false).unwrap();
        oracle(&scene, members, origin, &bytes, summary);
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn actual_count_transitions_have_independent_tails_and_reader_roundtrips() {
    for count in [65_534, 65_535, 65_536] {
        let scene = Scene::repeated(count, b"");
        let (bytes, summary) = scene.generate(65_536).unwrap();
        let archive = scene.scratch.join("count.zip");
        let description = scene.scratch.join("count.json");
        let result = scene.scratch.join("summary.json");
        std::fs::write(&archive, bytes).unwrap();
        std::fs::write(
            &description,
            serde_json::to_vec(&serde_json::json!({"count":count})).unwrap(),
        )
        .unwrap();
        std::fs::write(&result, serde_json::to_vec(&serde_json::json!({"size":summary.size,"emitted_size":summary.emitted_size,"md5":hex(&summary.md5),"etag":summary.etag.to_string()})).unwrap()).unwrap();
        let output = child::output(
            Command::new("python3")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/archive_support/check.py"
                ))
                .arg(archive)
                .arg(description)
                .arg("0")
                .arg(result),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
