use std::{cell::Cell, fs, os::unix::fs::symlink, sync::Arc};
use unityrs_batch::spool::{Counter, Directory, Error, Host};
use unityrs_testkit::{guard, scratch::Scratch};

#[test]
fn partial_cooperative_removal_retains_the_whole_claim_and_never_follows_symlinks() {
    let () = guard::finishes("cooperative no-follow cleanup", || {
        let scratch = Scratch::new("cooperative-removal");
        fs::create_dir(scratch.join("outside")).unwrap();
        fs::write(scratch.join("outside/keep"), b"keep").unwrap();
        let counter = Counter::new(64 << 20, 0, 4_096).unwrap();
        let mut directory =
            Directory::create(scratch.join("owned"), counter.clone(), Arc::new(Host)).unwrap();
        for i in 0..1_501 {
            let mut file = directory.output(i).unwrap();
            file.append(b"x").unwrap();
            file.finish().unwrap();
        }
        symlink(scratch.join("outside"), scratch.join("owned/link")).unwrap();
        let claim = directory.claimed();
        let checks = Cell::new(0);
        let result = directory.remove_stoppable(&|| {
            let old = checks.get();
            checks.set(old + 1);
            old >= 2
        });
        assert!(matches!(result, Err(Error::Stopped)));
        assert_eq!(checks.get(), 3);
        assert_eq!(directory.claimed(), claim);
        let remaining = fs::read_dir(directory.path()).unwrap().count();
        assert!(
            (501..=503).contains(&remaining),
            "one check per 1,000 entries: {remaining}"
        );
        assert_eq!(fs::read(scratch.join("outside/keep")).unwrap(), b"keep");
        directory.remove_stoppable(&|| false).unwrap();
        assert_eq!(directory.claimed(), 0);
        assert_eq!(fs::read(scratch.join("outside/keep")).unwrap(), b"keep");
    });
}

#[test]
fn a_stop_before_removal_leaves_every_artifact_and_claim() {
    let () = guard::finishes("cleanup starts stopped", || {
        let scratch = Scratch::new("stopped-removal");
        let mut directory = Directory::create(
            scratch.join("owned"),
            Counter::new(1 << 20, 0, 4_096).unwrap(),
            Arc::new(Host),
        )
        .unwrap();
        let mut file = directory.output(0).unwrap();
        file.append(b"x").unwrap();
        file.finish().unwrap();
        assert!(matches!(
            directory.remove_stoppable(&|| true),
            Err(Error::Stopped)
        ));
        assert_eq!(directory.claimed(), 4_096);
        assert_eq!(fs::read(directory.path().join("o0")).unwrap(), b"x");
    });
}

