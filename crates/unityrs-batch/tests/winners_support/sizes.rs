//! Size gates compare B6 encodings with the actual B4 counted work.
use super::{codec, json::Json, member, rows};
use crate::{
    phase::Account,
    records::{self, Record, Status, Text},
    spool::{Counter, Directory, Host},
    work::Work,
};
use std::sync::Arc;
use unityrs_testkit::scratch::Scratch;

#[test]
fn member_frames_rows_and_lists_fit_their_counted_member_work() {
    let scratch = Scratch::new("member-size");
    let account = Account::new(64 << 20);
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let directory = Directory::create(scratch.join("records"), counter, Arc::new(Host)).unwrap();
    let mut writer = records::Writer::new(&directory, account.meter()).unwrap();
    let mut work = Work::new(u64::MAX);
    let mut codec = member::Codec::new(account.meter()).unwrap();
    let mut json = Json::new(&account).unwrap();
    let long = format!(
        "{}/{}/{}/{}",
        "\"".repeat(255),
        "\\".repeat(255),
        "x".repeat(255),
        "x".repeat(255)
    );
    let repaired = "�".repeat(1024);
    for archive in ["a", long.as_str()] {
        for path in [
            "p",
            long.as_str(),
            repaired.as_str(),
            "\u{0}".repeat(1024).as_str(),
        ] {
            for reason in [
                None,
                Some(crate::reason::LeftOut::DuplicateMember),
                Some(crate::reason::LeftOut::PrefixConflict),
                Some(crate::reason::LeftOut::UnwritablePath(
                    crate::reason::KeyRule::TooLong,
                )),
            ] {
                if reason.is_none() && (path.len() > 1024 || path.contains('\0')) {
                    continue;
                }
                let m = member::Member {
                    archive,
                    index: 255,
                    path,
                    key: &long,
                    input: "a",
                    md5: [1; 16],
                    reason,
                    cut: true,
                    source: u32::MAX,
                    plan_index: u64::MAX,
                    size: u64::MAX,
                    crc32: u32::MAX,
                };
                // A fresh writer keeps every single line below its flush size,
                // so this delta is precisely B4's largest final-row work.
                let before = work.spooled_part();
                writer
                    .member(
                        archive,
                        records::Member {
                            archive: 255,
                            path: Text::Prepared(path, true),
                            key: &long,
                            input: "a",
                            md5: &m.md5,
                            reason,
                            cut: true,
                        },
                        &mut work,
                    )
                    .unwrap();
                let work_bytes = work.spooled_part() - before;
                // A flush can only increase this delta; independently measure
                // the same member with an empty writer when that happens.
                let fresh = Directory::create(
                    scratch.join(format!("one-{before}")),
                    Counter::new(u64::MAX, 0, 4096).unwrap(),
                    Arc::new(Host),
                )
                .unwrap();
                let mut one = records::Writer::new(&fresh, account.meter()).unwrap();
                let mut exact = Work::new(u64::MAX);
                one.member(
                    archive,
                    records::Member {
                        archive: 255,
                        path: Text::Prepared(path, true),
                        key: &long,
                        input: "a",
                        md5: &m.md5,
                        reason,
                        cut: true,
                    },
                    &mut exact,
                )
                .unwrap();
                let exact = exact.spooled_part();
                assert!(work_bytes >= exact);
                let frame = u64::try_from(codec.encode(m).unwrap().len() + 4).unwrap();
                assert!(frame <= exact, "frame={frame} counted={exact}");
                super::archives::member_row(&mut json, m).unwrap();
                let row = u64::try_from(json.end().unwrap().len()).unwrap();
                assert!(row <= exact, "row={row} counted={exact}");
                // Lists use this identical frame only when included.
                assert!(frame <= 2 * exact);
            }
        }
    }
}

#[test]
fn candidate_annotations_outputs_and_collisions_fit_source_record_work() {
    let scratch = Scratch::new("candidate-size");
    let account = Account::new(64 << 20);
    let mut codec = codec::Codec::new(account.meter()).unwrap();
    let mut json = Json::new(&account).unwrap();
    for (i, text) in [
        "x".to_owned(),
        "�".repeat(1024),
        "\0".repeat(1024),
        "\"\\".repeat(512),
    ]
    .iter()
    .enumerate()
    {
        let directory = Directory::create(
            scratch.join(format!("c{i}")),
            Counter::new(u64::MAX, 0, 4096).unwrap(),
            Arc::new(Host),
        )
        .unwrap();
        let mut writer = records::Writer::new(&directory, account.meter()).unwrap();
        let mut work = Work::new(u64::MAX);
        let hashes = crate::digest::Hashes::of(b"");
        writer
            .append(
                Record::Candidate(records::Candidate {
                    key: "k",
                    unit: Some("k"),
                    input: "a",
                    serialized_file: Text::Prepared(text, true),
                    kind: unityrs_export::ngui::OutputKind::AtlasJson,
                    path_id: i64::MIN,
                    hashes,
                    picture: None,
                    size: 0,
                    plan_index: u64::MAX,
                    cut: true,
                }),
                &mut work,
            )
            .unwrap();
        let summary = records::Summary {
            input: "a",
            status: Status::Complete,
            reason: None,
            detail: None,
            figures: Some(records::Figures::default()),
            spooled_part: 0,
            claimed_bytes: 0,
            cut: false,
        };
        writer.finish(summary, &mut work).unwrap();
        let bytes = std::fs::read(directory.path().join("records.ndjson")).unwrap();
        let j = bytes.iter().position(|&b| b == b'\n').unwrap() + 1;
        let mut read =
            records::read::Reader::new(bytes.as_slice(), "a", false, 0, account.meter()).unwrap();
        let Some(records::read::Row::Candidate(c)) = read.next(&|| false).unwrap() else {
            panic!("candidate missing")
        };
        let mut r = codec::Row::candidate(&c, 0);
        assert!(codec.encode(r).unwrap().len() + 4 <= j);
        for state in [
            codec::State::Publish,
            codec::State::Withheld,
            codec::State::UnitConflict,
            codec::State::UnitLost,
            codec::State::Prefix,
            codec::State::Archive,
            codec::State::Incomplete,
        ] {
            r.state = state;
            rows::output(&mut json, r, state).unwrap();
            let length = json.end().unwrap().len();
            assert!(length <= j + 64);
            assert!(length <= 2 * j);
        }
        for rule in [
            codec::Rule::FirstInput,
            codec::Rule::PreferMd5,
            codec::Rule::PreferInput,
            codec::Rule::Unit,
            codec::Rule::UnitConflict,
            codec::Rule::Withheld,
        ] {
            r.rule = rule;
            rows::collision(&mut json, r, r, false).unwrap();
            assert!(json.end().unwrap().len() <= j);
        }
    }
}
