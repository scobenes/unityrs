mod artifact_support;
mod stage_support;
use artifact_support::Faults;
use std::{
    cell::Cell,
    fs,
    io::{self, Read},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use unityrs_batch::{
    attempt::{DetailCode, ReasonCode},
    spool::{Counter, OwnedFile},
    stage::{
        self, DataFailure as D, Failure as F,
        local::{self, Hooks, Point, Snapshot, Source, Sources},
    },
};
use unityrs_testkit::{guard, scratch::Scratch};

const SHA_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn local_files_are_pinned_hashed_sealed_and_claim_initial_size_only() {
    for bytes in [vec![], b"abc".to_vec(), vec![17; local::BUFFER + 1]] {
        let scratch = Scratch::new("stage-pinned");
        let source = scratch.join("source");
        fs::write(&source, &bytes).unwrap();
        let expected = if bytes.is_empty() {
            Some(SHA_EMPTY)
        } else if bytes == b"abc" {
            Some(SHA_ABC)
        } else {
            None
        };
        let validated = stage_support::spec(&source, Some(bytes.len() as u64), expected);
        stage::require_local(&validated.spec).unwrap();
        let copy = local::open(validated.spec.bundle(0).unwrap(), bytes.len() as u64).unwrap();
        let counter = Counter::new(1 << 20, 0, 6000).unwrap();
        let mut file = OwnedFile::create(
            scratch.join("i0"),
            counter.clone(),
            Arc::new(Faults::default()),
        )
        .unwrap();
        let end = stage_support::run(copy, 0, &mut file, &|| false, &local::NoHooks);
        assert_eq!(end.failure, None, "{end:?}");
        assert_eq!(end.facts.unwrap().size, bytes.len() as u64);
        if let Some(expected) = expected {
            assert_eq!(
                format!("{}", unityrs_batch::digest::Hex(&end.facts.unwrap().sha256)),
                expected
            );
        }
        assert_eq!(
            counter.used(),
            unityrs_batch::work::rounded(bytes.len() as u64, 6000).unwrap()
        );
        assert_eq!(end.claimed_bytes, counter.used());
        assert_eq!(
            fs::metadata(file.path()).unwrap().permissions().mode() & 0o777,
            0o400
        );
        fs::write(&source, b"changed after staging").unwrap();
        fs::remove_file(&source).unwrap();
        assert_eq!(fs::read(file.path()).unwrap(), bytes);
        file.remove().unwrap();
        assert_eq!(counter.used(), 0);
    }
}

#[test]
fn stated_size_then_source_open_then_regular_then_fstat_size_precedence() {
    let scratch = Scratch::new("stage-open");
    let missing = scratch.join("missing");
    let v = stage_support::spec(&missing, Some(11), None);
    assert!(matches!(
        local::open(v.spec.bundle(0).unwrap(), 10),
        Err(F::Data(D::InputBytes(10)))
    ));
    let v = stage_support::spec(&missing, None, None);
    assert!(matches!(
        local::open(v.spec.bundle(0).unwrap(), 10),
        Err(F::Data(D::SourceMissing))
    ));
    let v = stage_support::spec(scratch.path(), None, None);
    assert!(matches!(
        local::open(v.spec.bundle(0).unwrap(), 0),
        Err(F::Data(D::SourceNotRegular))
    ));
    let source = scratch.join("source");
    fs::write(&source, [1; 11]).unwrap();
    let v = stage_support::spec(&source, Some(1), None);
    assert!(matches!(
        local::open(v.spec.bundle(0).unwrap(), 10),
        Err(F::Data(D::InputBytes(10)))
    ));
    // A nonblocking FIFO open must return without a writer.
    let path = scratch.join("fifo");
    let out =
        unityrs_testkit::child::output(std::process::Command::new("mkfifo").arg(&path)).unwrap();
    assert!(out.status.success());
    let v = stage_support::spec(&path, None, None);
    let () = guard::finishes("nonblocking FIFO source", move || {
        assert!(matches!(
            local::open(v.spec.bundle(0).unwrap(), 10),
            Err(F::Data(D::SourceNotRegular))
        ));
    });
    let v = stage_support::spec(&source, None, None);
    assert!(matches!(
        local::open_with(v.spec.bundle(0).unwrap(), 20, &Denied),
        Err(F::Data(D::SourceMissing))
    ));
}

/// Forces an open refusal independent of the test uid's privileges.
struct Denied;
impl Sources for Denied {
    fn open(&self, _: &Path) -> io::Result<Box<dyn Source>> {
        Err(io::Error::from_raw_os_error(libc::EACCES))
    }
}

/// Raises the stop once the first full buffer has been copied.
struct Stop<'a>(&'a AtomicBool);
impl Hooks for Stop<'_> {
    fn point(&self, point: Point) {
        if point == Point::Buffer(local::BUFFER as u64) {
            self.0.store(true, Ordering::Release);
        }
    }
}

