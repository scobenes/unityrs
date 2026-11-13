//! Revalidation and first-head durability at the filesystem boundary.
mod directory_support;
use directory_support::{fs, head, preflight, resolvers};
use std::{fs as host, os::unix::fs::MetadataExt};
use unityrs_batch::sink::{Class, Context, path::Root};
use unityrs_testkit::scratch::Scratch;

#[test]
fn parent_moved_outside_root_refuses_create_without_touching_outside() {
    for fallback in resolvers() {
        let scratch = Scratch::new("directory-create-revalidate");
        let outside = Scratch::new("directory-create-outside");
        host::create_dir(scratch.join("parent")).unwrap();
        let root = Root::open(scratch.path(), fs(fallback)).unwrap();
        let parent = root.parent("parent/file", false, Context::Output).unwrap();
        host::rename(scratch.join("parent"), outside.join("moved")).unwrap();
        // A new, real directory at the old name must fail the inode comparison.
        host::create_dir(scratch.join("parent")).unwrap();
        let error = parent.temporary("r", 0, Context::Output).err().unwrap();
        assert_eq!(error.class, Class::PublishFailed);
        assert_eq!(host::read_dir(outside.join("moved")).unwrap().count(), 0);
        assert_eq!(host::read_dir(scratch.join("parent")).unwrap().count(), 0);
    }
}

#[test]
fn parent_moved_outside_root_refuses_rename_and_preserves_destination() {
    for fallback in resolvers() {
        for replace in [false, true] {
            let scratch = Scratch::new("directory-commit-revalidate");
            let outside = Scratch::new("directory-commit-outside");
            host::create_dir(scratch.join("parent")).unwrap();
            host::write(scratch.join("parent/file"), b"old").unwrap();
            let root = Root::open(scratch.path(), fs(fallback)).unwrap();
            let parent = root.parent("parent/file", false, Context::Output).unwrap();
            let temp = parent.temporary("r", 0, Context::Output).unwrap();
            temp.write_all(b"new").unwrap();
            host::rename(scratch.join("parent"), outside.join("moved")).unwrap();
            if replace {
                host::create_dir(scratch.join("parent")).unwrap();
            }
            assert_eq!(temp.rename("file").unwrap_err().class, Class::PublishFailed);
            assert_eq!(host::read(outside.join("moved/file")).unwrap(), b"old");
            assert_eq!(host::read_dir(outside.join("moved")).unwrap().count(), 2);
            assert!(!scratch.join("parent/file").exists());
        }
    }
}

#[test]
fn moved_head_parent_refuses_create_and_preserves_fencing_history() {
    let scratch = Scratch::new("directory-head-revalidate");
    let outside = Scratch::new("directory-head-outside");
    let directory = preflight(scratch.path(), 1, fs(true)).unwrap();
    directory.write_head(&head(1), true).unwrap();
    host::rename(scratch.join("_unityrs/r"), outside.join("moved")).unwrap();
    host::create_dir(scratch.join("_unityrs/r")).unwrap();
    assert_eq!(
        directory.write_head(&head(2), false).unwrap_err().class,
        Class::PublishFailed
    );
    assert_eq!(
        host::read(outside.join("moved/manifest.json")).unwrap(),
        head(1)
    );
    assert_eq!(host::read_dir(outside.join("moved")).unwrap().count(), 1);
}

#[test]
// A found ancestor is synced too: an earlier preflight may have made it
// and stopped before its sync succeeded (the retry case).
fn first_head_syncs_each_ancestor_found_or_made_before_acknowledgment() {
    for existing_meta in [false, true] {
        let scratch = Scratch::new("directory-first-head-durability");
        if existing_meta {
            host::create_dir(scratch.join("_unityrs")).unwrap();
        }
        let adapter = fs(true);
        let directory = preflight(scratch.path(), 1, adapter.clone()).unwrap();
        directory.write_head(&head(1), true).unwrap();
        let expected = vec![
            host::metadata(scratch.path()).unwrap().ino(),
            host::metadata(scratch.join("_unityrs")).unwrap().ino(),
            host::metadata(scratch.join("_unityrs/r")).unwrap().ino(),
        ];
        assert_eq!(*adapter.synced.lock().unwrap(), expected);
        let calls: Vec<_> = adapter
            .calls
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|p| ["mkdir", "fsync_dir", "fsync", "rename"].contains(p))
            .collect();
        let expected_calls = if existing_meta {
            vec![
                "fsync_dir",
                "mkdir",
                "fsync_dir",
                "fsync",
                "rename",
                "fsync_dir",
            ]
        } else {
            vec![
                "mkdir",
                "fsync_dir",
                "mkdir",
                "fsync_dir",
                "fsync",
                "rename",
                "fsync_dir",
            ]
        };
        assert_eq!(calls, expected_calls);
    }
}

