//! Real borrowed synthetic bytes -> files -> parsed typed NDJSON.
mod artifact_support;
use artifact_support::{Faults, meter, summary};
use std::sync::{Arc, atomic::Ordering};
use unityrs_batch::{
    records::*,
    spool::{Counter, Directory, Error as SpoolError, Full},
    work::Work,
};
use unityrs_export::ngui::{OutputKind, SkipOutput};
use unityrs_testkit::scratch::Scratch;

#[test]
fn candidate_members_and_terminal_figures_roundtrip_from_disk() {
    let scratch = Scratch::new("copy-out");
    let (budget, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 6000).unwrap();
    let mut directory = Directory::create(
        scratch.join("attempt"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut work = Work::new(1024 * 1024);
    let bytes = b"synthetic json";
    let hashes = copy_output(&directory, 19, bytes, &mut work).unwrap();
    assert_eq!(std::fs::read(directory.path().join("o19")).unwrap(), bytes);
    let mut writer = Writer::new(&directory, &meter).unwrap();
    assert!(budget.snapshot().input.used >= 65536);
    writer
        .append(
            Record::Candidate(Candidate {
                key: "result.json",
                unit: None,
                input: "input",
                serialized_file: Text::Raw(b"file"),
                kind: OutputKind::AtlasJson,
                path_id: i64::MIN,
                hashes,
                picture: None,
                size: bytes.len() as u64,
                plan_index: 19,
                cut: false,
            }),
            &mut work,
        )
        .unwrap();
    for path in ["a.json", "b.json"] {
        writer
            .member(
                "archive.zip",
                Member {
                    archive: 0,
                    path: path.into(),
                    key: "result.json",
                    input: "input",
                    md5: &hashes.md5,
                    reason: None,
                    cut: false,
                },
                &mut work,
            )
            .unwrap();
    }
    let end = writer.finish(summary(Status::Complete), &mut work).unwrap();
    assert_eq!(end.figures.unwrap().export_work, work.export_work());
    assert_eq!(end.spooled_part, work.spooled_part());
    assert_eq!(end.claimed_bytes, counter.used());
    let text = std::fs::read_to_string(directory.path().join("records.ndjson")).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|scratch| serde_json::from_str(scratch).unwrap())
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["type"], "candidate");
    assert_eq!(rows[0]["path_id"], i64::MIN);
    assert_eq!(rows[0]["file"], "o19");
    assert!(rows[0]["visible_hash"].is_null());
    assert!(rows[0]["mode"].is_null());
    assert!(rows[0]["etag"].is_null());
    assert_eq!(rows[1]["path"], "a.json");
    assert_eq!(rows[2]["path"], "b.json");
    assert_eq!(rows[3]["type"], "input_summary");
    assert_eq!(rows[3]["export_work"], work.export_work());
    assert_eq!(rows[3]["claimed_bytes"], counter.used());
    let phase = unityrs_batch::phase::Account::new(64 << 20);
    let mut typed = read::Reader::new(
        std::fs::File::open(directory.path().join("records.ndjson")).unwrap(),
        "input",
        false,
        1,
        phase.meter(),
    )
    .unwrap();
    assert!(
        matches!(typed.next(&|| false).unwrap(), Some(read::Row::Candidate(c)) if c.plan_index == 19 && c.path_id == i64::MIN && c.md5 == hashes.md5)
    );
    for expected in ["a.json", "b.json"] {
        assert!(
            matches!(typed.next(&|| false).unwrap(), Some(read::Row::Member(m)) if m.path == expected && m.md5 == hashes.md5)
        );
    }
    assert!(
        matches!(typed.next(&|| false).unwrap(), Some(read::Row::Summary(s)) if s.claimed_bytes == counter.used() && s.figures.unwrap().export_work == work.export_work())
    );
    assert!(typed.next(&|| false).unwrap().is_none());
    drop(typed);
    assert_eq!(phase.snapshot().input.used, 0);
    assert!(budget.snapshot().is_clear());
    directory.remove().unwrap();
    assert_eq!(counter.used(), 0);
}

