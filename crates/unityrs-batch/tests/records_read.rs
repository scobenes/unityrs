use std::{io::Cursor, sync::Arc};
use unityrs_batch::{
    phase::{Account, Error},
    records::read::{Reader, Row},
};

fn summary() -> String {
    concat!("{\"type\":\"input_summary\",\"id\":\"input\",\"status\":\"complete\",",
        "\"reason\":null,\"detail\":null,\"input_peak\": 0,\"dependency_peak\": 0,",
        "\"evictions\": 0,\"export_work\": 0,\"decode_work_peak\": 0,\"ms\": 0,",
        "\"skips\": 0,\"unresolved_scripts\": 0,\"spooled_part\": 0,\"claimed_bytes\": 0,\"cut\":false}\n").into()
}
fn candidate() -> String {
    concat!(
        "{\"type\":\"candidate\",\"key\":\"a/é\",\"unit\":null,\"input\":\"input\",",
        "\"serialized_file\":\"\\ud83d\\ude00\\u0000\",\"kind\":\"atlas_json\",\"path_id\":-9,",
        "\"md5\":\"00000000000000000000000000000000\",\"crc32\":17,\"visible_hash\":null,",
        "\"mode\":null,\"size\":1,\"etag\":null,\"file\":\"o18446744073709551615\",\"cut\":true}\n"
    )
    .into()
}
#[test]
fn typed_reader_preserves_repaired_text_and_summary_whitespace() {
    let account = Account::new(64 << 20);
    let mut reader = Reader::new(
        Cursor::new(candidate() + &summary()),
        "input",
        false,
        2,
        account.meter(),
    )
    .unwrap();
    let Some(Row::Candidate(c)) = reader.next(&|| false).unwrap() else {
        panic!("candidate")
    };
    assert_eq!(c.key, "a/é");
    assert_eq!(c.serialized_file, "😀\0");
    assert_eq!(c.plan_index, u64::MAX);
    assert_eq!(c.md5, [0; 16]);
    assert!(c.cut);
    assert!(matches!(
        reader.next(&|| false).unwrap(),
        Some(Row::Summary(_))
    ));
    assert!(reader.next(&|| false).unwrap().is_none());
    drop(reader);
    assert_eq!(account.snapshot().input.used, 0);
}
#[test]
fn corrupt_committed_records_are_internal() {
    let cases = [
        String::new(),
        candidate(),
        summary() + &summary(),
        candidate().replace("o18446744073709551615", "o01") + &summary(),
        candidate().replace("atlas_json", "unknown") + &summary(),
        candidate().replace(
            "00000000000000000000000000000000",
            "G0000000000000000000000000000000",
        ) + &summary(),
        candidate().replace("\\ud83d\\ude00", "\\ud83d") + &summary(),
        candidate().replace("\"cut\":true", "\"cut\":true,\"cut\":false") + &summary(),
        summary().replace("\"complete\"", "\"failed\""),
        " ".repeat(32 * 1024) + "\n",
        summary().trim_end().into(),
    ];
    for bytes in cases {
        let account = Account::new(64 << 20);
        let mut reader =
            Reader::new(Cursor::new(bytes), "input", false, 0, account.meter()).unwrap();
        loop {
            match reader.next(&|| false) {
                Err(e) => {
                    assert_eq!(e, Error::internal());
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("accepted corruption"),
            }
        }
    }
}

#[test]
fn read_errors_have_no_input_culprit_and_stops_do_not_read() {
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        }
    }
    let account = Arc::new(Account::new(64 << 20));
    let mut reader = Reader::new(Broken, "input", false, 0, account.meter()).unwrap();
    assert_eq!(reader.next(&|| true).unwrap_err(), Error::Stopped);
    assert!(
        matches!(reader.next(&|| false), Err(Error::Run(f)) if f.reason == unityrs_batch::attempt::ReasonCode::HostIo)
    );
}