struct Change<'a> {
    path: &'a Path,
    action: &'a str,
    once: Cell<bool>,
}
impl Hooks for Change<'_> {
    fn point(&self, point: Point) {
        if point != Point::Buffer(0) || self.once.replace(true) {
            return;
        }
        match self.action {
            "truncate" => fs::write(self.path, b"").unwrap(),
            "grow" => {
                use std::io::Write;
                fs::OpenOptions::new()
                    .append(true)
                    .open(self.path)
                    .unwrap()
                    .write_all(b"more")
                    .unwrap();
            }
            "rewrite" => {
                fs::write(self.path, [9; 100]).unwrap();
                fs::File::open(self.path)
                    .unwrap()
                    .set_modified(
                        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(17),
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
    }
}
#[test]
fn changed_source_wins_over_eof_size_and_digest_and_returns_no_facts() {
    for action in ["truncate", "grow", "rewrite"] {
        let scratch = Scratch::new("stage-change");
        let source = scratch.join("source");
        fs::write(&source, [1; 100]).unwrap();
        let v = stage_support::spec(&source, Some(99), Some(SHA_EMPTY));
        let copy = local::open(v.spec.bundle(0).unwrap(), 100).unwrap();
        let counter = Counter::new(4096, 0, 4096).unwrap();
        let mut file = OwnedFile::create(
            scratch.join("i0"),
            counter.clone(),
            Arc::new(Faults::default()),
        )
        .unwrap();
        let end = stage_support::run(
            copy,
            0,
            &mut file,
            &|| false,
            &Change {
                path: &source,
                action,
                once: Cell::new(false),
            },
        );
        assert_eq!(end.failure, Some(F::Data(D::SourceChanged)), "{action}");
        assert_eq!(end.facts, None);
        assert_eq!(counter.used(), 0);
        assert!(!file.path().exists());
    }
}

#[test]
fn whole_body_integrity_mismatches_preserve_received_facts_and_delete_claims() {
    for (size, sha) in [
        (Some(2), Some(SHA_EMPTY)),
        (Some(4), None),
        (Some(3), Some(SHA_EMPTY)),
    ] {
        let scratch = Scratch::new("stage-integrity");
        let source = scratch.join("source");
        fs::write(&source, b"abc").unwrap();
        let v = stage_support::spec(&source, size, sha);
        let copy = local::open(v.spec.bundle(0).unwrap(), 4).unwrap();
        let counter = Counter::new(4096, 0, 4096).unwrap();
        let mut file = OwnedFile::create(
            scratch.join("i0"),
            counter.clone(),
            Arc::new(Faults::default()),
        )
        .unwrap();
        let end = stage_support::run(copy, 0, &mut file, &|| false, &local::NoHooks);
        assert_eq!(end.failure, Some(F::Data(D::IntegrityMismatch)));
        assert_eq!(end.facts.unwrap().size, 3);
        assert_eq!(
            format!("{}", unityrs_batch::digest::Hex(&end.facts.unwrap().sha256)),
            SHA_ABC
        );
        assert_eq!(counter.used(), 0);
        assert!(!file.path().exists());
    }
}

struct ShortSource {
    file: fs::File,
    mode: u8,
}
impl Read for ShortSource {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        if self.mode == 1 {
            Err(io::Error::from_raw_os_error(libc::EIO))
        } else {
            Ok(0)
        }
    }
}
impl Source for ShortSource {
    fn snapshot(&self) -> io::Result<Snapshot> {
        let mut snapshot = Snapshot::of(&self.file.metadata()?);
        if self.mode == 2 {
            snapshot.size += 1;
        }
        Ok(snapshot)
    }
}
struct Short<'a>(&'a Path, u8);
impl Sources for Short<'_> {
    fn open(&self, _: &Path) -> io::Result<Box<dyn Source>> {
        Ok(Box::new(ShortSource {
            file: fs::File::open(self.0)?,
            mode: self.1,
        }))
    }
}
#[test]
fn unchanged_early_eof_and_opened_source_read_error_stop_host_io() {
    for mode in [0, 1] {
        let scratch = Scratch::new("stage-read");
        let source = scratch.join("source");
        fs::write(&source, b"abc").unwrap();
        let v = stage_support::spec(&source, None, None);
        let copy = local::open_with(v.spec.bundle(0).unwrap(), 10, &Short(&source, mode)).unwrap();
        let counter = Counter::new(4096, 0, 4096).unwrap();
        let mut file = OwnedFile::create(
            scratch.join("i0"),
            counter.clone(),
            Arc::new(Faults::default()),
        )
        .unwrap();
        let end = stage_support::run(copy, 0, &mut file, &|| false, &local::NoHooks);
        assert!(matches!(end.failure, Some(F::Run(f)) if f.reason == ReasonCode::HostIo));
        assert_eq!(end.facts, None);
        assert_eq!(counter.used(), 4096);
        assert!(file.path().exists());
    }
}