#[test]
fn full_flushes_cross_lines_and_drop_discards_a_failed_partial_buffer() {
    let scratch = Scratch::new("flushes");
    let (budget, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(
        scratch.join("attempt"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut work = Work::new(1024 * 1024);
    let hashes = copy_output(&directory, 7, b"earlier output", &mut work).unwrap();
    let mut writer = Writer::new(&directory, &meter).unwrap();
    writer
        .append(
            Record::Candidate(Candidate {
                key: "earlier.json",
                unit: None,
                input: "input",
                serialized_file: "file".into(),
                kind: OutputKind::AtlasJson,
                path_id: 1,
                hashes,
                picture: None,
                size: 14,
                plan_index: 7,
                cut: false,
            }),
            &mut work,
        )
        .unwrap();
    let name = [0; 1024];
    for _ in 0..24 {
        writer
            .append(
                Record::Diagnostic(Diagnostic {
                    kind: DiagnosticKind::NestedContainer,
                    input: "input",
                    bundle: Some("input"),
                    name: Text::Raw(&name),
                    cut: false,
                }),
                &mut work,
            )
            .unwrap();
    }
    let len = std::fs::metadata(directory.path().join("records.ndjson"))
        .unwrap()
        .len();
    assert!(len >= 65536);
    assert_eq!(len % 65536, 0);
    assert_eq!(work.export_work(), len + 4096);
    // Input failure elsewhere: dropping the writer must not flush its tail.
    drop(writer);
    assert_eq!(
        std::fs::metadata(directory.path().join("records.ndjson"))
            .unwrap()
            .len(),
        len
    );
    assert!(budget.snapshot().is_clear());
    let fail = Directory::create(
        scratch.join("failure"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let before = work.export_work();
    let mut failed = summary(Status::Failed);
    failed.reason = Some(Reason::Text("container_error".into()));
    failed.figures.as_mut().unwrap().export_work = work.export_work();
    failed.spooled_part = work.spooled_part();
    let end = write_failure_record(&fail, &meter, failed).unwrap();
    assert_eq!(work.export_work(), before);
    assert_eq!(work.spooled_part(), len + 4096);
    assert_eq!(end.figures.unwrap().export_work, before);
    assert_eq!(end.spooled_part, len + 8192);
    let row: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fail.path().join("records.ndjson")).unwrap())
            .unwrap();
    assert_eq!(row["status"], "failed");
    assert_eq!(row["skips"], 0);
    assert_eq!(
        std::fs::read(directory.path().join("o7")).unwrap(),
        b"earlier output"
    );
    assert_eq!(std::fs::read_dir(fail.path()).unwrap().count(), 1);
}

#[test]
fn filesystem_errors_keep_their_detail_and_claims_until_deletion() {
    for op in 1..=4 {
        for errno in [libc::ENOSPC, libc::EDQUOT, libc::EIO] {
            let scratch = Scratch::new("io-fault");
            let (_, meter) = meter();
            let facts = Arc::new(Faults::default());
            let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
            let mut directory =
                Directory::create(scratch.join("attempt"), counter.clone(), facts.clone()).unwrap();
            let mut work = Work::new(1024 * 1024);
            let result = if op == 4 {
                copy_output(&directory, 0, b"content", &mut work).unwrap();
                facts.set(op, errno);
                directory.remove()
            } else {
                let mut writer = Writer::new(&directory, &meter).unwrap();
                writer
                    .append(
                        Record::Diagnostic(Diagnostic {
                            kind: DiagnosticKind::UndeclaredDependency,
                            input: "input",
                            bundle: None,
                            name: "name".into(),
                            cut: false,
                        }),
                        &mut work,
                    )
                    .unwrap();
                if op == 1 {
                    drop(writer);
                    facts.set(op, errno);
                    copy_output(&directory, 0, b"content", &mut work)
                        .map(|_| ())
                        .map_err(|e| match e {
                            Error::Spool(e) => e,
                            other => panic!("{other:?}"),
                        })
                } else {
                    facts.set(op, errno);
                    writer
                        .finish(summary(Status::Complete), &mut work)
                        .map(|_| ())
                        .map_err(|e| match e {
                            Error::Spool(e) => e,
                            other => panic!("{other:?}"),
                        })
                }
            };
            match (errno, result.unwrap_err()) {
                (libc::ENOSPC, SpoolError::Full(Full::Enospc))
                | (libc::EDQUOT, SpoolError::Full(Full::Edquot)) => (),
                (libc::EIO, SpoolError::HostIo(e)) => assert_eq!(e.raw_os_error(), Some(libc::EIO)),
                other => panic!("operation {op}: {other:?}"),
            }
            assert_eq!(counter.used(), if op == 1 { 0 } else { 4096 });
            facts.set(0, 0);
            directory.remove().unwrap();
            assert_eq!(counter.used(), 0);
        }
    }
}

#[test]
fn work_failure_poisoning_prevents_a_post_failure_flush() {
    let scratch = Scratch::new("poison");
    let (budget, meter) = meter();
    let facts = Arc::new(Faults::default());
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(scratch.join("attempt"), counter, facts.clone()).unwrap();
    let mut writer = Writer::new(&directory, &meter).unwrap();
    let mut work = Work::new(0);
    let member = Member {
        archive: 0,
        path: "p".into(),
        key: "k",
        input: "input",
        md5: &[0; 16],
        reason: None,
        cut: false,
    };
    assert!(matches!(
        writer.member("archive.zip", member, &mut work),
        Err(Error::Work(_))
    ));
    let mut fresh_work = Work::new(u64::MAX);
    assert!(
        writer
            .finish(summary(Status::Complete), &mut fresh_work)
            .is_err()
    );
    assert_eq!(facts.writes.load(Ordering::Relaxed), 0);
    assert_eq!(
        std::fs::metadata(directory.path().join("records.ndjson"))
            .unwrap()
            .len(),
        0
    );
    assert!(budget.snapshot().is_clear());
}

#[test]
fn maximal_control_fields_and_wrapped_reason_stay_bounded() {
    let scratch = Scratch::new("line-bound");
    let (_, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(
        scratch.join("attempt"),
        counter,
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut writer = Writer::new(&directory, &meter).unwrap();
    let mut work = Work::new(1024 * 1024);
    let bytes = [0; 1025];
    let text = Text::Raw(&bytes);
    let raw = String::from_utf8(vec![0; 1024]).unwrap();
    let hashes = copy_output(&directory, u64::MAX, b"bytes", &mut work).unwrap();
    writer
        .append(
            Record::Candidate(Candidate {
                key: &raw,
                unit: Some(&raw),
                input: &raw,
                serialized_file: text,
                kind: OutputKind::AtlasJson,
                path_id: i64::MIN,
                hashes,
                picture: None,
                size: u64::MAX,
                plan_index: u64::MAX,
                cut: false,
            }),
            &mut work,
        )
        .unwrap();
    writer
        .member(
            &raw,
            Member {
                archive: u64::MAX,
                path: text,
                key: &raw,
                input: &raw,
                md5: &hashes.md5,
                reason: Some(unityrs_batch::reason::LeftOut::UnwritablePath(
                    unityrs_batch::reason::KeyRule::ControlChar,
                )),
                cut: false,
            },
            &mut work,
        )
        .unwrap();
    writer
        .append(
            Record::Diagnostic(Diagnostic {
                kind: DiagnosticKind::SameNamedCab,
                input: &raw,
                bundle: Some(&raw),
                name: text,
                cut: false,
            }),
            &mut work,
        )
        .unwrap();
    writer
        .append(
            Record::Skip(Skip {
                input: &raw,
                serialized_file: text,
                path_id: i64::MIN,
                class: "Texture2D",
                output: SkipOutput::Texture,
                sprite: Some(u64::MAX),
                reason: Reason::Argument {
                    code: "outside_view",
                    value: text,
                },
                detail: Some(text),
                cut: false,
            }),
            &mut work,
        )
        .unwrap();
    let mut end = summary(Status::CompleteWithSkips);
    end.detail = Some(text);
    writer.finish(end, &mut work).unwrap();
    let bytes = std::fs::read(directory.path().join("records.ndjson")).unwrap();
    for line in bytes.split_inclusive(|budget| *budget == b'\n') {
        assert!(line.len() <= 32768);
        let row: serde_json::Value = serde_json::from_slice(line).unwrap();
        assert_eq!(row["cut"], true);
        if row["type"] == "skip" {
            let reason = row["reason"].as_str().unwrap();
            assert!(reason.starts_with("outside_view("));
            assert!(reason.ends_with(')'));
        }
    }
}

#[test]
fn member_work_uses_largest_manifest_row_and_the_final_archive_key() {
    let scratch = Scratch::new("member-work");
    let (_, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(
        scratch.join("attempt"),
        counter,
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut writer = Writer::new(&directory, &meter).unwrap();
    let mut work = Work::new(1024 * 1024);
    writer
        .member(
            "archive.zip",
            Member {
                archive: 0,
                path: "p".into(),
                key: "k",
                input: "input",
                md5: &[0; 16],
                reason: None,
                cut: false,
            },
            &mut work,
        )
        .unwrap();
    // Hand-checked final row with reason unwritable_path(archive_conflict),
    // included=false and cut=false: 177 bytes plus its newline.
    assert_eq!(work.export_work(), 178);
    assert_eq!(work.spooled_part(), 178);
    writer.finish(summary(Status::Complete), &mut work).unwrap();
    assert_eq!(work.export_work(), 4274);
}

#[test]
fn buffer_refusal_allocates_no_file_and_returns_all_charges() {
    use unityrs_batch::budget::SealedBudget;
    use unityrs_core::{budget::Reservation, limits::Meter};
    let scratch = Scratch::new("buffer-refusal");
    let budget = Arc::new(SealedBudget::new(65535, 0));
    let meter = Meter::new(1024 * 1024, budget.clone(), Reservation::Input);
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(
        scratch.join("attempt"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    assert!(matches!(
        Writer::new(&directory, &meter),
        Err(Error::Charge(_))
    ));
    assert!(budget.snapshot().is_clear());
    assert_eq!(budget.snapshot().refusals(), 1);
    assert!(!directory.path().join("records.ndjson").exists());
    assert_eq!(counter.used(), 0);
}

#[test]
fn success_after_cross_buffer_lines_has_one_complete_terminal_summary() {
    let scratch = Scratch::new("cross-buffer");
    let (_, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 6000).unwrap();
    let directory = Directory::create(
        scratch.join("attempt"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut writer = Writer::new(&directory, &meter).unwrap();
    let mut work = Work::new(1024 * 1024);
    for _ in 0..24 {
        writer
            .append(
                Record::Diagnostic(Diagnostic {
                    kind: DiagnosticKind::NestedContainer,
                    input: "input",
                    bundle: Some("input"),
                    name: Text::Raw(&[0; 1024]),
                    cut: false,
                }),
                &mut work,
            )
            .unwrap();
    }
    let end = writer.finish(summary(Status::Complete), &mut work).unwrap();
    let bytes = std::fs::read(directory.path().join("records.ndjson")).unwrap();
    let rows: Vec<serde_json::Value> = bytes
        .split_inclusive(|budget| *budget == b'\n')
        .map(|line| {
            assert!(line.len() <= 32768);
            serde_json::from_slice(line).unwrap()
        })
        .collect();
    assert_eq!(rows.len(), 25);
    assert!(rows.iter().take(24).all(|r| r["type"] == "diagnostic"));
    assert_eq!(rows[24]["type"], "input_summary");
    assert_eq!(rows[24]["id"], "input");
    assert_eq!(
        work.export_work(),
        unityrs_batch::work::rounded(bytes.len() as u64, 4096).unwrap()
    );
    assert_eq!(end.claimed_bytes, counter.used());
    assert_eq!(
        counter.used(),
        unityrs_batch::work::rounded(bytes.len() as u64, 6000).unwrap()
    );
}

#[test]
fn failure_before_dispatch_has_null_figures_and_only_failure_spool_work() {
    let scratch = Scratch::new("before-dispatch");
    let (_, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(
        scratch.join("failure"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut failed = summary(Status::Failed);
    failed.figures = None;
    failed.reason = Some(Reason::Dependency {
        id: Text::Raw(&[0xff; 1025]),
        reason: "not_a_bundle".into(),
    });
    failed.detail = Some(Text::Joined(&[b"read", b" ", &[b'x'; 1024]]));
    let end = write_failure_record(&directory, &meter, failed).unwrap();
    let row: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("records.ndjson")).unwrap())
            .unwrap();
    assert!(row["input_peak"].is_null());
    assert!(row["export_work"].is_null());
    assert_eq!(row["cut"], true);
    assert!(end.cut);
    assert!(end.figures.is_none());
    // The repaired dependency reason and detail make this one flush exceed 4 KiB.
    assert_eq!(row["spooled_part"], 8192);
    assert_eq!(end.spooled_part, 8192);
    let reason = row["reason"].as_str().unwrap();
    assert_eq!(
        reason,
        format!("dependency_failed({}, not_a_bundle)", "�".repeat(1024))
    );
    assert_eq!(row["detail"].as_str().unwrap().len(), 1024);
    assert_eq!(row["claimed_bytes"], counter.used());
}

#[test]
fn failed_summary_normalizes_counters_before_serialization_and_return() {
    let scratch = Scratch::new("failed-counters");
    let (_, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(
        scratch.join("failure"),
        counter,
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut failed = summary(Status::Failed);
    failed.figures = Some(Figures {
        skips: 3,
        unresolved_scripts: 2,
        ..Figures::default()
    });
    let end = write_failure_record(&directory, &meter, failed).unwrap();
    let row: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("records.ndjson")).unwrap())
            .unwrap();
    let figures = end.figures.unwrap();
    assert_eq!((figures.skips, figures.unresolved_scripts), (0, 0));
    assert_eq!(row["skips"], figures.skips);
    assert_eq!(row["unresolved_scripts"], figures.unresolved_scripts);
}

#[test]
fn failure_record_preserves_end_message_figures_and_adds_only_its_flush() {
    let scratch = Scratch::new("failed-figures");
    let (_, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 6000).unwrap();
    let directory = Directory::create(
        scratch.join("failure"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut failed = summary(Status::Failed);
    {
        let mut attempt = Work::new(4195);
        attempt.pixels(100).unwrap();
        assert!(attempt.file(1).is_err()); // The refused increment is measured too.
        failed.figures = Some(Figures {
            input_peak: 1000,
            dependency_peak: 2000,
            evictions: 3,
            export_work: attempt.export_work(),
            decode_work_peak: 4000,
            ms: 5,
            ..Figures::default()
        });
        failed.spooled_part = attempt.spooled_part();
    } // Only the end-message figures reach the controller.
    let end = write_failure_record(&directory, &meter, failed).unwrap();
    let row: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("records.ndjson")).unwrap())
            .unwrap();
    assert_eq!(end.figures, failed.figures);
    for (field, expected) in [
        ("input_peak", 1000),
        ("dependency_peak", 2000),
        ("evictions", 3),
        ("export_work", 4196),
        ("decode_work_peak", 4000),
        ("ms", 5),
        ("spooled_part", 8192),
        ("claimed_bytes", 6000),
    ] {
        assert_eq!(row[field], expected, "{field}");
    }
    assert_eq!(end.spooled_part, 8192);
    assert_eq!(end.claimed_bytes, counter.used());
}

#[test]
fn terminal_summary_accounts_for_its_own_page_and_buffer_boundaries() {
    for (base, padding, expected) in [
        (3, 154, 4095),
        (3, 155, 4096),
        (3, 156, 4097),
        (57, 628, 65535),
        (57, 629, 65536),
        (57, 630, 65537),
    ] {
        let scratch = Scratch::new("summary-boundary");
        let (_, meter) = meter();
        let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
        let directory = Directory::create(
            scratch.join("attempt"),
            counter.clone(),
            Arc::new(Faults::default()),
        )
        .unwrap();
        let mut writer = Writer::new(&directory, &meter).unwrap();
        let mut work = Work::new(1024 * 1024);
        for n in (0..base).map(|_| 1024).chain([padding]) {
            let name = vec![b'x'; n];
            writer
                .append(
                    Record::Diagnostic(Diagnostic {
                        kind: DiagnosticKind::UndeclaredDependency,
                        input: "input",
                        bundle: None,
                        name: Text::Raw(&name),
                        cut: false,
                    }),
                    &mut work,
                )
                .unwrap();
        }
        let end = writer.finish(summary(Status::Complete), &mut work).unwrap();
        let bytes = std::fs::read(directory.path().join("records.ndjson")).unwrap();
        // These lengths are hand-counted from the documented artifact
        // fields, the summary's fixed-width integer slots included; they
        // straddle both rounding and actual flush boundaries.
        assert_eq!(bytes.len(), expected);
        let expected_work = unityrs_batch::work::rounded(expected as u64, 4096).unwrap();
        assert_eq!(work.export_work(), expected_work);
        assert_eq!(end.figures.unwrap().export_work, expected_work);
        assert_eq!(end.spooled_part, expected_work);
        assert_eq!(end.claimed_bytes, counter.used());
        assert_eq!(counter.used(), expected_work);
        let last = bytes.split(|budget| *budget == b'\n').rev().nth(1).unwrap();
        let row: serde_json::Value = serde_json::from_slice(last).unwrap();
        assert_eq!(row["export_work"], expected_work);
    }
}

#[test]
fn pictures_keep_visible_hash_and_mode_and_oversized_prepared_lines_fail_before_writing() {
    use unityrs_export::ngui::Picture;
    let scratch = Scratch::new("pictures");
    let (budget, meter) = meter();
    let counter = Counter::new(1024 * 1024, 0, 4096).unwrap();
    let directory = Directory::create(
        scratch.join("attempt"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let mut work = Work::new(1024 * 1024);
    let mut writer = Writer::new(&directory, &meter).unwrap();
    for (n, mode) in ["L", "LA", "RGB", "RGBA"].into_iter().enumerate() {
        let hashes = copy_output(&directory, n as u64, b"synthetic picture", &mut work).unwrap();
        let picture = Picture {
            mode,
            width: 1,
            height: 1,
            visible_hash: [0xab; 32],
        };
        writer
            .append(
                Record::Candidate(Candidate {
                    key: "picture.png",
                    unit: Some("unit"),
                    input: "input",
                    serialized_file: "file".into(),
                    kind: OutputKind::Texture,
                    path_id: -1,
                    hashes,
                    picture: Some(&picture),
                    size: 17,
                    plan_index: n as u64,
                    cut: false,
                }),
                &mut work,
            )
            .unwrap();
    }
    writer.finish(summary(Status::Complete), &mut work).unwrap();
    let text = std::fs::read_to_string(directory.path().join("records.ndjson")).unwrap();
    for (line, mode) in text.lines().zip(["L", "LA", "RGB", "RGBA"]) {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(row["visible_hash"], "ab".repeat(32));
        assert_eq!(row["mode"], mode);
        assert_eq!(row["unit"], "unit");
    }
    let other =
        Directory::create(scratch.join("other"), counter, Arc::new(Faults::default())).unwrap();
    let mut writer = Writer::new(&other, &meter).unwrap();
    let oversized = "x".repeat(32768);
    assert!(matches!(
        writer.append(
            Record::Diagnostic(Diagnostic {
                kind: DiagnosticKind::NestedContainer,
                input: "input",
                bundle: None,
                name: Text::Prepared(&oversized, false),
                cut: false
            }),
            &mut work
        ),
        Err(Error::Internal)
    ));
    assert!(writer.finish(summary(Status::Complete), &mut work).is_err());
    assert_eq!(
        std::fs::metadata(other.path().join("records.ndjson"))
            .unwrap()
            .len(),
        0
    );
    assert!(budget.snapshot().is_clear());
}
