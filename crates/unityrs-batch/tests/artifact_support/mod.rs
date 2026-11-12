#![allow(dead_code)]
use std::io::{self, Write};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicUsize, Ordering},
};
use unityrs_batch::{
    budget::SealedBudget,
    records::{Figures, Status, Summary},
    spool::{FileSystem, Host, Writer},
};
use unityrs_core::{budget::Reservation, limits::Meter};

pub fn meter() -> (Arc<SealedBudget>, Arc<Meter>) {
    let b = Arc::new(SealedBudget::new(1024 * 1024, 1024));
    let m = Meter::new(1024 * 1024, b.clone(), Reservation::Input);
    (b, m)
}
pub fn summary(status: Status) -> Summary<'static> {
    Summary {
        input: "input",
        status,
        reason: None,
        detail: None,
        figures: Some(Figures::default()),
        spooled_part: 0,
        claimed_bytes: 0,
        cut: false,
    }
}
/// Operation 1=create, 2=write, 3=flush, 4=delete directory, 5=seal, 6=delete file. Wrapped writes use a
/// real file, preserving all side effects except the injected failure.
#[derive(Clone, Default)]
pub struct Faults {
    pub operation: Arc<AtomicUsize>,
    pub errno: Arc<AtomicI32>,
    pub writes: Arc<AtomicUsize>,
}
impl Faults {
    pub fn set(&self, operation: usize, errno: i32) {
        self.operation.store(operation, Ordering::Relaxed);
        self.errno.store(errno, Ordering::Relaxed);
    }
    fn check(&self, operation: usize) -> io::Result<()> {
        if self.operation.load(Ordering::Relaxed) == operation {
            Err(io::Error::from_raw_os_error(
                self.errno.load(Ordering::Relaxed),
            ))
        } else {
            Ok(())
        }
    }
}
impl FileSystem for Faults {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn Writer>> {
        self.check(1)?;
        Ok(Box::new(FaultFile {
            file: Host.create(path)?,
            faults: self.clone(),
        }))
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.check(4)?;
        Host.remove(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.check(6)?;
        Host.remove_file(path)
    }
}
struct FaultFile {
    file: Box<dyn Writer>,
    faults: Faults,
}
impl Write for FaultFile {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.faults.writes.fetch_add(1, Ordering::Relaxed);
        if self.faults.operation.load(Ordering::Relaxed) == 2 {
            // A partial write is possible before the filesystem refuses.
            self.file.write_all(b.get(..b.len().min(7)).unwrap())?;
            self.faults.check(2)?;
        }
        self.file.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.faults.check(3)?;
        self.file.flush()
    }
}

impl Writer for FaultFile {
    fn seal(&self) -> io::Result<()> {
        self.faults.check(5)?;
        self.file.seal()
    }
}
