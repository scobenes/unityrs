//! Typed handoff readers reject corrupt rows, frames and stream order.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
#[path = "winners_support/member_scene.rs"]
mod member_scene;
use std::{io::Read, sync::Arc};
use unityrs_batch::{
    phase::{Account, Error},
    sort::{NoHooks, Options},
    spool::{
        Counter, Host,
        workspace::{Sealed, Workspace},
    },
    winners::{self, Context, Prepared, rowread},
};
use unityrs_testkit::scratch::Scratch;
fn bytes(workspace: &Workspace, file: &Sealed) -> Vec<u8> {
    let mut b = Vec::new();
    workspace
        .open(file, &|| false)
        .unwrap()
        .read_to_end(&mut b)
        .unwrap();
    b
}
fn seal(workspace: &mut Workspace, b: &[u8]) -> Sealed {
    let mut w = workspace.create_file(&|| false).unwrap();
    w.append(b, &|| false).unwrap();
    w.seal(&|| false).unwrap()
}
fn check_rows(
    workspace: &Workspace,
    file: &Sealed,
    kind: rowread::File,
    account: &Account,
) -> Result<(), Error> {
    let mut reader =
        rowread::Reader::open(workspace, Some(file), kind, account.meter(), &|| false)?;
    while reader.next(&|| false)?.is_some() {}
    Ok(())
}
fn member_row(path: &str, reason: Option<&str>) -> Vec<u8> {
    let mut row = serde_json::to_vec(&serde_json::json!({
        "archive": "archive", "path": path, "key": "key", "input": "input",
        "md5": "00000000000000000000000000000000",
        "included": reason.is_none(), "reason": reason, "cut": false,
    }))
    .unwrap();
    row.push(b'\n');
    row
}

// Independent encoding: the fixed fields occupy 46 bytes, all zero for
// included, uncut members of archive/source zero with empty size and hashes.
fn list_frame(path: &str, key: &str) -> Vec<u8> {
    let mut payload = vec![0; 46];
    for text in ["archive", path, key, "input"] {
        payload.extend(u16::try_from(text.len()).unwrap().to_le_bytes());
        payload.extend(text.as_bytes());
    }
    let mut frame = u32::try_from(payload.len() + 4)
        .unwrap()
        .to_le_bytes()
        .to_vec();
    frame.extend(payload);
    frame
}