#[test]
fn spool_write_seal_and_cleanup_faults_are_run_failures_with_exact_details() {
    for operation in [2, 3, 5, 6] {
        for (errno, detail) in [
            (libc::ENOSPC, DetailCode::Enospc),
            (libc::EDQUOT, DetailCode::Edquot),
        ] {
            let scratch = Scratch::new("stage-spool");
            let source = scratch.join("source");
            fs::write(&source, b"abc").unwrap();
            let v = stage_support::spec(&source, (operation == 6).then_some(2), None);
            let copy = local::open(v.spec.bundle(0).unwrap(), 10).unwrap();
            let counter = Counter::new(4096, 0, 4096).unwrap();
            let faults = Arc::new(Faults::default());
            let mut file =
                OwnedFile::create(scratch.join("i0"), counter.clone(), faults.clone()).unwrap();
            faults.set(operation, errno);
            let end = stage_support::run(copy, 0, &mut file, &|| false, &local::NoHooks);
            assert!(
                matches!(end.failure, Some(F::Run(f)) if f.reason == ReasonCode::SpoolFull && f.detail == Some(detail)),
                "{end:?}"
            );
            assert_eq!(counter.used(), 4096);
            assert!(file.path().exists());
            faults.set(0, 0);
            file.remove().unwrap();
            assert_eq!(counter.used(), 0);
        }
    }
}