#[test]
fn member_indices_identities_and_digests_are_checked_without_a_table() {
    let member = concat!(
        "{\"type\":\"member\",\"archive\":0,\"path\":\"member\",\"key\":\"a/é\",",
        "\"input\":\"input\",\"md5\":\"00000000000000000000000000000000\",\"reason\":null,\"cut\":false}\n"
    );
    let account = Account::new(64 << 20);
    let mut good = Reader::new(
        Cursor::new(candidate() + member + &summary()),
        "input",
        false,
        1,
        account.meter(),
    )
    .unwrap();
    assert!(matches!(
        good.next(&|| false).unwrap(),
        Some(Row::Candidate(_))
    ));
    assert!(
        matches!(good.next(&|| false).unwrap(), Some(Row::Member(m)) if m.archive == 0 && m.md5 == [0;16])
    );
    assert!(matches!(
        good.next(&|| false).unwrap(),
        Some(Row::Summary(_))
    ));
    drop(good);
    for invalid in [
        member.replace("\"archive\":0", "\"archive\":1"),
        member.replace("\"a/é\"", "\"other\""),
        member.replace(
            "00000000000000000000000000000000",
            "10000000000000000000000000000000",
        ),
        member.replace("\"input\":\"input\"", "\"input\":\"other\""),
    ] {
        let mut reader = Reader::new(
            Cursor::new(candidate() + &invalid + &summary()),
            "input",
            false,
            1,
            account.meter(),
        )
        .unwrap();
        reader.next(&|| false).unwrap();
        assert_eq!(reader.next(&|| false).unwrap_err(), Error::internal());
    }
    let mut orphan = Reader::new(
        Cursor::new(member.to_owned() + &summary()),
        "input",
        false,
        1,
        account.meter(),
    )
    .unwrap();
    assert_eq!(orphan.next(&|| false).unwrap_err(), Error::internal());
}

#[test]
fn exact_line_bound_failure_only_summary_and_repaired_text_are_preserved() {
    let account = Account::new(64 << 20);
    let bytes = " ".repeat(unityrs_batch::records::LINE - summary().len()) + &summary();
    assert_eq!(bytes.len(), unityrs_batch::records::LINE);
    let mut reader = Reader::new(Cursor::new(bytes), "input", false, 0, account.meter()).unwrap();
    assert!(matches!(
        reader.next(&|| false).unwrap(),
        Some(Row::Summary(_))
    ));
    drop(reader);
    let failure = summary()
        .replace("\"complete\"", "\"failed\"")
        .replace("\"reason\":null", "\"reason\":\"source_missing\"");
    let mut reader = Reader::new(
        Cursor::new(failure.clone()),
        "input",
        true,
        0,
        account.meter(),
    )
    .unwrap();
    assert!(
        matches!(reader.next(&|| false).unwrap(), Some(Row::Summary(s)) if s.status == unityrs_batch::records::Status::Failed)
    );
    drop(reader);
    let mut reader = Reader::new(
        Cursor::new(candidate() + &failure),
        "input",
        true,
        0,
        account.meter(),
    )
    .unwrap();
    assert_eq!(reader.next(&|| false).unwrap_err(), Error::internal());
    drop(reader);
    let repaired = "�".repeat(1024);
    let bytes = candidate().replace("\\ud83d\\ude00\\u0000", &repaired) + &summary();
    let mut reader = Reader::new(Cursor::new(bytes), "input", false, 0, account.meter()).unwrap();
    assert!(
        matches!(reader.next(&|| false).unwrap(), Some(Row::Candidate(c)) if c.serialized_file == repaired && c.cut)
    );
}

#[test]
fn c1_code_points_are_valid_key_bytes_and_ascii_controls_are_rejected() {
    let account = Account::new(64 << 20);
    let bytes = candidate().replace("a/é", "a/\u{85}") + &summary();
    let mut reader = Reader::new(Cursor::new(bytes), "input", false, 0, account.meter()).unwrap();
    assert!(
        matches!(reader.next(&|| false).unwrap(), Some(Row::Candidate(c)) if c.key == "a/\u{85}")
    );
    drop(reader);
    let bytes = candidate().replace("a/é", "a/\\u007f") + &summary();
    let mut reader = Reader::new(Cursor::new(bytes), "input", false, 0, account.meter()).unwrap();
    assert_eq!(reader.next(&|| false).unwrap_err(), Error::internal());
}

#[test]
fn candidate_modes_follow_the_spec_including_luminance() {
    let account = Account::new(64 << 20);
    for mode in ["L", "LA", "RGB", "RGBA"] {
        let bytes = candidate()
            .replace("atlas_json", "texture")
            .replace("\"mode\":null", &format!("\"mode\":\"{mode}\""))
            .replace(
                "\"visible_hash\":null",
                &format!("\"visible_hash\":\"{}\"", "0".repeat(64)),
            )
            + &summary();
        let mut reader =
            Reader::new(Cursor::new(bytes), "input", false, 0, account.meter()).unwrap();
        let Some(Row::Candidate(c)) = reader.next(&|| false).unwrap() else {
            panic!("candidate")
        };
        assert_eq!(c.mode, Some(mode));
        assert_eq!(c.visible_hash, Some([0; 32]));
        assert!(matches!(
            reader.next(&|| false).unwrap(),
            Some(Row::Summary(_))
        ));
        assert!(reader.next(&|| false).unwrap().is_none());
    }
    assert_eq!(account.snapshot().input.used, 0);
}
