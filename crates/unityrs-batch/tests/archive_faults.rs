mod archive_support;
mod artifact_support;
use archive_support::Scene;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use unityrs_batch::{
    archive::{self, Hooks, Point},
    attempt::ReasonCode,
    phase::{Account, Buffer, Error},
    spool::Full,
};

struct StopAt {
    point: Point,
    stopped: AtomicBool,
}
impl Hooks for StopAt {
    fn at(&self, point: Point) -> Result<(), Error> {
        if point == self.point {
            self.stopped.store(true, Ordering::Release);
        }
        Ok(())
    }
}
fn consume(
    scene: &Scene,
    account: &Account,
    hooks: &dyn Hooks,
    stopped: &dyn Fn() -> bool,
) -> Result<archive::Summary, Error> {
    let mut cx = scene.context(account);
    cx.hooks = hooks;
    let mut stream = archive::open(cx, scene.input(), stopped)?;
    let mut buffer = Buffer::new(account.meter(), 65_536)?;
    buffer.fill(0);
    while stream.read(&mut buffer, stopped)? > 0 {}
    stream.finish(stopped)
}
#[test]
fn stops_at_every_stream_boundary_yield_no_summary_and_no_drop_cleanup() {
    for point in [
        Point::Member,
        Point::PayloadRead,
        Point::CentralWrite,
        Point::CentralSeal,
        Point::CentralRead,
        Point::Retire,
        Point::Complete,
    ] {
        let scene = Scene::new(&[("a", b"x")]);
        let account = Account::new(1 << 20);
        let hooks = StopAt {
            point,
            stopped: AtomicBool::new(false),
        };
        let baseline = scene.counter.used();
        assert_eq!(
            consume(&scene, &account, &hooks, &|| hooks
                .stopped
                .load(Ordering::Acquire)),
            Err(Error::Stopped)
        );
        let retained = scene.counter.used() - baseline;
        if matches!(
            point,
            Point::CentralSeal | Point::CentralRead | Point::Retire
        ) {
            assert_eq!(retained, 4096);
        } else {
            assert_eq!(retained, 0);
        }
        assert_eq!(scene.workspace.claimed(), scene.counter.used());
        assert_eq!(account.snapshot().input.used, 0);
    }
}
#[test]
fn write_flush_seal_and_delete_failures_keep_claims() {
    for operation in [2, 3, 5, 6] {
        for errno in [libc::EIO, libc::ENOSPC, libc::EDQUOT] {
            let faults = artifact_support::Faults::default();
            let scene = Scene::with_fs(&[("a", b"x")], 4096, Arc::new(faults.clone()));
            let baseline = scene.counter.used();
            faults.set(operation, errno);
            let account = Account::new(1 << 20);
            let result = consume(&scene, &account, &archive::NoHooks, &|| false);
            let expected = match errno {
                libc::ENOSPC => Error::Run(unityrs_batch::job::RunFailure::from(
                    unityrs_batch::spool::Error::Full(Full::Enospc),
                )),
                libc::EDQUOT => Error::Run(unityrs_batch::job::RunFailure::from(
                    unityrs_batch::spool::Error::Full(Full::Edquot),
                )),
                _ => Error::Run(unityrs_batch::job::RunFailure::new(ReasonCode::HostIo)),
            };
            assert_eq!(result, Err(expected), "operation={operation} errno={errno}");
            assert_eq!(scene.counter.used() - baseline, 4096);
            assert_eq!(scene.workspace.claimed(), scene.counter.used());
            assert_eq!(account.snapshot().input.used, 0);
        }
    }
}
#[test]
fn drop_after_replay_and_after_eof_retains_claim_until_explicit_deletion() {
    for exhaust in [false, true] {
        let scene = Scene::new(&[("a", b"x")]);
        let account = Account::new(1 << 20);
        let baseline = scene.counter.used();
        let mut stream = archive::open(scene.context(&account), scene.input(), &|| false).unwrap();
        let mut buffer = vec![0; 65_536];
        loop {
            let n = stream.read(&mut buffer, &|| false).unwrap();
            if n == 0 || (!exhaust && scene.counter.used() > baseline) {
                break;
            }
        }
        drop(stream);
        assert_eq!(scene.counter.used(), baseline + 4096);
        assert_eq!(account.snapshot().input.used, 0);
    }
}