#[test]
fn stop_during_copy_keeps_partial_bytes_and_the_full_upfront_claim() {
    let scratch = Scratch::new("stage-stop");
    let source = scratch.join("source");
    fs::write(&source, vec![3; local::BUFFER + 1]).unwrap();
    let v = stage_support::spec(&source, None, None);
    let copy = local::open(v.spec.bundle(0).unwrap(), 1 << 20).unwrap();
    let counter = Counter::new(1 << 20, 0, 4096).unwrap();
    let mut file = OwnedFile::create(
        scratch.join("i0"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let stop = AtomicBool::new(false);
    let end = stage_support::run(
        copy,
        0,
        &mut file,
        &|| stop.load(Ordering::Acquire),
        &Stop(&stop),
    );
    assert_eq!(end.failure, Some(F::Stopped));
    assert_eq!(end.facts, None);
    assert_eq!(fs::read(file.path()).unwrap(), vec![3; local::BUFFER]);
    assert_eq!(counter.used(), local::BUFFER as u64 + 4096);
}

#[test]
fn local_capability_rejects_even_unused_url_dependencies_before_any_write() {
    let v = stage_support::from_bundles(
        &[serde_json::json!({"id":"x","path":"/source"})],
        &[serde_json::json!({"id":"unused","url":"https://example.net/bundle"})],
    );
    assert_eq!(stage::require_local(&v.spec), Err(stage::UrlUnsupported));
    assert_eq!(
        stage::path(Path::new("in"), 0, 2, false),
        Path::new("in/i0")
    );
    assert_eq!(
        stage::path(Path::new("in"), 2, 2, false),
        Path::new("in/d0")
    );
}

#[test]
fn larger_expected_size_is_never_claimed_and_cleanup_panics_return_fixed_ends() {
    struct PanicDelete;
    impl unityrs_batch::spool::FileSystem for PanicDelete {
        fn available(&self, _: &Path) -> io::Result<u64> {
            Ok(u64::MAX)
        }
        fn remove_file(&self, _: &Path) -> io::Result<()> {
            panic!("cleanup seam");
        }
    }
    let scratch = Scratch::new("stage-cleanup");
    let source = scratch.join("source");
    fs::write(&source, b"abc").unwrap();
    let v = stage_support::spec(&source, Some(6001), None);
    for panic_delete in [false, true] {
        let copy = local::open(v.spec.bundle(0).unwrap(), 6001).unwrap();
        let counter = Counter::new(6000, 0, 6000).unwrap();
        let fs: Arc<dyn unityrs_batch::spool::FileSystem> = if panic_delete {
            Arc::new(PanicDelete)
        } else {
            Arc::new(Faults::default())
        };
        let mut file = OwnedFile::create(
            scratch.join(if panic_delete { "i1" } else { "i0" }),
            counter.clone(),
            fs,
        )
        .unwrap();
        let end = stage_support::run(copy, 0, &mut file, &|| false, &local::NoHooks);
        if panic_delete {
            assert!(matches!(end.failure, Some(F::Run(f)) if f.reason == ReasonCode::Internal));
            assert_eq!(counter.used(), 6000);
            assert_eq!(fs::read(file.path()).unwrap(), b"abc");
        } else {
            assert_eq!(end.failure, Some(F::Data(D::IntegrityMismatch)));
            assert_eq!(counter.peak(), 6000);
            assert_eq!(counter.used(), 0);
            assert!(!file.path().exists());
        }
    }
}

#[test]
fn staging_under_owner_read_mask_seals_on_the_open_descriptor() {
    const SOURCE: &str = "UNITYRS_STAGE_UMASK_SOURCE";
    const DESTINATION: &str = "UNITYRS_STAGE_UMASK_DESTINATION";
    if let Some(source) = std::env::var_os(SOURCE) {
        let source = std::path::PathBuf::from(source);
        let v = stage_support::spec(&source, Some(3), Some(SHA_ABC));
        let copy = local::open(v.spec.bundle(0).unwrap(), 3).unwrap();
        let counter = Counter::new(4096, 0, 4096).unwrap();
        let mut file = OwnedFile::create(
            std::env::var_os(DESTINATION).unwrap().into(),
            counter,
            Arc::new(Faults::default()),
        )
        .unwrap();
        let end = stage_support::run(copy, 0, &mut file, &|| false, &local::NoHooks);
        assert_eq!(end.failure, None, "{end:?}");
        assert!(file.is_sealed());
        assert_eq!(
            fs::metadata(file.path()).unwrap().permissions().mode() & 0o777,
            0o400
        );
        return;
    }
    let scratch = Scratch::new("stage-umask");
    let source = scratch.join("source");
    let destination = scratch.join("i0");
    fs::write(&source, b"abc").unwrap();
    let result = unityrs_testkit::child::output(
        std::process::Command::new("sh")
            .args(["-c", "umask 0400; exec \"$1\" --exact staging_under_owner_read_mask_seals_on_the_open_descriptor --test-threads=1", "stage-umask"])
            .arg(std::env::current_exe().unwrap())
            .env(SOURCE, &source)
            .env(DESTINATION, &destination),
    ).unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert_eq!(fs::read(&destination).unwrap(), b"abc");
}

#[test]
fn copy_step_never_restats_or_seals() {
    struct Counted {
        file: fs::File,
        snapshots: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Read for Counted {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.file.read(bytes)
        }
    }
    impl Source for Counted {
        fn snapshot(&self) -> io::Result<Snapshot> {
            self.snapshots.fetch_add(1, Ordering::Relaxed);
            self.file.metadata().map(|m| Snapshot::of(&m))
        }
    }
    struct CountSources(Arc<std::sync::atomic::AtomicUsize>);
    impl Sources for CountSources {
        fn open(&self, path: &Path) -> io::Result<Box<dyn Source>> {
            Ok(Box::new(Counted {
                file: fs::File::open(path)?,
                snapshots: self.0.clone(),
            }))
        }
    }
    for early_eof in [false, true] {
        let scratch = Scratch::new("stage-copy-only");
        let source = scratch.join("source");
        fs::write(&source, b"abc").unwrap();
        let snapshots = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let v = stage_support::spec(&source, Some(2), Some(SHA_EMPTY));
        let copy = local::open_with(
            v.spec.bundle(0).unwrap(),
            3,
            &CountSources(snapshots.clone()),
        )
        .unwrap();
        if early_eof {
            fs::write(&source, b"").unwrap();
        }
        let counter = Counter::new(4096, 0, 4096).unwrap();
        let mut file = OwnedFile::create(
            scratch.join("i0"),
            counter.clone(),
            Arc::new(Faults::default()),
        )
        .unwrap();
        let pending = copy.run(0, &mut file, &|| false, &local::NoHooks);
        assert_eq!(snapshots.load(Ordering::Relaxed), 1);
        assert!(!file.is_sealed());
        assert!(file.path().exists());
        assert_eq!(counter.used(), 4096);
        let end = pending.finish(&mut file, &|| false, &local::NoHooks);
        assert_eq!(
            end.failure,
            Some(F::Data(if early_eof {
                D::SourceChanged
            } else {
                D::IntegrityMismatch
            }))
        );
        assert_eq!(snapshots.load(Ordering::Relaxed), 2);
        assert_eq!(counter.used(), 0);
    }
}

#[test]
fn same_size_rewrite_with_restored_mtime_is_source_changed() {
    let scratch = Scratch::new("stage-ctime");
    let source = scratch.join("source");
    fs::write(&source, b"abc").unwrap();
    let original = fs::metadata(&source).unwrap();
    let initial = Snapshot::of(&original);
    let v = stage_support::spec(&source, Some(3), Some(SHA_ABC));
    let copy = local::open(v.spec.bundle(0).unwrap(), 3).unwrap();
    let counter = Counter::new(4096, 0, 4096).unwrap();
    let mut file = OwnedFile::create(
        scratch.join("i0"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    let pending = copy.run(0, &mut file, &|| false, &local::NoHooks);
    // Cross even a filesystem's one-second ctime tick before rewriting.
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    fs::write(&source, b"xyz").unwrap();
    fs::File::open(&source)
        .unwrap()
        .set_modified(original.modified().unwrap())
        .unwrap();
    let rewritten = Snapshot::of(&fs::metadata(&source).unwrap());
    assert_eq!(rewritten.size, initial.size);
    assert_eq!(rewritten.modified, initial.modified);
    assert_ne!(rewritten.changed, initial.changed);
    assert_eq!(fs::read(&source).unwrap(), b"xyz");
    let end = pending.finish(&mut file, &|| false, &local::NoHooks);
    assert_eq!(end.failure, Some(F::Data(D::SourceChanged)));
    assert_eq!(end.facts, None);
    assert_eq!(counter.used(), 0);
    assert!(!file.path().exists());
}
