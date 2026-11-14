//! Controller-side open/finalisation around a movable bulk copy/hash job.
//! The destination owner stays outside the job and outlives every failure.
use super::{DataFailure, Facts, Failure, StageEnd};
use crate::{attempt::ReasonCode, job::RunFailure, spec::model::BundleRef, spool::OwnedFile};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
};

pub const BUFFER: usize = 64 * 1024;

/// All metadata comparisons preserve the filesystem's timestamp precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub regular: bool,
    pub size: u64,
    pub device: u64,
    pub inode: u64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
}
impl Snapshot {
    #[must_use]
    pub fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            regular: meta.is_file(),
            size: meta.len(),
            device: meta.dev(),
            inode: meta.ino(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}
/// A bounded source seam for deterministic short-read/read-error tests.
pub trait Source: Read + Send {
    fn snapshot(&self) -> io::Result<Snapshot>;
    fn blocking(&self) -> io::Result<()> {
        Ok(())
    }
}
impl Source for File {
    fn snapshot(&self) -> io::Result<Snapshot> {
        self.metadata().map(|m| Snapshot::of(&m))
    }
    fn blocking(&self) -> io::Result<()> {
        crate::sys::blocking_source(self)
    }
}
pub trait Sources {
    fn open(&self, path: &Path) -> io::Result<Box<dyn Source>>;
}
pub struct Host;
impl Sources for Host {
    fn open(&self, path: &Path) -> io::Result<Box<dyn Source>> {
        let file = crate::sys::local_source(path)?;
        Ok(crate::alloc::try_box(file)?)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Buffer(u64),
    BeforeRestat,
    BeforeSeal,
}
pub trait Hooks {
    fn point(&self, _point: Point) {}
    fn request_stop(&self, _end: &StageEnd) {}
}
pub struct NoHooks;
impl Hooks for NoHooks {}

/// No paths, whole-body buffer or spec strings are copied into this job.
pub struct LocalCopy {
    source: Box<dyn Source>,
    initial: Snapshot,
    size: Option<u64>,
    sha256: Option<[u8; 32]>,
}
/// Check supplied size, open and fstat before any spool file/claim is made.
pub fn open(bundle: BundleRef<'_>, m: u64) -> Result<LocalCopy, Failure> {
    open_with(bundle, m, &Host)
}
pub fn open_with(
    bundle: BundleRef<'_>,
    m: u64,
    sources: &dyn Sources,
) -> Result<LocalCopy, Failure> {
    if bundle.size().is_some_and(|n| n > m) {
        return Err(Failure::Data(DataFailure::InputBytes(m)));
    }
    let path = bundle.path().ok_or_else(internal)?;
    let source = sources.open(Path::new(path)).map_err(|e| {
        if e.kind() == io::ErrorKind::OutOfMemory {
            Failure::Run(RunFailure::new(ReasonCode::HostMemory))
        } else {
            Failure::Data(DataFailure::SourceMissing)
        }
    })?;
    let initial = source.snapshot().map_err(|_| host_io())?;
    if !initial.regular {
        return Err(Failure::Data(DataFailure::SourceNotRegular));
    }
    if initial.size > m {
        return Err(Failure::Data(DataFailure::InputBytes(m)));
    }
    // O_NONBLOCK has no effect on a regular file, but clear it after fstat as
    // required for production sources. Source adapters need no fd operations.
    source.blocking().map_err(|_| host_io())?;
    let sha256 = bundle
        .sha256()
        .map(digest)
        .transpose()
        .map_err(|()| internal())?;
    Ok(LocalCopy {
        source,
        initial,
        size: bundle.size(),
        sha256,
    })
}
impl LocalCopy {
    #[must_use]
    pub fn size(&self) -> u64 {
        self.initial.size
    }
    /// Copy/hash through one 64 KiB buffer on the caller's blocking pool.
    /// No re-stat, integrity comparison, seal or cleanup occurs here. Return
    /// the result to the controller and always call [`LocalFinish::finish`],
    /// including after a failed or stopped copy. The destination owner stays
    /// with the caller throughout both steps.
    pub fn run(
        mut self,
        bundle: u32,
        destination: &mut OwnedFile,
        stopped: &dyn Fn() -> bool,
        hooks: &dyn Hooks,
    ) -> LocalFinish {
        let before = crate::alloc::refusals();
        let mut end = StageEnd {
            bundle,
            failure: None,
            integrity: None,
            facts: None,
            claimed_bytes: 0,
        };
        let mut received = None;
        let result = catch_unwind(AssertUnwindSafe(|| self.copy(destination, stopped, hooks)));
        end.failure = match result {
            Ok(Ok(body)) => {
                received = body;
                None
            }
            Ok(Err(failure)) => Some(failure),
            Err(_) => Some(internal()),
        };
        if crate::alloc::refusals() != before {
            reduce(&mut end, RunFailure::new(ReasonCode::HostMemory));
        }
        LocalFinish {
            copy: self,
            end,
            received,
        }
    }
    fn copy(
        &mut self,
        destination: &mut OwnedFile,
        stopped: &dyn Fn() -> bool,
        hooks: &dyn Hooks,
    ) -> Result<Option<Facts>, Failure> {
        check_stop(stopped)?;
        if destination.length() != 0 || destination.claimed() != 0 {
            return Err(internal());
        }
        destination.reserve(self.initial.size)?;
        // The copy buffer is on the heap: a worker or pool thread's stack is
        // sized for the core, not for a 64 KiB local. Its refusal is the
        // host's memory, like any other allocator refusal.
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(BUFFER)
            .map_err(|_| Failure::Run(RunFailure::new(ReasonCode::HostMemory)))?;
        buffer.resize(BUFFER, 0);
        let mut hash = Sha256::new();
        let mut copied = 0;
        while copied < self.initial.size {
            check_stop(stopped)?;
            hooks.point(Point::Buffer(copied));
            check_stop(stopped)?;
            let n = usize::try_from((self.initial.size - copied).min(BUFFER as u64))
                .map_err(|_| internal())?;
            let buffer = buffer.get_mut(..n).ok_or_else(internal)?;
            let read = loop {
                match self.source.read(buffer) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        check_stop(stopped)?;
                    }
                    result => break result.map_err(|_| host_io())?,
                }
            };
            if read == 0 {
                return Ok(None);
            }
            let bytes = buffer.get(..read).ok_or_else(internal)?;
            destination.append(bytes, stopped)?;
            hash.update(bytes);
            copied += read as u64;
        }
        check_stop(stopped)?;
        Ok(Some(Facts {
            size: copied,
            sha256: hash.finalize().into(),
        }))
    }
    fn unchanged(&self, hooks: &dyn Hooks) -> Result<(), Failure> {
        hooks.point(Point::BeforeRestat);
        if self.source.snapshot().map_err(|_| host_io())? == self.initial {
            Ok(())
        } else {
            Err(Failure::Data(DataFailure::SourceChanged))
        }
    }
}
/// A copy result carrying the pinned source back to the controller. A short
/// body keeps no facts, but still needs re-stat so `source_changed` wins over
/// early EOF. A read/write failure is already authoritative and skips re-stat.
#[must_use = "return the copy result to the controller and call finish"]
pub struct LocalFinish {
    copy: LocalCopy,
    end: StageEnd,
    received: Option<Facts>,
}
impl LocalFinish {
    /// Controller-only finalisation: re-stat, size/SHA checks, then `fchmod`
    /// and close. Cleanup, stop checks and run-failure precedence apply even
    /// when the bulk copy failed. Pass the same destination used by `run`.
    #[must_use]
    pub fn finish(
        mut self,
        destination: &mut OwnedFile,
        stopped: &dyn Fn() -> bool,
        hooks: &dyn Hooks,
    ) -> StageEnd {
        let before = crate::alloc::refusals();
        if self.end.failure.is_none() {
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.finalise(destination, stopped, hooks)
            }));
            self.end.failure = match result {
                Ok(Ok(())) => None,
                Ok(Err(failure)) => Some(failure),
                Err(_) => Some(internal()),
            };
        }
        let mut end = self.end;
        if crate::alloc::refusals() != before {
            reduce(&mut end, RunFailure::new(ReasonCode::HostMemory));
        }
        // Only ordinary data failures are cleaned here. A run failure or stop
        // preserves the owner/claim for handover, and never releases on drop.
        let cleanup = catch_unwind(AssertUnwindSafe(|| {
            if matches!(end.failure, Some(Failure::Data(_))) {
                if stopped() {
                    end.failure = Some(Failure::Stopped);
                } else if let Err(error) = destination.remove() {
                    end.failure = Some(error.into());
                }
            }
        }));
        if cleanup.is_err() {
            reduce(&mut end, RunFailure::new(ReasonCode::Internal));
        }
        if crate::alloc::refusals() != before {
            reduce(&mut end, RunFailure::new(ReasonCode::HostMemory));
        }
        end.claimed_bytes = destination.claimed();
        if !matches!(
            end.failure,
            None | Some(Failure::Data(DataFailure::IntegrityMismatch))
        ) {
            end.facts = None;
            end.integrity = None;
        }
        if matches!(end.failure, Some(Failure::Run(_)))
            && catch_unwind(AssertUnwindSafe(|| hooks.request_stop(&end))).is_err()
        {
            reduce(&mut end, RunFailure::new(ReasonCode::Internal));
        }
        end
    }
    fn finalise(
        &mut self,
        destination: &mut OwnedFile,
        stopped: &dyn Fn() -> bool,
        hooks: &dyn Hooks,
    ) -> Result<(), Failure> {
        check_stop(stopped)?;
        self.copy.unchanged(hooks)?;
        let received = self.received.ok_or_else(host_io)?;
        self.end.facts = Some(received);
        // A full, unchanged body is available: size has precedence over SHA.
        self.end.integrity = if self.copy.size.is_some_and(|n| n != received.size) {
            Some(super::Integrity::Size)
        } else if self.copy.sha256.is_some_and(|h| h != received.sha256) {
            Some(super::Integrity::Sha256)
        } else {
            None
        };
        if self.end.integrity.is_some() {
            return Err(Failure::Data(DataFailure::IntegrityMismatch));
        }
        hooks.point(Point::BeforeSeal);
        check_stop(stopped)?;
        destination.seal(stopped)?;
        Ok(())
    }
}

fn check_stop(stopped: &dyn Fn() -> bool) -> Result<(), Failure> {
    if stopped() {
        Err(Failure::Stopped)
    } else {
        Ok(())
    }
}
fn host_io() -> Failure {
    Failure::Run(RunFailure::new(ReasonCode::HostIo))
}
fn internal() -> Failure {
    Failure::Run(RunFailure::new(ReasonCode::Internal))
}
fn reduce(end: &mut StageEnd, failure: RunFailure) {
    let mut run = match end.failure {
        Some(Failure::Run(f)) => Some(f),
        _ => None,
    };
    RunFailure::reduce(&mut run, failure);
    end.failure = run.map(Failure::Run);
}
fn digest(text: &str) -> Result<[u8; 32], ()> {
    let mut out = [0; 32];
    if text.len() != 64 {
        return Err(());
    }
    let (pairs, _) = text.as_bytes().as_chunks::<2>();
    for (slot, pair) in out.iter_mut().zip(pairs) {
        let hex = |b| match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err(()),
        };
        let [a, b] = *pair;
        *slot = hex(a)? * 16 + hex(b)?;
    }
    Ok(out)
}
