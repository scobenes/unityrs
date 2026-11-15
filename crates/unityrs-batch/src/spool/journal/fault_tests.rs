//! Real-file mutation faults; adapters only interrupt individual IO calls.
use super::{
    Entry, Journal,
    mutation::{FileIo, Host, JournalIo},
};
use crate::spool::{Error, Full};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Barrier, Mutex},
};
use unityrs_testkit::scratch::Scratch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Open,
    Create,
    Write,
    DataSync,
    Rename,
    DirectorySync,
}
#[derive(Clone, Copy)]
struct Failure {
    op: Op,
    errno: i32,
    after: usize,
}
struct Control {
    failure: Option<Failure>,
    chunk: usize,
    interrupt: bool,
    bytes: usize,
    events: Vec<Op>,
    directory_durable: bool,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            failure: None,
            chunk: usize::MAX,
            interrupt: false,
            bytes: 0,
            events: Vec::new(),
            directory_durable: false,
        }
    }
}
impl Control {
    fn before(&mut self, op: Op) -> io::Result<()> {
        self.events.push(op);
        if let Some(failure) = self.failure
            && failure.op == op
            && (op != Op::Write || self.bytes >= failure.after)
        {
            return Err(io::Error::from_raw_os_error(failure.errno));
        }
        Ok(())
    }
}
#[derive(Clone, Default)]
struct FaultIo(Arc<Mutex<Control>>);
struct FaultFile {
    file: File,
    control: Arc<Mutex<Control>>,
}
impl Read for FaultFile {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}
impl Seek for FaultFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}
impl Write for FaultFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = {
            let mut control = self.control.lock().unwrap();
            control.before(Op::Write)?;
            if std::mem::take(&mut control.interrupt) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let mut count = bytes.len().min(control.chunk);
            if let Some(failure) = control.failure.filter(|failure| failure.op == Op::Write) {
                count = count.min(failure.after - control.bytes);
            }
            count
        };
        let written = self.file.write(&bytes[..count])?;
        self.control.lock().unwrap().bytes += written;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl FileIo for FaultFile {
    fn length(&self) -> io::Result<u64> {
        self.file.metadata().map(|metadata| metadata.len())
    }
    fn truncate(&self) -> io::Result<()> {
        self.file.set_len(0)
    }
    fn sync_data(&self) -> io::Result<()> {
        self.control.lock().unwrap().before(Op::DataSync)?;
        self.file.sync_data()
    }
}
impl JournalIo for FaultIo {
    type File = FaultFile;
    fn open(&self, path: &Path, create: bool) -> io::Result<FaultFile> {
        self.0
            .lock()
            .unwrap()
            .before(if create { Op::Create } else { Op::Open })?;
        let file = Host.open(path, create)?;
        if create {
            self.0.lock().unwrap().directory_durable = false;
        }
        Ok(FaultFile {
            file,
            control: self.0.clone(),
        })
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.0.lock().unwrap().before(Op::Rename)?;
        Host.rename(from, to)?;
        self.0.lock().unwrap().directory_durable = false;
        Ok(())
    }
    fn sync_directory(&self, path: &Path) -> io::Result<()> {
        self.0.lock().unwrap().before(Op::DirectorySync)?;
        Host.sync_directory(path)?;
        self.0.lock().unwrap().directory_durable = true;
        Ok(())
    }
}
fn entry(key: &str) -> Entry<'_> {
    Entry {
        endpoint: None,
        bucket: "bucket",
        key,
        upload_id: "id",
    }
}

#[test]
fn later_concurrent_append_syncs_a_name_left_by_failed_initial_append() {
    let scratch = Scratch::new("journal-failed-create");
    let journal = Journal::new(&scratch).unwrap();
    let io = FaultIo::default();
    io.0.lock().unwrap().failure = Some(Failure {
        op: Op::Write,
        errno: libc::ENOSPC,
        after: 5,
    });
    let created = Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            assert!(matches!(
                journal.append_using(entry("torn"), &io),
                Err(Error::Full(Full::Enospc))
            ));
            created.wait();
        });
        scope.spawn(|| {
            created.wait();
            io.0.lock().unwrap().failure = None;
            journal.append_using(entry("acknowledged"), &io).unwrap();
            assert!(
                io.0.lock().unwrap().directory_durable,
                "successful append must make an earlier failed creation durable"
            );
        });
    });
    let mut reader = journal.reader().unwrap();
    assert_eq!(
        reader.next().unwrap().unwrap().entry(),
        entry("acknowledged")
    );
    assert!(reader.next().unwrap().is_none());
}

fn assert_errno(error: Error, errno: i32) {
    match (error, errno) {
        (Error::Full(Full::Enospc), libc::ENOSPC) | (Error::Full(Full::Edquot), libc::EDQUOT) => {}
        (Error::HostIo(error), libc::EIO) => assert_eq!(error.raw_os_error(), Some(libc::EIO)),
        _ => panic!("journal mutation returned the wrong failure class for errno {errno}"),
    }
}