#[test]
fn included_lists_reject_duplicate_and_ancestor_paths() {
    let scratch = Scratch::new("member-list-conflicts");
    let account = Account::new(64 << 20);
    let mut workspace = Workspace::create(
        scratch.join("w"),
        Counter::new(u64::MAX, 0, 4096).unwrap(),
        Arc::new(Host),
    )
    .unwrap();
    for (paths, valid) in [
        (vec!["p", "p"], false),
        (vec!["p", "p/child"], false),
        (vec!["p/child", "p/child/grandchild"], false),
        (vec!["p/child", "p!", "pa"], true),
        (vec!["p", "pa/child"], true),
    ] {
        let mut data = b"URSSORT\0".to_vec();
        data.extend(1_u32.to_le_bytes());
        data.extend(114_u32.to_le_bytes());
        for (i, path) in paths.iter().enumerate() {
            data.extend(list_frame(path, &format!("key{i}")));
        }
        let file = seal(&mut workspace, &data);
        let mut reader =
            winners::member::Reader::open(&workspace, &file, account.meter(), &|| false).unwrap();
        assert_eq!(reader.next(&|| false).unwrap().unwrap().path, paths[0]);
        if valid {
            for path in &paths[1..] {
                assert_eq!(reader.next(&|| false).unwrap().unwrap().path, *path);
            }
            assert!(reader.next(&|| false).unwrap().is_none());
        } else {
            assert_eq!(
                reader.next(&|| false).unwrap_err(),
                Error::internal(),
                "paths={paths:?}"
            );
            assert_eq!(reader.next(&|| false).unwrap_err(), Error::internal());
        }
    }
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn public_member_paths_obey_every_member_rule_for_each_row_kind() {
    let scratch = Scratch::new("member-path-rules");
    let account = Account::new(64 << 20);
    let mut workspace = Workspace::create(
        scratch.join("w"),
        Counter::new(u64::MAX, 0, 4096).unwrap(),
        Arc::new(Host),
    )
    .unwrap();
    let mut cases = vec![
        ("empty", String::new()),
        ("control_char", "a\0b".into()),
        ("control_char", "a\u{1f}b".into()),
        ("control_char", "a\u{7f}b".into()),
        ("leading_slash", "/a".into()),
        ("trailing_slash", "a/".into()),
        ("empty_segment", "a//b".into()),
        ("dot_segment", "../escape".into()),
        ("dot_segment", "a/./b".into()),
        ("segment_too_long", "a".repeat(256)),
        (
            "too_long",
            format!("{}/a", vec!["a".repeat(255); 4].join("/")),
        ),
    ];
    // Invalid UTF-8 has a replacement character in its public display path.
    cases.push(("not_utf8", "\u{fffd}".into()));
    for (rule, path) in &cases {
        for reason in [None, Some("duplicate_member"), Some("prefix_conflict")] {
            let file = seal(&mut workspace, &member_row(path, reason));
            let expected = if *rule == "not_utf8" {
                Ok(())
            } else {
                Err(Error::internal())
            };
            assert_eq!(
                check_rows(&workspace, &file, rowread::File::Members, &account),
                expected,
                "rule={rule} reason={reason:?}",
            );
        }
        let reason = format!("unwritable_path({rule})");
        let file = seal(&mut workspace, &member_row(path, Some(&reason)));
        let mut reader = rowread::Reader::open(
            &workspace,
            Some(&file),
            rowread::File::Members,
            account.meter(),
            &|| false,
        )
        .unwrap();
        let rowread::Row::Member(row) = reader.next(&|| false).unwrap().unwrap() else {
            panic!("expected member row");
        };
        assert_eq!(row.path, path);
        assert_eq!(row.reason, Some(reason.as_str()));
        assert!(reader.next(&|| false).unwrap().is_none());
    }
    // Raw non-UTF-8 is invalid JSON for every row kind, including unwritable.
    for reason in [
        None,
        Some("duplicate_member"),
        Some("prefix_conflict"),
        Some("unwritable_path(not_utf8)"),
    ] {
        let mut row = member_row("p", reason);
        let at = row
            .windows(10)
            .position(|w| w == b"\"path\":\"p\"")
            .unwrap();
        row[at + 8] = 0xff;
        let file = seal(&mut workspace, &row);
        assert_eq!(
            check_rows(&workspace, &file, rowread::File::Members, &account),
            Err(Error::internal())
        );
    }
    // Member paths allow names reserved only for loose keys, and non-ASCII.
    for path in ["_unityrs/a", "a/.unityrs-tmp-b", "é/leaf", "a\\b"] {
        for reason in [None, Some("duplicate_member"), Some("prefix_conflict")] {
            let file = seal(&mut workspace, &member_row(path, reason));
            assert_eq!(
                check_rows(&workspace, &file, rowread::File::Members, &account),
                Ok(())
            );
        }
    }
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn public_readers_reject_schema_order_enums_and_truncation() {
    let scratch = Scratch::new("row-reader");
    let case = member_scene::scene(false);
    let spec = case.spec().spec;
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let account = Account::new(64 << 20);
    let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
    let prepared = Prepared::from_spec(&spec).unwrap();
    let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
    let handoff = winners::complete(&mut Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &commits,
        workspace: &mut workspace,
        account: &account,
        options: Options::default(),
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &winners::NoObserver,
    })
    .unwrap();
    for (file, kind, enum_field) in [
        (handoff.outputs.as_ref(), rowread::File::Outputs, "kind"),
        (
            handoff.collisions.as_ref(),
            rowread::File::Collisions,
            "rule",
        ),
        (handoff.archives.as_ref(), rowread::File::Archives, "status"),
        (
            handoff.archive_members.as_ref(),
            rowread::File::Members,
            "reason",
        ),
    ] {
        let original = bytes(&workspace, file.unwrap());
        let lines: Vec<_> = original.split_inclusive(|&b| b == b'\n').collect();
        assert!(lines.len() > 1);
        let mut unknown: serde_json::Value = serde_json::from_slice(lines[0]).unwrap();
        unknown["unknown"] = true.into();
        let mut bad_enum: serde_json::Value = serde_json::from_slice(lines[0]).unwrap();
        bad_enum[enum_field] = "unknown".into();
        let with_newline = |v: &serde_json::Value| {
            let mut b = serde_json::to_vec(v).unwrap();
            b.push(b'\n');
            b
        };
        let duplicate = [lines[0], lines[0]].concat();
        let reverse = [lines[1], lines[0]].concat();
        let mut bad_key: serde_json::Value = serde_json::from_slice(lines[0]).unwrap();
        bad_key["key"] = serde_json::Value::Null;
        for b in [
            with_newline(&unknown),
            with_newline(&bad_enum),
            with_newline(&bad_key),
            reverse,
            original[..original.len() - 1].to_vec(),
        ] {
            let file = seal(&mut workspace, &b);
            assert_eq!(
                check_rows(&workspace, &file, kind, &account),
                Err(Error::internal())
            );
        }
        if !matches!(kind, rowread::File::Members) {
            let file = seal(&mut workspace, &duplicate);
            assert_eq!(
                check_rows(&workspace, &file, kind, &account),
                Err(Error::internal())
            );
        }
    }
    assert_eq!(account.snapshot().input.used, 0);
}
#[test]
fn metadata_and_list_readers_reject_bad_tags_and_truncated_frames() {
    let scratch = Scratch::new("binary-reader");
    let case = member_scene::scene(false);
    let spec = case.spec().spec;
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let account = Account::new(64 << 20);
    let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
    let prepared = Prepared::from_spec(&spec).unwrap();
    let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
    let handoff = winners::complete(&mut Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &commits,
        workspace: &mut workspace,
        account: &account,
        options: Options::default(),
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &winners::NoObserver,
    })
    .unwrap();
    let mut metadata = winners::archives::Reader::open(
        &workspace,
        handoff.metadata.as_ref().unwrap(),
        account.meter(),
        &|| false,
    )
    .unwrap();
    let list = loop {
        let a = metadata.next(&|| false).unwrap().unwrap();
        if let Some(list) = a.list {
            break list;
        }
    };
    drop(metadata);
    for (file, member) in [(handoff.metadata.as_ref().unwrap(), false), (&list, true)] {
        let original = bytes(&workspace, file);
        let mut invalid = original.clone();
        invalid[if member { 20 } else { 48 }] = 255;
        let mut version = original.clone();
        version[8] = 255;
        for b in [invalid, version, original[..original.len() - 1].to_vec()] {
            let file = seal(&mut workspace, &b);
            let result = account.execute(|| {
                if member {
                    let mut r =
                        winners::member::Reader::open(&workspace, &file, account.meter(), &|| {
                            false
                        })?;
                    while r.next(&|| false)?.is_some() {}
                } else {
                    let mut r = winners::archives::Reader::open(
                        &workspace,
                        &file,
                        account.meter(),
                        &|| false,
                    )?;
                    while r.next(&|| false)?.is_some() {}
                }
                Ok(())
            });
            assert_eq!(result, Err(Error::internal()));
        }
    }
    assert_eq!(account.snapshot().input.used, 0);
}
