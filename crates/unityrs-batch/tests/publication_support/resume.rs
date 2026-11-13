use super::*;

#[test]
fn resume_leaf_replaced_between_check_and_open_is_a_miss() {
    for fallback in directory_support::resolvers() {
        for (kind, point) in [
            ("socket", "resume_checked"),
            ("symlink", "resume_checked"),
            ("regular", "resume_checked"),
            ("socket", "resume_open"),
            ("symlink", "resume_open"),
            ("regular", "resume_reopen"),
            ("socket", "resume_reopen"),
        ] {
            let scene = Scene::new(&["key"], false, 4096);
            let adapter = fs(fallback);
            let dir = directory(&scene, adapter.clone());
            let path = scene.scratch.join("sink/key");
            let replacement = scene.scratch.join("replacement");
            std::fs::write(&path, b"payload").unwrap();
            let _socket = match kind {
                "socket" => Some(std::os::unix::net::UnixListener::bind(&replacement).unwrap()),
                "symlink" => {
                    std::os::unix::fs::symlink("/dev/null", &replacement).unwrap();
                    None
                }
                _ => {
                    std::fs::write(&replacement, b"payload").unwrap();
                    None
                }
            };
            *adapter.race.lock().unwrap() = Some((path.clone(), replacement, point));
            let out = scene.outcomes("out", &Host);
            let result = drive(
                &scene,
                &dir,
                &out,
                &Gate::default(),
                true,
                &publish::NoHooks,
                &AtomicBool::new(false),
            );
            assert!(
                matches!(
                    result,
                    Ok(Admission::Drained(publish::Report {
                        written: 1,
                        present: 0,
                        failed: 0
                    }))
                ),
                "{fallback} {kind}: {result:?}"
            );
            assert!(
                adapter.race.lock().unwrap().is_none(),
                "replacement hook reached"
            );
            assert_eq!(out.loose(0).unwrap(), Code::Written);
            assert_eq!(std::fs::read(path).unwrap(), b"payload");
        }
    }
}

#[test]
fn resume_reopen_misses_do_not_abort_and_real_io_errors_do() {
    for errno in [
        libc::ENXIO,
        libc::EOPNOTSUPP,
        libc::ELOOP,
        libc::ENOENT,
        libc::EIO,
    ] {
        let scene = Scene::new(&["key"], false, 4096);
        let adapter = Arc::new(Faults {
            fallback: Some(libc::ENOSYS),
            fail: Some(("reopen", errno)),
            disarmed: AtomicBool::new(true),
            crossing: AtomicUsize::new(usize::MAX),
            ..Faults::default()
        });
        let dir = directory(&scene, adapter.clone());
        std::fs::write(scene.scratch.join("sink/key"), b"payload").unwrap();
        adapter.disarmed.store(false, Ordering::SeqCst);
        let out = scene.outcomes("out", &Host);
        let result = drive(
            &scene,
            &dir,
            &out,
            &Gate::default(),
            true,
            &publish::NoHooks,
            &AtomicBool::new(false),
        );
        if errno == libc::EIO {
            assert!(matches!(result, Err(Error::Run(f))
                if f.reason == unityrs_batch::attempt::ReasonCode::HostIo));
            assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
        } else {
            assert!(
                matches!(
                    result,
                    Ok(Admission::Drained(publish::Report { written: 1, .. }))
                ),
                "{errno}: {result:?}"
            );
            assert_eq!(out.loose(0).unwrap(), Code::Written);
        }
    }
}
#[test]
fn resume_reopened_different_inode_with_matching_bytes_is_a_miss() {
    let scene = Scene::new(&["key"], false, 4096);
    let adapter = fs(true);
    let dir = directory(&scene, adapter.clone());
    let path = scene.scratch.join("sink/key");
    let other = scene.scratch.join("other");
    std::fs::write(&path, b"payload").unwrap();
    std::fs::write(&other, b"payload").unwrap();
    *adapter.race.lock().unwrap() = Some((path, other, "resume_wrong_inode"));
    let out = scene.outcomes("out", &Host);
    let result = drive(
        &scene,
        &dir,
        &out,
        &Gate::default(),
        true,
        &publish::NoHooks,
        &AtomicBool::new(false),
    );
    assert!(
        matches!(
            result,
            Ok(Admission::Drained(publish::Report { written: 1, .. }))
        ),
        "{result:?}"
    );
    assert!(adapter.race.lock().unwrap().is_none());
    assert_eq!(out.loose(0).unwrap(), Code::Written);
}
