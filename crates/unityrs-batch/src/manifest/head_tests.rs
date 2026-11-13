use super::*;

fn head() -> String {
    r#"{"manifest_version":1,"run_id":"r","attempt":2,"write_seq":0,"binary_version":"future","spec_version":1,"spec_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#.into()
}
#[test]
fn accepts_future_versions_and_skips_unknown_values_without_a_tree() {
    let text = head().replace("\"manifest_version\":1", "\"manifest_version\":99");
    assert_eq!(recognize(text.as_bytes(), "r").unwrap().attempt, 2);
    let text = text.replace(
        '{',
        "{\"unknown\":[null,true,false,{\"escaped\":\"\\ud83d\\ude00\"},-1.2e+3],",
    );
    assert_eq!(recognize(text.as_bytes(), "r").unwrap().write_seq, 0);
    let text = head()
        .replace("run_id", "run_\\u0069d")
        .replace("\"r\"", "\"\\u0072\"");
    assert!(recognize(text.as_bytes(), "r").is_ok());
}
#[test]
fn rejects_wrong_missing_duplicate_and_mistyped_identity() {
    assert_eq!(
        recognize(head().as_bytes(), "other"),
        Err(Unreadable::NotAManifest)
    );
    for (old, new) in [
        ("\"attempt\":2", "\"attempt\":2.0"),
        ("\"attempt\":2", "\"attempt\":4294967296"),
        ("\"attempt\":2", "\"attempt\":2147483648"),
        ("\"attempt\":2", "\"attempt\":0"),
        ("\"attempt\":2", "\"attempt\":-1"),
        ("\"attempt\":2,", ""),
        ("\"attempt\":2", "\"attempt\":2,\"attempt\":1"),
        ("\"write_seq\":0", "\"write_seq\":false"),
        ("\"manifest_version\":1", "\"manifest_version\":0"),
        ("\"spec_version\":1", "\"spec_version\":null"),
        ("\"binary_version\":\"future\"", "\"binary_version\":1"),
        ("aaaa", "AAaa"),
    ] {
        assert_eq!(
            recognize(head().replace(old, new).as_bytes(), "r"),
            Err(Unreadable::NotAManifest),
            "{new}"
        );
    }
}
#[test]
fn rejects_malformed_unknown_values_trailing_bytes_utf8_and_depth() {
    for value in [
        "[1,]",
        "{\"x\":}",
        "01",
        "1e",
        "\"\\ud800\"",
        "\"\\udc00\"",
        "\"\\q\"",
        "\"\n\"",
        "NaN",
    ] {
        let text = head().replacen('{', &format!("{{\"x\":{value},"), 1);
        assert_eq!(
            recognize(text.as_bytes(), "r"),
            Err(Unreadable::NotAManifest),
            "{value}"
        );
    }
    assert!(recognize(format!("{}x", head()).as_bytes(), "r").is_err());
    assert!(recognize(b"\xff", "r").is_err());
    let text = format!(
        "{{\"x\":{}0{},{}",
        "[".repeat(128),
        "]".repeat(128),
        &head()[1..]
    );
    assert!(recognize(text.as_bytes(), "r").is_err());
    assert_eq!(
        recognize(&vec![b' '; HEAD_LIMIT + 1], "r"),
        Err(Unreadable::TooLarge)
    );
    let mut text = head();
    text.extend(std::iter::repeat_n(' ', HEAD_LIMIT - text.len()));
    assert!(recognize(text.as_bytes(), "r").is_ok());
}
