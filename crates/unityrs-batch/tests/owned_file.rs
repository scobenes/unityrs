mod artifact_support;
use artifact_support::Faults;
use std::{
    io,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use unityrs_batch::spool::{Counter, Error, FileSystem, Full, OwnedFile};
use unityrs_testkit::scratch::Scratch;

#[test]
fn upfront_and_growing_claims_round_once_seal_and_explicit_delete() {
    let scratch = Scratch::new("owned-file");
    let counter = Counter::new(18000, 0, 6000).unwrap();
    let mut file = OwnedFile::create(
        scratch.join("i0"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    file.reserve(6001).unwrap();
    assert_eq!(counter.used(), 12000);
    file.append(&[7; 6000], &|| false).unwrap();
    file.append(&[8; 1], &|| false).unwrap();
    assert_eq!(counter.used(), 12000);
    file.append(&[9; 6000], &|| false).unwrap();
    assert_eq!(counter.used(), 18000);
    file.seal(&|| false).unwrap();
    assert!(file.is_sealed());
    let bytes = std::fs::read(file.path()).unwrap();
    assert_eq!(bytes.len(), 12001);
    assert_eq!(&bytes[..6000], &[7; 6000]);
    assert_eq!(bytes[6000], 8);
    assert_eq!(
        std::fs::metadata(file.path()).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert!(file.append(b"no", &|| false).is_err());
    assert!(
        OwnedFile::create(
            file.path().into(),
            counter.clone(),
            Arc::new(Faults::default())
        )
        .is_err()
    );
    file.remove().unwrap();
    assert_eq!(counter.used(), 0);
    assert!(!file.path().exists());
    assert!(file.remove().is_err());
}

#[test]
fn spool_faults_keep_the_whole_claim_until_delete_succeeds() {
    for operation in [1, 2, 3, 5, 6] {
        for (errno, full) in [(libc::ENOSPC, Full::Enospc), (libc::EDQUOT, Full::Edquot)] {
            let scratch = Scratch::new("owned-fault");
            let faults = Arc::new(Faults::default());
            let counter = Counter::new(8192, 0, 4096).unwrap();
            if operation == 1 {
                faults.set(operation, errno);
                assert!(
                    matches!(OwnedFile::create(scratch.join("i0"), counter.clone(), faults), Err(Error::Full(f)) if f == full)
                );
                assert_eq!(counter.used(), 0);
                continue;
            }
            let mut file =
                OwnedFile::create(scratch.join("i0"), counter.clone(), faults.clone()).unwrap();
            file.reserve(4097).unwrap();
            faults.set(operation, errno);
            let error = match operation {
                2 => file.append(&[3; 20], &|| false).unwrap_err(),
                3 | 5 => {
                    file.append(&[3; 20], &|| false).unwrap();
                    file.seal(&|| false).unwrap_err()
                }
                6 => file.remove().unwrap_err(),
                _ => unreachable!(),
            };
            assert!(matches!(error, Error::Full(f) if f == full));
            assert_eq!(counter.used(), 8192);
            if operation == 2 {
                assert_eq!(std::fs::read(file.path()).unwrap(), [3; 7]);
            }
            if operation != 6 {
                assert!(file.append(b"poisoned", &|| false).is_err());
            }
            faults.set(0, 0);
            file.remove().unwrap();
            assert_eq!(counter.used(), 0);
        }
    }
}

#[test]
fn drop_and_stop_never_release_a_claim_and_empty_files_claim_zero() {
    let scratch = Scratch::new("owned-drop");
    let counter = Counter::new(4096, 0, 4096).unwrap();
    let mut file = OwnedFile::create(
        scratch.join("i0"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    file.reserve(0).unwrap();
    file.seal(&|| false).unwrap();
    assert_eq!(std::fs::read(file.path()).unwrap(), Vec::<u8>::new());
    assert_eq!(counter.used(), 0);
    let mut file = OwnedFile::create(
        scratch.join("d0"),
        counter.clone(),
        Arc::new(Faults::default()),
    )
    .unwrap();
    file.reserve(1).unwrap();
    assert!(matches!(file.append(b"x", &|| true), Err(Error::Stopped)));
    assert_eq!(std::fs::read(file.path()).unwrap(), Vec::<u8>::new());
    drop(file);
    assert_eq!(counter.used(), 4096);
}

struct Available(AtomicU64);
impl FileSystem for Available {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}
#[test]
fn cap_reserve_and_overflow_refuse_before_the_write() {
    let scratch = Scratch::new("owned-cap");
    let facts = Arc::new(Available(AtomicU64::new(10)));
    let counter = Counter::new(4096, 10, 4096).unwrap();
    let mut file = OwnedFile::create(scratch.join("i0"), counter.clone(), facts.clone()).unwrap();
    file.reserve(4096).unwrap(); // Both equalities succeed.
    facts.0.store(9, Ordering::Relaxed);
    assert!(matches!(
        file.append(b"x", &|| false),
        Err(Error::Full(Full::Reserve))
    ));
    assert_eq!(std::fs::read(file.path()).unwrap(), Vec::<u8>::new());
    assert_eq!(counter.used(), 4096);
    file.remove().unwrap();
    facts.0.store(10, Ordering::Relaxed);
    for (name, size) in [("d0", 4097), ("d1", u64::MAX)] {
        let mut file =
            OwnedFile::create(scratch.join(name), counter.clone(), facts.clone()).unwrap();
        assert!(matches!(
            file.reserve(size),
            Err(Error::Full(Full::SizeBytes))
        ));
        assert_eq!(counter.used(), 0);
        assert_eq!(std::fs::read(file.path()).unwrap(), Vec::<u8>::new());
        file.remove().unwrap();
    }
}

#[test]
fn positional_writes_recheck_free_space_inside_an_upfront_reservation() {
    let scratch = Scratch::new("owned-positional-reserve");
    let facts = Arc::new(Available(AtomicU64::new(10)));
    let counter = Counter::new(4096, 10, 4096).unwrap();
    let mut file = OwnedFile::create(scratch.join("i0"), counter.clone(), facts.clone()).unwrap();
    file.reserve(4096).unwrap();
    file.write_at(b"a", 0, &|| false).unwrap();
    facts.0.store(9, Ordering::Relaxed);
    assert!(matches!(
        file.write_at(b"b", 1, &|| false),
        Err(Error::Full(Full::Reserve))
    ));
    assert_eq!(std::fs::read(file.path()).unwrap(), b"a");
    assert_eq!(file.length(), 1);
    assert_eq!(counter.used(), 4096);
    file.remove().unwrap();
    assert_eq!(counter.used(), 0);
}
