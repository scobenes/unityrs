use std::{
    cell::Cell,
    io::{self, Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    attempt::ReasonCode,
    phase::{Account, Error},
    sort::{self, Frame, Hooks, Options, Order, Point, Source},
    spool::{self, Counter, FileSystem, Host, Writer, workspace::Workspace},
};
use unityrs_testkit::scratch::Scratch;

struct Bytes;
impl Order for Bytes {
    fn tag(&self) -> u32 {
        1
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        a.cmp(b)
    }
}
struct Data(u8);
impl Source for Data {
    fn next(&mut self, frame: &mut Frame, _: &dyn Fn() -> bool) -> Result<bool, Error> {
        if self.0 == 0 {
            return Ok(false);
        }
        self.0 -= 1;
        frame.set(&[self.0])?;
        Ok(true)
    }
}
#[derive(Clone, Default)]
struct Faults {
    operation: Arc<AtomicUsize>,
    errno: Arc<AtomicI32>,
    writes: Arc<AtomicUsize>,
    unlinks: Arc<AtomicUsize>,
}
impl Faults {
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
        self.check(1)?;
        Ok(u64::MAX)
    }
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        self.check(2)?;
        Host.mkdir(path)
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn Writer>> {
        self.check(3)?;
        Ok(Box::new(Output {
            inner: Host.create(path)?,
            faults: self.clone(),
        }))
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        self.check(7)?;
        if self.operation.load(Ordering::Relaxed) == 12 {
            // Truncate the stream while preserving descriptor metadata.
            return Ok(Box::new(Host.open(path)?.take(17)));
        }
        Ok(Box::new(Input {
            inner: Host.open(path)?,
            interrupted: false,
            faults: self.clone(),
        }))
    }
    fn length(&self, path: &Path) -> io::Result<u64> {
        self.check(8)?;
        Host.length(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.check(9)?;
        Host.remove_file(path)?;
        self.unlinks.fetch_add(1, Ordering::Relaxed);
        self.check(10) // Failure after a real successful unlink retains claims.
    }
}
struct Output {
    inner: Box<dyn Writer>,
    faults: Faults,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.faults.writes.fetch_add(1, Ordering::Relaxed);
        match self.faults.operation.load(Ordering::Relaxed) {
            13 => {
                self.inner
                    .write_all(bytes.get(..bytes.len().min(3)).unwrap())?;
                panic!("write adapter unwind");
            }
            4 => {
                self.inner
                    .write_all(bytes.get(..bytes.len().min(3)).unwrap())?;
                self.faults.check(4)?;
            }
            11 => return self.inner.write(bytes.get(..bytes.len().min(3)).unwrap()),
            _ => {}
        }
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.faults.check(5)?;
        self.inner.flush()
    }
}
// The seal is fault 6, as it was when the filesystem sealed by path.
impl Writer for Output {
    fn seal(&self) -> io::Result<()> {
        self.faults.check(6)?;
        self.inner.seal()
    }
}
struct Input {
    inner: Box<dyn Read + Send>,
    interrupted: bool,
    faults: Faults,
}
impl Read for Input {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.faults.operation.load(Ordering::Relaxed) == 11 && !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.inner.read(out.get_mut(..out.len().min(7)).unwrap())
    }
}
struct Arm<'a> {
    point: Point,
    faults: &'a Faults,
    operation: usize,
}
impl Hooks for Arm<'_> {
    fn point(&self, point: Point) {
        if point == self.point {
            self.faults
                .operation
                .store(self.operation, Ordering::Relaxed);
        }
    }
}
#[test]
fn spool_faults_preserve_classification_and_claims_at_each_sort_stage() {
    let scratch = Scratch::new("sort-faults");
    for point in [Point::Entry, Point::BeforeSort, Point::MergeGroup] {
        for operation in 1..=10 {
            if operation == 2 && point != Point::Entry {
                continue;
            }
            for errno in [libc::EIO, libc::ENOSPC, libc::EDQUOT] {
                let faults = Faults::default();
                faults.errno.store(errno, Ordering::Relaxed);
                let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
                let mut workspace = Workspace::create(
                    scratch.join(format!("{point:?}-{operation}-{errno}")),
                    counter.clone(),
                    Arc::new(faults.clone()),
                )
                .unwrap();
                if operation == 2 {
                    faults.operation.store(2, Ordering::Relaxed);
                    assert!(
                        Workspace::create(
                            scratch.join(format!("mkdir-{errno}")),
                            counter,
                            Arc::new(faults)
                        )
                        .is_err()
                    );
                    continue;
                }
                let account = Account::new(64 << 20);
                let error = sort::sort(
                    &mut Data(4),
                    &Bytes,
                    &mut workspace,
                    &account,
                    Options {
                        run_records: 1,
                        ..Options::default()
                    },
                    &|| false,
                    &Arm {
                        point,
                        faults: &faults,
                        operation,
                    },
                )
                .unwrap_err();
                let Error::Run(failure) = error else {
                    panic!("{error:?}")
                };
                // Read/metadata failures are host_io, even when errno is a
                // capacity code; actual spool mutations preserve full detail.
                let expected = if matches!(operation, 7 | 8) || errno == libc::EIO {
                    ReasonCode::HostIo
                } else {
                    ReasonCode::SpoolFull
                };
                assert_eq!(failure.reason, expected, "{point:?} {operation} {errno}");
                if failure.reason == ReasonCode::SpoolFull {
                    assert_eq!(
                        failure.detail.unwrap().name(),
                        if errno == libc::ENOSPC {
                            "ENOSPC"
                        } else {
                            "EDQUOT"
                        }
                    );
                }
                assert_eq!(workspace.claimed(), counter.used());
                assert_eq!(account.snapshot().input.used, 0);
                if operation == 10 {
                    assert!(faults.unlinks.load(Ordering::Relaxed) > 0);
                    assert!(counter.used() > 0);
                }
                let held = counter.used();
                drop(workspace);
                assert_eq!(counter.used(), held);
            }
        }
    }
}