#[test]
fn ancestor_sync_failure_refuses_preflight_before_a_head_can_be_written() {
    let scratch = Scratch::new("directory-ancestor-sync-failure");
    let adapter = std::sync::Arc::new(directory_support::Faults {
        fallback: Some(libc::ENOSYS),
        fail: Some(("fsync_dir", libc::EIO)),
        crossing: std::sync::atomic::AtomicUsize::new(usize::MAX),
        ..directory_support::Faults::default()
    });
    assert_eq!(
        preflight(scratch.path(), 1, adapter).err().unwrap().class,
        Class::SinkRootUnusable
    );
    assert!(!scratch.join("_unityrs/r/manifest.json").exists());
}

#[test]
fn default_adapter_probe_reports_real_syscall_availability() {
    use unityrs_batch::{sink::fs::FileSystem, sys};
    let scratch = Scratch::new("directory-probe-availability");
    let root = sys::open_directory(scratch.path()).unwrap();
    let real = sys::beneath_probe(&root);
    let adapter = directory_support::Faults::default().probe(&root);
    assert_eq!(
        adapter
            .as_ref()
            .err()
            .and_then(std::io::Error::raw_os_error),
        real.as_ref().err().and_then(std::io::Error::raw_os_error)
    );
    assert_eq!(adapter.is_ok(), real.is_ok());
}

#[test]
fn parent_moved_during_stale_temp_unlink_is_rechecked_before_create() {
    for fallback in resolvers() {
        let scratch = Scratch::new("directory-unlink-revalidate");
        let outside = Scratch::new("directory-unlink-outside");
        host::create_dir(scratch.join("parent")).unwrap();
        let adapter = fs(fallback);
        let root = Root::open(scratch.path(), adapter.clone()).unwrap();
        let parent = root.parent("parent/file", false, Context::Output).unwrap();
        *adapter.race.lock().unwrap() =
            Some((scratch.join("parent"), outside.join("moved"), "unlink_move"));
        assert_eq!(
            parent
                .temporary("r", 0, Context::Output)
                .err()
                .unwrap()
                .class,
            Class::PublishFailed
        );
        assert_eq!(host::read_dir(outside.join("moved")).unwrap().count(), 0);
    }
}

#[test]
fn three_field_head_is_not_a_manifest_and_cannot_authorize_overwrite() {
    let scratch = Scratch::new("directory-seven-field-envelope");
    host::create_dir_all(scratch.join("_unityrs/r")).unwrap();
    let bytes = br#"{"run_id":"r","attempt":1,"write_seq":0}"#;
    let path = scratch.join("_unityrs/r/manifest.json");
    host::write(&path, bytes).unwrap();
    let error = preflight(scratch.path(), 2, fs(true)).err().unwrap();
    assert_eq!(error.class, Class::ManifestUnreadable);
    assert_eq!(error.call, "not_a_manifest");
    assert_eq!(host::read(&path).unwrap(), bytes);
}

#[test]
fn moved_intermediate_parent_refuses_mkdir_before_creating_anything_outside() {
    for fallback in resolvers() {
        let scratch = Scratch::new("directory-mkdir-revalidate");
        let outside = Scratch::new("directory-mkdir-outside");
        host::create_dir(scratch.join("parent")).unwrap();
        let adapter = fs(fallback);
        let root = Root::open(scratch.path(), adapter.clone()).unwrap();
        *adapter.race.lock().unwrap() =
            Some((scratch.join("parent"), outside.join("moved"), "lookup_move"));
        assert_eq!(
            root.parent("parent/child/file", true, Context::Output)
                .err()
                .unwrap()
                .class,
            Class::PublishFailed
        );
        assert_eq!(host::read_dir(outside.join("moved")).unwrap().count(), 0);
    }
}

#[test]
fn moved_metadata_parent_refuses_lock_and_run_creates() {
    for fallback in resolvers() {
        for point in ["metadata_open", "lookup_move"] {
            let scratch = Scratch::new("directory-metadata-revalidate");
            let outside = Scratch::new("directory-metadata-outside");
            host::create_dir(scratch.join("_unityrs")).unwrap();
            let adapter = fs(fallback);
            *adapter.race.lock().unwrap() =
                Some((scratch.join("_unityrs"), outside.join("moved"), point));
            assert_eq!(
                preflight(scratch.path(), 1, adapter).err().unwrap().class,
                Class::SinkRootUnusable
            );
            assert_eq!(
                host::read_dir(outside.join("moved")).unwrap().count(),
                usize::from(point == "lookup_move")
            );
            assert!(!outside.join("moved/r").exists());
        }
    }
}

#[test]
fn nested_unicode_output_parents_remain_writable_without_per_output_syncs() {
    for fallback in resolvers() {
        let scratch = Scratch::new("directory-nested-prefix");
        let adapter = fs(fallback);
        let root = Root::open(scratch.path(), adapter.clone()).unwrap();
        let parent = root
            .parent("one/twø/three/file", true, Context::Output)
            .unwrap();
        let temp = parent.temporary("r", 0, Context::Output).unwrap();
        temp.write_all(b"new").unwrap();
        temp.writeback().unwrap();
        temp.rename("file").unwrap();
        assert_eq!(
            host::read(scratch.join("one/twø/three/file")).unwrap(),
            b"new"
        );
        assert_eq!(adapter.synced.lock().unwrap().len(), 0);
    }
}
