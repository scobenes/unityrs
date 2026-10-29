use super::*;

fn sha(b: u8) -> [u8; 32] {
    [b; 32]
}

#[test]
fn header_and_end_lines() {
    assert_eq!(
        header(Levels::ALL, "\"name\":\"unityrs\""),
        "{\"rec\":\"dump\",\"format\":\"unityrs-canonical-dump\",\"version\":1,\"levels\":[\"L0\",\"L1\",\"L2\",\"L3\"],\"producer\":{\"name\":\"unityrs\"}}"
    );
    assert_eq!(end(2, 5), "{\"rec\":\"end\",\"files\":2,\"objects\":5}");
}

#[test]
fn a_detect_error_record() {
    let rec = FileRecord {
        file: vec![RawStr::from(&b"a\xffb"[..])],
        l0: L0 {
            ty: None,
            parsed_as: ParsedAs::Error,
            size: 20,
            sha256: sha(0xab),
        },
        l1: None,
        error: Some(FileError {
            kind: FileErrorKind::Detect,
            note: None,
        }),
    };
    assert_eq!(
        file_line(&rec),
        format!(
            "{{\"rec\":\"file\",\"file\":[{{\"t\":\"str\",\"hex\":\"61ff62\"}}],\"l0\":{{\"type\":\"error\",\"parsed_as\":\"error\",\"size\":20,\"sha256\":\"{}\"}},\"l1\":null,\"error\":{{\"kind\":\"detect\"}}}}",
            "ab".repeat(32)
        )
    );
}

#[test]
fn an_object_record_with_every_section() {
    let rec = ObjectRecord {
        path_id: -3,
        l1: Some(ObjectL1 {
            type_id: 0,
            class_id: 114,
            byte_start: i128::from(i64::MAX) + 144,
            byte_size: 12,
            is_destroyed: None,
            is_stripped: Some(-1),
        }),
        l2: Some(L2 {
            root_type: Some(RawStr::from("MonoBehaviour")),
            source: Some("embedded"),
            error: Some(L2Error {
                kind: L2Kind::CheckRead,
                bytes_read: Some(16),
                note: Some("Expected to read 12 bytes, but only read 16 bytes".into()),
            }),
            value: Some("{\"t\":\"dict\",\"v\":[]}".into()),
        }),
        l3: Some(L3 {
            raw_size: 12,
            raw_sha256: sha(1),
            resave: Resave::Skipped,
        }),
    };
    let line = object_line(&[RawStr::from("in"), RawStr::from("CAB-x")], &rec);
    assert_eq!(
        line,
        format!(
            "{{\"rec\":\"object\",\"file\":[\"in\",\"CAB-x\"],\"path_id\":-3,\"l1\":{{\"type_id\":0,\"class_id\":114,\"byte_start\":9223372036854775951,\"byte_size\":12,\"is_destroyed\":null,\"is_stripped\":-1}},\"l2\":{{\"root_type\":\"MonoBehaviour\",\"source\":\"embedded\",\"error\":{{\"kind\":\"check_read\",\"bytes_read\":16,\"note\":\"Expected to read 12 bytes, but only read 16 bytes\"}},\"value\":{{\"t\":\"dict\",\"v\":[]}}}},\"l3\":{{\"raw_size\":12,\"raw_sha256\":\"{}\",\"resave\":\"skipped\",\"resave_size\":null,\"resave_sha256\":null,\"first_diff\":null}}}}",
            "01".repeat(32)
        )
    );
}

#[test]
fn notes_are_cut_as_the_oracle_cuts_them() {
    assert_eq!(note("short"), "short");
    let long: String = "\u{e9}".repeat(400);
    let n = note(&long);
    assert_eq!(n.chars().count(), NOTE_MAX);
    assert!(n.ends_with("..."));
}

#[test]
fn first_difference_is_the_offset_or_the_shorter_length() {
    assert_eq!(first_difference(b"abc", b"abd"), 2);
    assert_eq!(first_difference(b"ab", b"abc"), 2);
    assert_eq!(first_difference(b"", b""), 0);
}