#[test]
fn swapping_an_ancestor_after_the_stop_check_keeps_outside_files_and_claims() {
    let () = guard::finishes("ancestor swap during cleanup", || {
        let scratch = Scratch::new("ancestor-swap");
        fs::create_dir(scratch.join("ancestor")).unwrap();
        fs::create_dir(scratch.join("outside")).unwrap();
        fs::create_dir(scratch.join("outside/owned")).unwrap();
        fs::write(scratch.join("outside/owned/o0"), b"outside").unwrap();
        let counter = Counter::new(1 << 20, 0, 4_096).unwrap();
        let mut directory = Directory::create(
            scratch.join("ancestor/owned"),
            counter.clone(),
            Arc::new(Host),
        )
        .unwrap();
        let mut file = directory.output(0).unwrap();
        file.append(b"owned").unwrap();
        file.finish().unwrap();
        let checks = Cell::new(0);
        let result = directory.remove_stoppable(&|| {
            let old = checks.replace(checks.get() + 1);
            if old == 0 {
                fs::rename(scratch.join("ancestor"), scratch.join("original")).unwrap();
                symlink(scratch.join("outside"), scratch.join("ancestor")).unwrap();
            }
            false
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(scratch.join("outside/owned/o0")).unwrap(),
            b"outside"
        );
        assert_eq!(
            fs::read(scratch.join("original/owned/o0")).unwrap(),
            b"owned"
        );
        assert_eq!(directory.claimed(), 4_096);
        assert_eq!(counter.used(), 4_096);
    });
}

#[test]
fn excessive_cleanup_depth_is_internal_and_keeps_the_whole_claim() {
    let () = guard::finishes("bounded cleanup depth", || {
        let scratch = Scratch::new("removal-depth");
        let counter = Counter::new(1 << 20, 0, 4_096).unwrap();
        let mut directory =
            Directory::create(scratch.join("owned"), counter.clone(), Arc::new(Host)).unwrap();
        let mut file = directory.output(0).unwrap();
        file.append(b"claim").unwrap();
        file.finish().unwrap();
        let mut deepest = directory.path().to_path_buf();
        for _ in 0..8 {
            deepest.push("nested");
            fs::create_dir(&deepest).unwrap();
        }
        fs::write(deepest.join("keep"), b"deep").unwrap();
        assert!(matches!(
            directory.remove_stoppable(&|| false),
            Err(Error::Internal)
        ));
        assert_eq!(directory.claimed(), 4_096);
        assert_eq!(counter.used(), 4_096);
        assert_eq!(fs::read(deepest.join("keep")).unwrap(), b"deep");
    });
}

#[test]
fn swapping_an_ancestor_before_top_unlink_uses_the_open_parent() {
    let () = guard::finishes("descriptor anchored top unlink", || {
        let scratch = Scratch::new("top-unlink-swap");
        fs::create_dir(scratch.join("ancestor")).unwrap();
        fs::create_dir(scratch.join("outside")).unwrap();
        fs::create_dir(scratch.join("outside/owned")).unwrap();
        fs::write(scratch.join("outside/owned/keep"), b"outside").unwrap();
        let counter = Counter::new(1 << 20, 0, 4_096).unwrap();
        let mut directory = Directory::create(
            scratch.join("ancestor/owned"),
            counter.clone(),
            Arc::new(Host),
        )
        .unwrap();
        let mut file = directory.output(0).unwrap();
        file.append(b"owned").unwrap();
        file.finish().unwrap();
        let checks = Cell::new(0);
        directory
            .remove_stoppable(&|| {
                let old = checks.replace(checks.get() + 1);
                if old == 2 {
                    fs::rename(scratch.join("ancestor"), scratch.join("original")).unwrap();
                    symlink(scratch.join("outside"), scratch.join("ancestor")).unwrap();
                }
                false
            })
            .unwrap();
        assert_eq!(checks.get(), 3);
        assert_eq!(
            fs::read(scratch.join("outside/owned/keep")).unwrap(),
            b"outside"
        );
        assert!(!scratch.join("original/owned").exists());
        assert_eq!(directory.claimed(), 0);
        assert_eq!(counter.used(), 0);
    });
}

#[test]
fn renamed_directory_cleanup_removes_its_new_anchored_location() {
    let () = guard::finishes("renamed owner cleanup", || {
        let scratch = Scratch::new("renamed-cleanup");
        let mut directory = Directory::create(
            scratch.join("owned"),
            Counter::new(1 << 20, 0, 4_096).unwrap(),
            Arc::new(Host),
        )
        .unwrap();
        let mut file = directory.output(0).unwrap();
        file.append(b"owned").unwrap();
        file.finish().unwrap();
        directory.rename(scratch.join("renamed")).unwrap();
        directory.remove_stoppable(&|| false).unwrap();
        assert!(!scratch.join("renamed").exists());
        assert_eq!(directory.claimed(), 0);
    });
}

#[test]
fn cleanup_accepts_the_last_permitted_directory_level() {
    let () = guard::finishes("cleanup depth boundary", || {
        let scratch = Scratch::new("removal-depth-boundary");
        let mut directory = Directory::create(
            scratch.join("owned"),
            Counter::new(1 << 20, 0, 4_096).unwrap(),
            Arc::new(Host),
        )
        .unwrap();
        let mut deepest = directory.path().to_path_buf();
        for _ in 0..7 {
            deepest.push("nested");
            fs::create_dir(&deepest).unwrap();
        }
        fs::write(deepest.join("file"), b"deep").unwrap();
        directory.remove_stoppable(&|| false).unwrap();
        assert!(!directory.path().exists());
    });
}

#[test]
fn relative_directory_child() {
    if std::env::var_os("UNITYRS_RELATIVE_DIRECTORY_CHILD").is_none() {
        return;
    }
    fs::create_dir("area").unwrap();
    for relative in ["owned", "area/owned"] {
        let counter = Counter::new(1 << 20, 0, 4_096).unwrap();
        let mut directory =
            Directory::create(relative.into(), counter.clone(), Arc::new(Host)).unwrap();
        let mut file = directory.output(0).unwrap();
        file.append(b"owned").unwrap();
        file.finish().unwrap();
        let renamed = std::path::PathBuf::from(format!("{relative}-renamed"));
        directory.rename(renamed.clone()).unwrap();
        directory.remove_stoppable(&|| false).unwrap();
        assert!(!renamed.exists());
        assert_eq!(counter.used(), 0);
    }
}

#[test]
fn relative_directory_owners_can_rename_and_remove_from_the_pinned_root() {
    let () = guard::finishes("relative directory ownership", || {
        let scratch = Scratch::new("relative-directory");
        let output = unityrs_testkit::child::output(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "relative_directory_child", "--nocapture"])
                .env("UNITYRS_RELATIVE_DIRECTORY_CHILD", "1")
                .current_dir(scratch.path()),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    });
}
