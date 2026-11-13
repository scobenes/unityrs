//! Claims remain owned through partial writes, renames and failed deletion.
use std::io;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use unityrs_batch::spool::{Counter, Directory, Error, FileSystem, Full, Host, Writer};
use unityrs_testkit::scratch::Scratch;

struct Facts {
    available: AtomicU64,
    fail: AtomicU64,
}
impl Facts {
    fn new(available: u64) -> Arc<Self> {
        Arc::new(Self {
            available: AtomicU64::new(available),
            fail: AtomicU64::new(0),
        })
    }
}
impl FileSystem for Facts {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(self.available.load(Ordering::Relaxed))
    }
    fn create(&self, p: &Path) -> io::Result<Box<dyn Writer>> {
        if self.fail.load(Ordering::Relaxed) == 1 {
            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
        }
        Host.create(p)
    }
    fn remove(&self, p: &Path) -> io::Result<()> {
        if self.fail.load(Ordering::Relaxed) == 3 {
            std::fs::remove_file(p.join("o0"))?;
            // Removal of the second owned file fails after the first is gone.
            return Err(io::Error::from_raw_os_error(libc::EDQUOT));
        }
        if self.fail.load(Ordering::Relaxed) == 2 {
            return Err(io::Error::from_raw_os_error(libc::EDQUOT));
        }
        Host.remove(p)
    }
}

#[test]
fn cumulative_claims_use_actual_block_size_and_survive_rename() {
    let s = Scratch::new("claims");
    let facts = Facts::new(100);
    let c = Counter::new(12000, 100, 6000).unwrap();
    let mut d = Directory::create(s.join("attempt"), c.clone(), facts).unwrap();
    let mut f = d.records().unwrap();
    f.append(b"x").unwrap();
    f.append(&[0; 5999]).unwrap();
    assert_eq!(c.used(), 6000);
    f.append(b"y").unwrap();
    f.finish().unwrap();
    assert_eq!(c.used(), 12000);
    assert_eq!(
        std::fs::metadata(d.path().join("records.ndjson"))
            .unwrap()
            .len(),
        6001
    );
    d.rename(s.join("renamed")).unwrap();
    assert_eq!(c.used(), 12000);
    d.remove().unwrap();
    assert_eq!(c.used(), 0);
    assert_eq!(c.peak(), 12000);
}

#[test]
fn exact_cap_and_reserve_equality_succeed_one_over_does_not() {
    let s = Scratch::new("cap");
    let facts = Facts::new(10);
    let c = Counter::new(4096, 10, 4096).unwrap();
    let mut d = Directory::create(s.join("attempt"), c.clone(), facts.clone()).unwrap();
    let mut f = d.records().unwrap();
    f.append(&[0; 4096]).unwrap();
    assert!(matches!(f.append(b"!"), Err(Error::Full(Full::SizeBytes))));
    drop(f);
    assert_eq!(c.used(), 4096);
    d.remove().unwrap();
    let mut d = Directory::create(s.join("other"), c.clone(), facts.clone()).unwrap();
    facts.available.store(9, Ordering::Relaxed);
    let mut f = d.records().unwrap();
    assert!(matches!(f.append(b"!"), Err(Error::Full(Full::Reserve))));
    drop(f);
    d.remove().unwrap();
    assert_eq!(c.used(), 0);
}

#[test]
fn failed_deletion_and_dropping_owner_never_release_claims() {
    let s = Scratch::new("retain");
    let facts = Facts::new(u64::MAX);
    let c = Counter::new(4096, 0, 4096).unwrap();
    let mut d = Directory::create(s.join("attempt"), c.clone(), facts.clone()).unwrap();
    let mut f = d.records().unwrap();
    f.append(b"x").unwrap();
    f.finish().unwrap();
    facts.fail.store(2, Ordering::Relaxed);
    assert!(matches!(d.remove(), Err(Error::Full(Full::Edquot))));
    assert_eq!(d.claimed(), 4096);
    drop(d);
    assert_eq!(c.used(), 4096);
}

