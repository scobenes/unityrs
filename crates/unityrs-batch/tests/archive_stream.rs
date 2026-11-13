mod archive_support;
use archive_support::{Scene, seal};
use unityrs_batch::{
    archive::{self, Eligibility, Input},
    digest::Hashes,
    phase::{Account, Error},
    winners::layout::LIMIT,
};

#[test]
fn precedence_is_empty_then_withheld_then_limit_inclusive() {
    for size in [0, LIMIT - 1, LIMIT, LIMIT + 1, u64::MAX] {
        assert_eq!(archive::eligibility(0, true, size), Eligibility::Empty);
        assert_eq!(archive::eligibility(0, false, size), Eligibility::Empty);
        assert_eq!(archive::eligibility(1, true, size), Eligibility::Withheld);
        assert_eq!(
            archive::eligibility(1, false, size),
            if size > LIMIT {
                Eligibility::Oversized
            } else {
                Eligibility::Eligible
            }
        );
    }
}
#[test]
fn ordinary_members_are_chunk_independent_and_retire_before_summary() {
    let scene = Scene::new(&[("a/leaf", b"hello"), ("a!", b""), ("é\\\"", b"world")]);
    let baseline = scene.counter.used();
    let mut expected = None;
    for chunk in [1, 7, 65_536, 1 << 20] {
        let (bytes, summary) = scene.generate(chunk).unwrap();
        assert_eq!(summary.size, scene.size);
        assert_eq!(summary.emitted_size, scene.size);
        assert_eq!(summary.md5, Hashes::of(&bytes).md5);
        assert_eq!(summary.etag, Hashes::of(&bytes).etag);
        assert_eq!(scene.counter.used(), baseline);
        assert_eq!(scene.workspace.claimed(), baseline);
        if let Some(expected) = &expected {
            assert_eq!(&bytes, expected);
        } else {
            expected = Some(bytes);
        }
    }
}
#[test]
fn empty_output_does_not_advance_and_early_finish_refuses() {
    let scene = Scene::new(&[("a", b"x")]);
    let account = Account::new(1 << 20);
    let mut stream = archive::open(scene.context(&account), scene.input(), &|| false).unwrap();
    assert_eq!(stream.read(&mut [], &|| false).unwrap(), 0);
    assert_eq!(stream.finish(&|| false), Err(Error::internal()));
    assert_eq!(account.snapshot().input.used, 0);
}
#[test]
fn source_extent_and_same_length_corruption_are_internal() {
    for bytes in [b"".as_slice(), b"xy", b"y"] {
        let scene = Scene::new(&[("a", b"x")]);
        std::fs::write(&scene.files.paths[0], bytes).unwrap();
        assert_eq!(scene.generate(1).unwrap_err(), Error::internal());
    }
}
#[test]
fn key_index_count_size_crc_and_md5_are_checked_independently() {
    for field in ["key", "index", "count", "size", "crc", "md5"] {
        let mut scene = Scene::new(&[("a", b"x")]);
        let mut data = archive_support::list_header();
        let mut frame = archive_support::frame("a", 0, b"x");
        if field == "crc" {
            frame[30] ^= 1;
        }
        if field == "md5" {
            frame[34] ^= 1;
        }
        data.extend(frame);
        scene.list = seal(&mut scene.workspace, &data);
        let account = Account::new(1 << 20);
        let input = Input {
            key: if field == "key" { "other" } else { "archive" },
            index: u32::from(field == "index"),
            members: if field == "count" { 2 } else { 1 },
            size: scene.size + u64::from(field == "size"),
            list: &scene.list,
        };
        let mut stream = archive::open(scene.context(&account), input, &|| false).unwrap();
        let mut buffer = vec![0; 4096];
        let error = loop {
            match stream.read(&mut buffer, &|| false) {
                Ok(0) => panic!("unexpected stream completion for {field}"),
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert_eq!(error, Error::internal(), "{field}");
        assert_eq!(stream.read(&mut buffer, &|| false), Err(Error::internal()));
        assert_eq!(stream.finish(&|| false), Err(Error::internal()));
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
fn ineligible_archives_refuse_before_opening_or_creating_scratch() {
    let scene = Scene::new(&[("a", b"x")]);
    let account = Account::new(1 << 20);
    let baseline = std::fs::read_dir(scene.scratch.join("w")).unwrap().count();
    for (members, size) in [(0, 22), (1, LIMIT + 1)] {
        let input = Input {
            key: "archive",
            index: 0,
            members,
            size,
            list: &scene.list,
        };
        assert!(matches!(
            archive::open(scene.context(&account), input, &|| false),
            Err(Error::Run(_))
        ));
        assert_eq!(
            std::fs::read_dir(scene.scratch.join("w")).unwrap().count(),
            baseline
        );
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
fn insufficient_phase_room_is_internal_and_returns_all_memory_charges() {
    let scene = Scene::new(&[("a", b"x")]);
    let account = Account::new(100);
    assert!(
        matches!(archive::open(scene.context(&account),scene.input(),&||false),Err(e) if e==Error::internal())
    );
    assert_eq!(account.snapshot().input.used, 0);
}
