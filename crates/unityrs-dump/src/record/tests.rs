use super::*;
use crate::Levels;

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

/// `first_diff` (`docs/CANONICAL_DUMP.md` section 4.3): the first offset at
/// which the resave and the raw bytes differ, or the shorter length when
/// one is a prefix of the other, in either order. The oracle's own answers
/// are replayed by `tests/oracle_fixtures.rs`.
#[test]
fn first_difference_is_the_offset_or_the_shorter_length() {
    assert_eq!(first_difference(b"abc", b"abd"), 2);
    assert_eq!(first_difference(b"xbc", b"abc"), 0);
    assert_eq!(first_difference(b"ab", b"abc"), 2);
    assert_eq!(first_difference(b"abcd", b"ab"), 2);
    assert_eq!(first_difference(b"", b"a"), 0);
    assert_eq!(first_difference(b"", b""), 0);
}

/// A limit refusal at the end of a text longer than the note is kept
/// whole after the text's first characters; one the note already holds,
/// or none, leaves `note_of`'s note as it is.
#[test]
fn a_note_keeps_its_limit_refusal_whole() {
    /// `prefix`, then the refusal's text.
    struct Wrapped<'a>(&'a str, LimitError);
    impl std::fmt::Display for Wrapped<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}{}", self.0, self.1)
        }
    }
    let l = LimitError {
        kind: unityrs_core::LimitKind::Inflate,
        value: 5000,
        max: 1000,
    };
    let tail = "limit Inflate: 5000 > 1000";
    for n in [
        0,
        200,
        NOTE_MAX - tail.len(),
        NOTE_MAX - tail.len() + 1,
        5000,
    ] {
        let prefix: String = "\u{e9}x".chars().cycle().take(n).collect();
        let e = Wrapped(&prefix, l);
        let kept = note_keeping(&e, Some(&l));
        assert!(kept.chars().count() <= NOTE_MAX, "{n}");
        if n + tail.len() <= NOTE_MAX {
            assert_eq!(kept, format!("{prefix}{tail}"), "{n}");
        } else {
            let head: String = prefix.chars().take(NOTE_MAX - 3 - tail.len()).collect();
            assert_eq!(kept, format!("{head}...{tail}"), "{n}");
            assert_eq!(kept.chars().count(), NOTE_MAX);
        }
        assert_eq!(note_keeping(&e, None), note_of(&e), "{n}");
    }
    // A refusal the cut leaves whole stays where it is.
    let e = format!("{tail}: {}", "y".repeat(400));
    assert_eq!(note_keeping(&e, Some(&l)), note(&e));
}

/// `note_of` formats only the prefix the note keeps, and keeps what `note`
/// of the whole text keeps, at every length around the cut.
#[test]
fn a_note_of_an_error_is_the_note_of_its_whole_text() {
    /// A text written in pieces.
    struct Pieces<'a>(&'a [&'a str]);
    impl std::fmt::Display for Pieces<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.iter().try_for_each(|p| f.write_str(p))
        }
    }
    /// A text without end: only what the note keeps is ever written.
    struct Endless;
    impl std::fmt::Display for Endless {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            loop {
                f.write_str("long")?;
            }
        }
    }
    for n in [
        0,
        1,
        NOTE_MAX - 1,
        NOTE_MAX,
        NOTE_MAX + 1,
        NOTE_MAX + 2,
        5000,
    ] {
        let text: String = "\u{e9}x".chars().cycle().take(n).collect();
        assert_eq!(note_of(&text), note(&text), "{n}");
        let mid = text
            .char_indices()
            .nth(n / 2)
            .map_or(text.len(), |(i, _)| i);
        let (a, b) = text.split_at(mid);
        assert_eq!(note_of(&Pieces(&[a, b])), note(&text), "{n}");
    }
    let n = note_of(&Endless);
    assert_eq!(n.chars().count(), NOTE_MAX);
    assert!(n.starts_with("longlong") && n.ends_with("..."));
}