#[test]
fn partial_deletion_retains_the_entire_claim_until_a_successful_retry() {
    let s = Scratch::new("partial-delete");
    let facts = Facts::new(u64::MAX);
    let c = Counter::new(12288, 0, 4096).unwrap();
    let mut d = Directory::create(s.join("attempt"), c.clone(), facts.clone()).unwrap();
    for n in 0..2 {
        let mut file = d.output(n).unwrap();
        file.append(b"content").unwrap();
        file.finish().unwrap();
    }
    // A different owner's live claim exposes any duplicate release on retry.
    let mut other = Directory::create(s.join("other"), c.clone(), facts.clone()).unwrap();
    let mut file = other.records().unwrap();
    file.append(b"other").unwrap();
    file.finish().unwrap();
    assert_eq!(d.claimed(), 8192);
    assert_eq!(c.used(), 12288);
    facts.fail.store(3, Ordering::Relaxed);
    assert!(matches!(d.remove(), Err(Error::Full(Full::Edquot))));
    assert!(!d.path().join("o0").exists());
    assert_eq!(std::fs::read(d.path().join("o1")).unwrap(), b"content");
    assert_eq!(d.claimed(), 8192);
    assert_eq!(c.used(), 12288);

    facts.fail.store(0, Ordering::Relaxed);
    d.remove().unwrap();
    assert!(!d.path().exists());
    assert_eq!(d.claimed(), 0);
    assert_eq!(c.used(), 4096);
    assert!(matches!(d.remove(), Err(Error::Internal)));
    drop(d);
    assert_eq!(c.used(), 4096);
    other.remove().unwrap();
    assert_eq!(c.used(), 0);
    assert_eq!(c.peak(), 12288);
}

#[test]
fn overflow_zero_block_and_concurrent_claims() {
    assert!(Counter::new(1, 0, 0).is_err());
    let s = Scratch::new("contention");
    let c = Counter::new(4096, 0, 4096).unwrap();
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for n in 0..8 {
            let c = c.clone();
            let path = s.join(n.to_string());
            threads.push(scope.spawn(move || {
                let d = Directory::create(path, c, Facts::new(u64::MAX)).unwrap();
                let mut f = d.records().unwrap();
                f.append(b"x").is_ok()
            }));
        }
        assert_eq!(
            threads
                .into_iter()
                .map(|t| t.join().unwrap())
                .filter(|&ok| ok)
                .count(),
            1
        );
    });
    assert_eq!(c.used(), 4096);
    assert_eq!(unityrs_batch::work::rounded(u64::MAX, 6000), None);
}

#[test]
fn live_writers_block_owner_deletion_and_failed_writes_block_flushes() {
    let s = Scratch::new("live-writers");
    let facts = Facts::new(0);
    let c = Counter::new(4096, 1, 4096).unwrap();
    let mut d = Directory::create(s.join("attempt"), c.clone(), facts.clone()).unwrap();
    let mut f = d.output(0).unwrap();
    assert!(matches!(d.remove(), Err(Error::Internal)));
    assert!(matches!(d.rename(s.join("other")), Err(Error::Internal)));
    assert!(matches!(f.append(b"x"), Err(Error::Full(Full::Reserve))));
    facts.available.store(u64::MAX, Ordering::Relaxed);
    assert!(matches!(f.append(b"x"), Err(Error::Internal)));
    assert!(matches!(f.finish(), Err(Error::Internal)));
    d.remove().unwrap();
    assert_eq!(c.used(), 0);
}

#[test]
fn host_length_open_and_unlink_preserve_large_sparse_files() {
    let scratch = Scratch::new("spool-large-file");
    let path = scratch.join("large");
    let length = (1_u64 << 31) + 17;
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(length).unwrap();
    drop(file);
    assert_eq!(Host.length(&path).unwrap(), length);
    let mut reader = Host.open(&path).unwrap();
    let mut byte = [1];
    std::io::Read::read_exact(&mut reader, &mut byte).unwrap();
    assert_eq!(byte, [0]);
    drop(reader);
    Host.remove_file(&path).unwrap();
    assert!(!path.exists());
}