#[test]
fn short_writes_and_interrupted_reads_complete_but_truncation_is_internal() {
    let scratch = Scratch::new("sort-short");
    for operation in [11, 12] {
        let faults = Faults::default();
        faults.operation.store(operation, Ordering::Relaxed);
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let mut workspace = Workspace::create(
            scratch.join(operation.to_string()),
            counter.clone(),
            Arc::new(faults),
        )
        .unwrap();
        let result = sort::sort(
            &mut Data(4),
            &Bytes,
            &mut workspace,
            &Account::new(64 << 20),
            Options {
                run_records: 1,
                ..Options::default()
            },
            &|| false,
            &sort::NoHooks,
        );
        if operation == 11 {
            workspace
                .retire(result.unwrap().file.unwrap(), &|| false)
                .unwrap();
            assert_eq!(counter.used(), 0);
        } else {
            assert_eq!(result.unwrap_err(), Error::internal());
        }
    }
}

struct Stop {
    target: Point,
    occurrence: usize,
    seen: Cell<usize>,
    stopped: Cell<bool>,
}
impl Hooks for Stop {
    fn point(&self, point: Point) {
        if point == self.target {
            self.seen.set(self.seen.get() + 1);
            if self.seen.get() == self.occurrence {
                self.stopped.set(true);
            }
        }
    }
}
#[test]
fn stop_at_every_boundary_retains_claims_and_never_flushes_on_drop() {
    let scratch = Scratch::new("sort-stops");
    for point in [
        Point::Entry,
        Point::Pass,
        Point::BeforeRead,
        Point::BeforeSort,
        Point::AfterSort,
        Point::RunSealed,
        Point::MergeGroup,
        Point::BeforeWrite,
        Point::BeforeRetire,
        Point::BeforeReturn,
    ] {
        for occurrence in [1, 2] {
            if matches!(point, Point::Entry | Point::BeforeReturn) && occurrence == 2 {
                continue;
            }
            let stop = Stop {
                target: point,
                occurrence,
                seen: Cell::new(0),
                stopped: Cell::new(false),
            };
            let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
            let faults = Faults::default();
            let mut workspace = Workspace::create(
                scratch.join(format!("{point:?}-{occurrence}")),
                counter.clone(),
                Arc::new(faults.clone()),
            )
            .unwrap();
            let account = Account::new(64 << 20);
            let result = sort::sort(
                &mut Data(17),
                &Bytes,
                &mut workspace,
                &account,
                Options {
                    run_records: 1,
                    fan_in: 2,
                    ..Options::default()
                },
                &|| stop.stopped.get(),
                &stop,
            );
            assert_eq!(
                result.unwrap_err(),
                Error::Stopped,
                "{point:?} {occurrence}"
            );
            let held = counter.used();
            let writes = faults.writes.load(Ordering::Relaxed);
            drop(workspace);
            assert_eq!(counter.used(), held);
            assert_eq!(faults.writes.load(Ordering::Relaxed), writes);
            assert_eq!(account.snapshot().input.used, 0);
        }
    }
}