struct ClaimStop {
    enabled: AtomicBool,
    stopped: AtomicBool,
}
impl unityrs_batch::spool::FileSystem for ClaimStop {
    fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
        if self.enabled.load(Ordering::Acquire) {
            self.stopped.store(true, Ordering::Release);
        }
        Ok(u64::MAX)
    }
}
#[test]
fn stop_between_claim_and_write_keeps_the_unwritten_claim() {
    let fs = Arc::new(ClaimStop {
        enabled: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
    });
    let scene = Scene::with_fs(&[("a", b"x")], 4096, fs.clone());
    let baseline = scene.counter.used();
    let account = Account::new(1 << 20);
    fs.enabled.store(true, Ordering::Release);
    assert_eq!(
        consume(&scene, &account, &archive::NoHooks, &|| fs
            .stopped
            .load(Ordering::Acquire)),
        Err(Error::Stopped)
    );
    assert_eq!(scene.counter.used(), baseline + 4096);
    assert_eq!(scene.workspace.claimed(), baseline + 4096);
    assert_eq!(account.snapshot().input.used, 0);
}
struct ReadFaults {
    mode: Arc<std::sync::atomic::AtomicI32>,
}
impl unityrs_batch::spool::FileSystem for ReadFaults {
    fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
    fn open(&self, path: &std::path::Path) -> std::io::Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(ReadFault {
            inner: unityrs_batch::spool::Host.open(path)?,
            mode: self.mode.clone(),
        }))
    }
}
struct ReadFault {
    inner: Box<dyn std::io::Read + Send>,
    mode: Arc<std::sync::atomic::AtomicI32>,
}
impl std::io::Read for ReadFault {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let mode = self.mode.load(Ordering::Acquire);
        if mode == -1 {
            return Ok(0);
        }
        if mode > 0 {
            if mode == libc::EINTR {
                self.mode.store(0, Ordering::Release);
            }
            return Err(std::io::Error::from_raw_os_error(mode));
        }
        let n = out.len().min(1);
        self.inner.read(&mut out[..n])
    }
}
struct AtRead {
    mode: Arc<std::sync::atomic::AtomicI32>,
    errno: i32,
}
impl Hooks for AtRead {
    fn at(&self, point: Point) -> Result<(), Error> {
        if point == Point::CentralRead {
            self.mode.store(self.errno, Ordering::Release);
        }
        Ok(())
    }
}
#[test]
fn central_reads_distinguish_eof_from_io_and_retry_interrupted_short_reads() {
    for errno in [-1, libc::EIO, libc::ENOSPC, libc::EDQUOT, libc::EINTR] {
        let mode = Arc::new(std::sync::atomic::AtomicI32::new(0));
        let fs = Arc::new(ReadFaults { mode: mode.clone() });
        let scene = Scene::with_fs(&[("a", b"x")], 4096, fs);
        let baseline = scene.counter.used();
        let account = Account::new(1 << 20);
        let result = consume(&scene, &account, &AtRead { mode, errno }, &|| false);
        if errno == libc::EINTR {
            result.unwrap();
            assert_eq!(scene.counter.used(), baseline);
        } else {
            assert_eq!(
                result,
                Err(if errno == -1 {
                    Error::internal()
                } else {
                    Error::Run(unityrs_batch::job::RunFailure::new(ReasonCode::HostIo))
                })
            );
            assert_eq!(scene.counter.used(), baseline + 4096);
        }
        assert_eq!(account.snapshot().input.used, 0);
    }
}

struct PayloadReads<'a> {
    files: &'a archive_support::Files,
    mode: Arc<std::sync::atomic::AtomicI32>,
}
impl archive::Payloads for PayloadReads<'_> {
    fn open(&self, source: u32, ordinal: u64) -> Result<Box<dyn std::io::Read + Send>, Error> {
        Ok(Box::new(ReadFault {
            inner: archive::Payloads::open(self.files, source, ordinal)?,
            mode: self.mode.clone(),
        }))
    }
}
#[test]
fn payload_read_errors_are_run_failures_and_interrupted_reads_resume() {
    for errno in [libc::EIO, libc::ENOSPC, libc::EDQUOT, libc::EINTR] {
        let scene = Scene::new(&[("a", b"xyz")]);
        let account = Account::new(1 << 20);
        let expected = scene.generate(7).unwrap().1;
        let source = PayloadReads {
            files: &scene.files,
            mode: Arc::new(std::sync::atomic::AtomicI32::new(errno)),
        };
        let mut cx = scene.context(&account);
        cx.payloads = &source;
        let result = account.execute(|| {
            let mut stream = archive::open(cx, scene.input(), &|| false)?;
            let mut buffer = Buffer::new(account.meter(), 65_536)?;
            buffer.fill(0);
            while stream.read(&mut buffer, &|| false)? > 0 {}
            stream.finish(&|| false)
        });
        if errno == libc::EINTR {
            assert_eq!(result.unwrap(), expected);
        } else {
            assert_eq!(
                result,
                Err(Error::Run(unityrs_batch::job::RunFailure::new(
                    ReasonCode::HostIo
                )))
            );
        }
        assert_eq!(account.snapshot().input.used, 0);
    }
}
