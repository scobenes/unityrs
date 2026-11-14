//! Original-descriptor faults and a bounded held-write barrier.
use super::Time;
use std::{
    fs::File,
    io::{self, Write},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Waker,
};
use unityrs_batch::spool::{Counter, FileSystem, Host, Writer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub operation: &'static str,
    pub claim: u64,
    pub length: u64,
}
pub struct Disk {
    pub counter: Arc<Counter>,
    pub time: Arc<Time>,
    pub mode: AtomicUsize,
    pub available_calls: AtomicUsize,
    pub stop_on_flush: Mutex<Option<Arc<super::Stop>>>,
    pub events: Mutex<Vec<Event>>,
    pub held: AtomicBool,
    pub entered: AtomicBool,
    pub waiter: Mutex<Option<Waker>>,
    gate: (Mutex<bool>, Condvar),
    writes: AtomicUsize,
}
impl Disk {
    pub fn new(counter: Arc<Counter>, time: Arc<Time>) -> Arc<Self> {
        Arc::new(Self {
            counter,
            time,
            mode: AtomicUsize::new(0),
            available_calls: AtomicUsize::new(0),
            stop_on_flush: Mutex::new(None),
            events: Mutex::new(vec![]),
            held: AtomicBool::new(false),
            entered: AtomicBool::new(false),
            waiter: Mutex::new(None),
            gate: (Mutex::new(false), Condvar::new()),
            writes: AtomicUsize::new(0),
        })
    }
    pub fn release(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
    fn record(&self, operation: &'static str, file: &File) {
        self.events.lock().unwrap().push(Event {
            operation,
            claim: self.counter.used(),
            length: file.metadata().unwrap().len(),
        });
    }
}
pub struct Fs(pub Arc<Disk>);
impl std::ops::Deref for Fs {
    type Target = Disk;
    fn deref(&self) -> &Disk {
        &self.0
    }
}
impl FileSystem for Fs {
    fn available(&self, _: &Path) -> io::Result<u64> {
        self.available_calls.fetch_add(1, Ordering::AcqRel);
        if self.mode.load(Ordering::Acquire) == 10 {
            self.time.at(1000);
        }
        if self.mode.load(Ordering::Acquire) == 7 {
            Err(io::Error::from_raw_os_error(libc::EIO))
        } else {
            Ok(u64::MAX)
        }
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn Writer>> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        Ok(Box::new(Output {
            file,
            disk: self.0.clone(),
        }))
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        if self.mode.load(Ordering::Acquire) == 6 {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Host.remove_file(path)
    }
}
struct Output {
    file: File,
    disk: Arc<Disk>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()?;
        if let Some(stop) = self.disk.stop_on_flush.lock().unwrap().as_ref() {
            stop.stopped.store(true, Ordering::Release);
        }
        Ok(())
    }
}
impl Writer for Output {
    fn seal(&self) -> io::Result<()> {
        self.disk.record("seal", &self.file);
        if self.disk.mode.load(Ordering::Acquire) == 5 {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        self.file.seal()
    }
    fn write_at(&mut self, bytes: &[u8], offset: u64) -> io::Result<usize> {
        self.disk.record("write", &self.file);
        self.disk.entered.store(true, Ordering::Release);
        if let Some(waker) = self.disk.waiter.lock().unwrap().take() {
            waker.wake();
        }
        if self.disk.held.load(Ordering::Acquire) {
            let (released, _) = self
                .disk
                .gate
                .1
                .wait_timeout_while(
                    self.disk.gate.0.lock().unwrap(),
                    unityrs_testkit::guard::DEADLOCK_GUARD,
                    |released| !*released,
                )
                .unwrap();
            assert!(*released, "held write guard");
        }
        let mode = self.disk.mode.load(Ordering::Acquire);
        let ordinal = self.disk.writes.fetch_add(1, Ordering::AcqRel);
        if mode == 1 && ordinal == 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if mode == 4 {
            self.disk.time.complete_write_at(5000);
        }
        assert!(mode != 8, "synthetic write panic");
        if mode == 9 {
            return Ok(0);
        }
        let count = if mode == 1 || mode == 2 {
            bytes.len().min(2)
        } else {
            bytes.len()
        };
        let n = std::os::unix::fs::FileExt::write_at(&self.file, &bytes[..count], offset)?;
        if mode == 2 {
            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
        }
        Ok(n)
    }
    fn truncate(&mut self) -> io::Result<()> {
        self.disk.record("truncate", &self.file);
        self.file.set_len(0)?;
        if self.disk.mode.load(Ordering::Acquire) == 3 {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    }
}