#[test]
fn handover_between_space_check_and_claim_permits_only_one_claim_and_no_write() {
    struct Handover {
        stopped: Arc<AtomicBool>,
    }
    impl FileSystem for Handover {
        fn available(&self, _: &Path) -> io::Result<u64> {
            self.stopped.store(true, Ordering::Release);
            Ok(100)
        }
    }
    let scratch = Scratch::new("sort-handover");
    let stopped = Arc::new(AtomicBool::new(false));
    let counter = Counter::new(8192, 100, 4096).unwrap();
    let mut workspace = Workspace::create(
        scratch.join("w"),
        counter.clone(),
        Arc::new(Handover {
            stopped: stopped.clone(),
        }),
    )
    .unwrap();
    let mut file = workspace.create_file(&|| false).unwrap();
    assert!(matches!(
        file.append(b"buffer", &|| stopped.load(Ordering::Acquire)),
        Err(spool::Error::Stopped)
    ));
    assert_eq!(counter.used(), 4096);
    assert!(matches!(
        file.append(b"again", &|| false),
        Err(spool::Error::Internal)
    ));
    assert_eq!(std::fs::metadata(scratch.join("w/f0")).unwrap().len(), 0);
    drop(file);
    assert_eq!(counter.used(), 4096);
}

#[test]
fn an_unwinding_write_still_belongs_to_the_workspace_aggregate() {
    let scratch = Scratch::new("sort-unwind-claim");
    let faults = Faults::default();
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let mut workspace =
        Workspace::create(scratch.join("w"), counter.clone(), Arc::new(faults.clone())).unwrap();
    let account = Account::new(64 << 20);
    let result = sort::sort(
        &mut Data(4),
        &Bytes,
        &mut workspace,
        &account,
        Options::default(),
        &|| false,
        &Arm {
            point: Point::BeforeSort,
            faults: &faults,
            operation: 13,
        },
    );
    assert_eq!(result.unwrap_err(), Error::internal());
    assert!(counter.used() > 4096);
    assert_eq!(workspace.claimed(), counter.used());
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn stops_during_merge_reads_writes_and_later_generation_boundaries() {
    struct LateStop {
        target: Point,
        passes: Cell<usize>,
        merging: Cell<bool>,
        stopped: Cell<bool>,
    }
    impl Hooks for LateStop {
        fn point(&self, point: Point) {
            if point == Point::Pass {
                self.passes.set(self.passes.get() + 1);
            }
            if point == Point::MergeGroup {
                self.merging.set(true);
            }
            if point == self.target && self.merging.get() && self.passes.get() >= 3 {
                self.stopped.set(true);
            }
        }
    }
    let scratch = Scratch::new("sort-late-stops");
    for target in [Point::BeforeRead, Point::BeforeWrite, Point::Pass] {
        let stop = LateStop {
            target,
            passes: Cell::new(0),
            merging: Cell::new(false),
            stopped: Cell::new(false),
        };
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let faults = Faults::default();
        let mut workspace = Workspace::create(
            scratch.join(format!("{target:?}")),
            counter.clone(),
            Arc::new(faults.clone()),
        )
        .unwrap();
        let account = Account::new(64 << 20);
        let result = sort::sort(
            &mut Data(17),
            &Bytes,
            &mut workspace,
            &account,
            Options {
                run_records: 1,
                fan_in: 2,
                ..Options::default()
            },
            &|| stop.stopped.get(),
            &stop,
        );
        assert_eq!(result.unwrap_err(), Error::Stopped);
        assert!(stop.passes.get() >= 3);
        let held = counter.used();
        let writes = faults.writes.load(Ordering::Relaxed);
        drop(workspace);
        assert_eq!(counter.used(), held);
        assert_eq!(faults.writes.load(Ordering::Relaxed), writes);
        assert_eq!(account.snapshot().input.used, 0);
    }
}
