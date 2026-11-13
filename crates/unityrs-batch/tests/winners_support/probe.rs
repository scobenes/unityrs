#![allow(dead_code)]
use std::{
    io::{self, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    spool::{Counter, FileSystem, Host, Writer},
    winners::{Observer, Pass},
    work::rounded,
};

pub struct Probe {
    pub counter: Arc<Counter>,
    pub baseline: u64,
    pub block: u64,
    pub current: AtomicUsize,
    pub max_runs: AtomicU64,
    pub peaks: [AtomicU64; 14],
    pub bounds: [AtomicU64; 14],
    pub operation: AtomicUsize,
    pub errno: AtomicI32,
    pub fail_pass: AtomicUsize,
}
impl Probe {
    pub fn new(counter: Arc<Counter>, baseline: u64, block: u64) -> Arc<Self> {
        Arc::new(Self {
            counter,
            baseline,
            block,
            current: AtomicUsize::new(0),
            max_runs: AtomicU64::new(0),
            peaks: std::array::from_fn(|_| AtomicU64::new(0)),
            bounds: std::array::from_fn(|_| AtomicU64::new(0)),
            operation: AtomicUsize::new(0),
            errno: AtomicI32::new(libc::EIO),
            fail_pass: AtomicUsize::new(usize::MAX),
        })
    }
    pub fn sample(&self) {
        self.peaks[self.current.load(Ordering::Relaxed)]
            .fetch_max(self.counter.used() - self.baseline, Ordering::Relaxed);
    }
    fn check(&self, op: usize) -> io::Result<()> {
        self.sample();
        if self.operation.load(Ordering::Relaxed) == op
            && self.fail_pass.load(Ordering::Relaxed) == self.current.load(Ordering::Relaxed)
        {
            Err(io::Error::from_raw_os_error(
                self.errno.load(Ordering::Relaxed),
            ))
        } else {
            Ok(())
        }
    }
}
impl Observer for Probe {
    fn pass(&self, pass: Pass, _claimed: u64) {
        self.sample();
        self.current.store(pass as usize, Ordering::Relaxed);
        self.sample();
    }
    fn sort(&self, retained: u64, encoded: u64, runs: u64) {
        self.max_runs.fetch_max(runs, Ordering::Relaxed);
        let bound = if runs == 0 {
            retained + 2 * rounded(16, self.block).unwrap()
        } else {
            let p = encoded + runs * (16 + self.block - 1);
            let k = rounded(16 + 32 * runs, self.block).unwrap();
            retained + 2 * p + 2 * k
        };
        self.bounds[self.current.load(Ordering::Relaxed)].fetch_max(bound, Ordering::Relaxed);
        self.sample();
    }
}
#[derive(Clone)]
pub struct Fs(pub Arc<Probe>);
impl FileSystem for Fs {
    fn available(&self, _: &Path) -> io::Result<u64> {
        self.0.sample();
        Ok(u64::MAX)
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn Writer>> {
        self.0.check(1)?;
        Ok(Box::new(File {
            file: Host.create(path)?,
            probe: self.0.clone(),
        }))
    }
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        Host.mkdir(path)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.0.check(4)?;
        Host.remove(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.0.check(6)?;
        Host.remove_file(path)
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn io::Read + Send>> {
        self.0.check(7)?;
        Host.open(path)
    }
    fn length(&self, path: &Path) -> io::Result<u64> {
        self.0.check(8)?;
        Host.length(path)
    }
}
struct File {
    file: Box<dyn Writer>,
    probe: Arc<Probe>,
}
impl Write for File {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.probe.check(2)?;
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.probe.check(3)?;
        self.file.flush()
    }
}
impl Writer for File {
    fn seal(&self) -> io::Result<()> {
        self.probe.check(5)?;
        self.file.seal()
    }
}
