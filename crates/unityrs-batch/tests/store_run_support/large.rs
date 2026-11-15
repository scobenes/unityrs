//! Whole-run multipart and crash recovery with real loopback requests.
use super::*;
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use unityrs_testkit::{child, guard::DEADLOCK_GUARD};

fn large_pixel(root: &Path) {
    let mut rng = unityrs_testkit::Rng::new(42);
    let mut pixels = vec![0; 2048 * 2048 * 4];
    for part in pixels.as_chunks_mut::<8>().0 {
        part.copy_from_slice(&rng.next_u64().to_le_bytes());
    }
    let file = unityrs_testkit::ngui::file(
        &[],
        &[unityrs_testkit::ngui::texture(
            1,
            b"pixel",
            4,
            (2048, 2048),
            pixels,
        )],
        true,
    )
    .unwrap();
    fs::write(
        root.join("input"),
        unityrs_testkit::env::bundle(&[("CAB-synthetic", file)]),
    )
    .unwrap();
}
#[test]
fn store_engine_child() {
    let Some(root) = std::env::var_os("B11D_ENGINE_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let spec: Value = serde_json::from_slice(&fs::read(root.join("job.json")).unwrap()).unwrap();
    #[cfg(all(feature = "test-hooks", target_os = "linux"))]
    if std::env::var_os("B11D_HOST").is_some() {
        let clock = unityrs_batch::progress::clock::Clock::process(Instant::now());
        let mut startup = run::process::Startup::prepare().unwrap();
        startup.hooks().skip_host_checks = true;
        startup.hooks().files = Some(run::hooks::FileSystems {
            spool: Arc::new(Space),
            sink: Arc::new(publication_support::Portable),
            started: Arc::new(unityrs_batch::spool::Host),
        });
        let prepared = phase1::prepare(
            unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap(),
        )
        .unwrap();
        run::process::run(startup, clock, prepared, 1 << 30, None);
    }
    let result = drive(&spec, &root).unwrap();
    std::process::exit(i32::from(result.code.value()));
}
fn command(root: &Path, hosted: bool) -> std::io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "large::store_engine_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("B11D_ENGINE_DIR", root)
        .env("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE")
        .env("AWS_SECRET_ACCESS_KEY", "secret")
        .env_remove("AWS_SESSION_TOKEN")
        .env_remove("AWS_CREDENTIAL_EXPIRATION")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if hosted {
        command.env("B11D_HOST", "1");
    }
    Ok(command)
}
#[test]
#[cfg_attr(debug_assertions, ignore = "scale: run in release")]
fn x10_kill_and_resume_loose_and_archive_multipart_from_local_and_url_sources() {
    store_support::run(async {
        for url in [false, true] {
            for killed_key in ["p/a/pixel.png", "p/all.zip"] {
                let fake = Arc::new(Fake::start().await);
                fake.schedule(
                    store_support::fake::Operation::Part,
                    killed_key,
                    1,
                    store_support::fake::Fault {
                        before_head: Some(DEADLOCK_GUARD),
                        ..Default::default()
                    },
                );
                for key in ["p/a/pixel.png", "p/all.zip"] {
                    fake.schedule(
                        store_support::fake::Operation::Complete,
                        key,
                        1,
                        store_support::fake::Fault {
                            lost_response: true,
                            ..Default::default()
                        },
                    );
                }
                let peer = fake.clone();
                let work = tokio::task::spawn_blocking(move || {
                    child::scene(|scene| {
                        let root = scene.scratch.path().to_owned();
                        let mut spec = make_spec(&scene.scratch, &peer);
                        large_pixel(&root);
                        spec["max_inflight_uncompressed_bytes"] = (512 * 1024 * 1024).into();
                        spec["max_decoded_bytes"] = (128 * 1024 * 1024).into();
                        spec["dep_cache_bytes"] = (128 * 1024 * 1024).into();
                        spec["archives"] = json!([{"key":"all.zip","members":[{"kind":"texture","path":"{name}.png"}]}]);
                        let origin = super::url_run_support::Origin::start();
                        if url { super::url_run_support::onto(&origin, &mut spec, |_| true); }
                        fs::write(root.join("job.json"), serde_json::to_vec(&spec).unwrap())?;
                        let hosted = cfg!(all(feature = "test-hooks", target_os = "linux")) && url;
                        let first = child::live(scene, &mut command(&root, hosted)?, false)?;
                        let (_err, _) = child::drain(scene.take_stderr(first).unwrap());
                        let deadline = Instant::now() + DEADLOCK_GUARD;
                        while !peer.logs().iter().any(|l| l.operation == store_support::fake::Operation::Part && l.raw_path == format!("/bucket/{killed_key}") && l.body_len > 0) {
                            assert!(scene.running(first)?, "engine ended before its multipart request");
                            assert!(Instant::now() < deadline, "multipart request guard");
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        scene.kill(first)?;
                        assert!(child::killed(scene.wait(first)?));
                        assert_eq!(peer.open_uploads(), 1);
                        let journal = unityrs_batch::spool::journal::Journal::new(&root.join("spool/runs/r")).unwrap();
                        let mut reader = journal.reader().unwrap();
                        assert_eq!(reader.next().unwrap().unwrap().entry().key, killed_key);
                        spec["attempt"] = 2.into(); spec["resume"] = true.into();
                        fs::write(root.join("job.json"), serde_json::to_vec(&spec).unwrap())?;
                        let second = child::live(scene, &mut command(&root, hosted)?, false)?;
                        let (_err, _) = child::drain(scene.take_stderr(second).unwrap());
                        assert_eq!(scene.wait(second)?.code(), Some(0));
                        assert_eq!(peer.open_uploads(), 0);
                        assert!(journal.reader().unwrap().next().unwrap().is_none());
                        let head = head(&peer);
                        assert_eq!(head["verified"], true);
                        assert!(head["unsettled_tries"].as_u64().unwrap() > 0);
                        assert_eq!(head["attempt"], 2);
                        assert_eq!(head["warnings"]["abort_failed"], 0);
                        for (key, mime) in [("p/a/pixel.png", "image/png"), ("p/all.zip", "application/zip")] {
                            let object = peer.object("bucket", key).unwrap();
                            assert!(object.bytes.len() > 16 * 1024 * 1024);
                            assert_eq!(object.content_type.as_deref(), Some(mime));
                            let expected = format!("{}", unityrs_batch::digest::Hashes::of(&object.bytes).etag);
                            assert!(unityrs_batch::store::etag::matches(object.etag.as_bytes(), expected.as_bytes()));
                        }
                        assert!(peer.logs().iter().filter(|l| l.operation == store_support::fake::Operation::Part).all(|l| l.body_len > 0 && l.body_len <= 16 * 1024 * 1024));
                        assert!(peer.logs().iter().any(|l| l.operation == store_support::fake::Operation::Abort));
                        assert!(peer.logs().iter().filter(|l| !(l.operation == store_support::fake::Operation::Part && l.raw_path == format!("/bucket/{killed_key}") && l.try_ordinal == 1)).all(|l| l.signature_valid));
                        Ok(())
                    }).unwrap();
                });
                work.await.unwrap();
            }
        }
    });
}