#[test]
fn append_propagates_write_data_sync_and_directory_sync_failures() {
    for op in [Op::Create, Op::Write, Op::DataSync, Op::DirectorySync] {
        for errno in [libc::ENOSPC, libc::EDQUOT, libc::EIO] {
            let scratch = Scratch::new("journal-append-fault");
            let journal = Journal::new(&scratch).unwrap();
            let io = FaultIo::default();
            io.0.lock().unwrap().failure = Some(Failure {
                op,
                errno,
                after: 5,
            });
            assert_errno(
                journal.append_using(entry("failed"), &io).unwrap_err(),
                errno,
            );
            assert_eq!(io.0.lock().unwrap().events.last(), Some(&op));
            io.0.lock().unwrap().failure = None;
            journal.append_using(entry("retry"), &io).unwrap();
            assert!(io.0.lock().unwrap().directory_durable);
            let mut reader = journal.reader().unwrap();
            let mut retry_found = false;
            while let Some(record) = reader.next().unwrap() {
                retry_found |= record.entry() == entry("retry");
            }
            assert!(retry_found);
        }
    }
}

#[test]
fn removal_failures_preserve_the_original_until_rename_and_allow_retry() {
    for op in [Op::Write, Op::DataSync, Op::Rename, Op::DirectorySync] {
        for errno in [libc::ENOSPC, libc::EDQUOT, libc::EIO] {
            let scratch = Scratch::new("journal-remove-fault");
            let journal = Journal::new(&scratch).unwrap();
            journal.append(entry("remove")).unwrap();
            journal.append(entry("keep")).unwrap();
            let original = std::fs::read(scratch.join("journal")).unwrap();
            let io = FaultIo::default();
            io.0.lock().unwrap().failure = Some(Failure {
                op,
                errno,
                after: 5,
            });
            assert_errno(
                journal.remove_using(entry("remove"), &io).unwrap_err(),
                errno,
            );
            assert_eq!(io.0.lock().unwrap().events.last(), Some(&op));
            if op == Op::DirectorySync {
                assert!(!io.0.lock().unwrap().directory_durable);
            } else {
                assert_eq!(std::fs::read(scratch.join("journal")).unwrap(), original);
            }
            io.0.lock().unwrap().failure = None;
            journal.remove_using(entry("remove"), &io).unwrap();
            assert!(io.0.lock().unwrap().directory_durable);
            let mut reader = journal.reader().unwrap();
            assert_eq!(reader.next().unwrap().unwrap().entry(), entry("keep"));
            assert!(reader.next().unwrap().is_none());
            assert!(!scratch.join("journal.tmp").exists());
        }
    }
}

#[test]
fn append_retries_interrupted_and_short_writes_before_both_syncs() {
    let scratch = Scratch::new("journal-append-short");
    let journal = Journal::new(&scratch).unwrap();
    // The directory must be synced even when this append opens an existing file.
    journal.append(entry("before")).unwrap();
    let io = FaultIo::default();
    {
        let mut control = io.0.lock().unwrap();
        control.chunk = 3;
        control.interrupt = true;
    }
    journal.append_using(entry("short"), &io).unwrap();
    assert_eq!(
        std::fs::read(scratch.join("journal")).unwrap(),
        b"[null,\"bucket\",\"before\",\"id\"]\n[null,\"bucket\",\"short\",\"id\"]\n"
    );
    let control = io.0.lock().unwrap();
    assert!(control.events.iter().filter(|&&op| op == Op::Write).count() > 3);
    assert_eq!(
        control
            .events
            .iter()
            .copied()
            .filter(|op| matches!(op, Op::DataSync | Op::DirectorySync))
            .collect::<Vec<_>>(),
        [Op::DataSync, Op::DirectorySync]
    );
}

#[test]
fn removal_retries_interrupted_short_stream_writes_before_ordered_commit() {
    let scratch = Scratch::new("journal-remove-short");
    let journal = Journal::new(&scratch).unwrap();
    journal.append(entry("remove")).unwrap();
    journal.append(entry("keep")).unwrap();
    let mut malformed = vec![b'x'; super::LINE_LIMIT + 1];
    malformed.push(b'\n');
    let mut original = malformed.clone();
    original.extend_from_slice(&std::fs::read(scratch.join("journal")).unwrap());
    std::fs::write(scratch.join("journal"), original).unwrap();
    let io = FaultIo::default();
    {
        let mut control = io.0.lock().unwrap();
        control.chunk = 37;
        control.interrupt = true;
    }
    journal.remove_using(entry("remove"), &io).unwrap();
    malformed.extend_from_slice(b"[null,\"bucket\",\"keep\",\"id\"]\n");
    assert_eq!(std::fs::read(scratch.join("journal")).unwrap(), malformed);
    assert_eq!(
        io.0.lock()
            .unwrap()
            .events
            .iter()
            .copied()
            .filter(|op| matches!(op, Op::DataSync | Op::Rename | Op::DirectorySync))
            .collect::<Vec<_>>(),
        [Op::DataSync, Op::Rename, Op::DirectorySync]
    );
}
