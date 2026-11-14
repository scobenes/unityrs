//! Bounded directory publication: a serial component seam and concurrent
//! drives on the existing pool, with no per-winner task or path inventory.
pub mod archive;
mod local;
mod pool;
pub use pool::{Owned, parallel};
pub mod payload;
mod resume;
use crate::{
    outcomes::{Code, Outcomes},
    phase::{self, Account, Error},
    sink::directory::Directory,
    spool::workspace::Workspace,
    winners::{
        self,
        stream::{By, Cursor},
    },
};
use std::sync::atomic::{AtomicU8, Ordering};
use unityrs_core::limits::heap_cost;

pub const BUFFER: usize = 16 * 1024 * 1024;
pub const OVERHEAD: u64 = 1024 * 1024;
/// Includes the buffer allocation's chunk header and every fixed reader/path.
pub const WORKING_SET: u64 = 2 * OVERHEAD + heap_cost(16 * 1024 * 1024);
/// A drive that may also stream archives adds one stream envelope; the
/// driver's buffer is the stream's transport, so no second buffer is charged.
pub const ARCHIVE_WORKING_SET: u64 = WORKING_SET + crate::archive::ENVELOPE;
#[must_use]
pub const fn working_set(archives: bool) -> u64 {
    if archives {
        ARCHIVE_WORKING_SET
    } else {
        WORKING_SET
    }
}
/// The commit gate: free, held by one commit, or closed for good by the
/// controller's hand-over (2.9), which waits for an in-flight commit (W15).
#[derive(Default)]
pub struct Gate(AtomicU8);
const FREE: u8 = 0;
const HELD: u8 = 1;
const CLOSED: u8 = 2;
// std's mutex can lazily allocate platform storage at its first lock. This
// small non-reentrant mutex has no lazy heap, including on non-Linux hosts.
// A closed gate hands out an empty guard: it closes only on a stop, so the
// stop check every holder makes next refuses the commit.
struct Guard<'a>(Option<&'a Gate>);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Some(gate) = self.0 {
            gate.0.store(FREE, Ordering::Release);
        }
    }
}
pub struct Frozen<'a> {
    _guard: Guard<'a>,
    pub outcomes: &'a Outcomes,
}
impl Gate {
    fn lock(&self) -> Guard<'_> {
        loop {
            match self
                .0
                .compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return Guard(Some(self)),
                Err(CLOSED) => return Guard(None),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
    }
    pub fn freeze<'a>(&'a self, outcomes: &'a Outcomes) -> Result<Frozen<'a>, Error> {
        Ok(Frozen {
            _guard: self.lock(),
            outcomes,
        })
    }
    /// The hand-over's close: wait for a commit under way, then never
    /// reopen. Every outcome a commit wrote is in the files once this returns.
    pub fn close(&self) {
        while let Err(state) =
            self.0
                .compare_exchange(FREE, CLOSED, Ordering::Acquire, Ordering::Relaxed)
        {
            if state == CLOSED {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Admission,
    Resume,
    /// Hashing has released its loan of the reusable driver buffer.
    ResumeReleased,
    Transfer,
    Buffer,
    BeforeCommit,
    Authorized,
    Outcome,
    Drained,
}
/// Hooks run inside the audit and must not allocate. B9 adds process ownership.
pub trait Hooks: Sync {
    fn point(&self, _point: Point) {}
}
pub struct NoHooks;
impl Hooks for NoHooks {}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub written: u64,
    pub present: u64,
    pub failed: u64,
}
pub struct Context<'a> {
    pub handoff: &'a winners::Handoff,
    pub workspace: &'a Workspace,
    pub payloads: &'a dyn payload::Payloads,
    pub directory: &'a Directory,
    pub outcomes: &'a Outcomes,
    pub gate: &'a Gate,
    pub account: &'a Account,
    pub run_id: &'a str,
    pub resume: bool,
    pub stopped: &'a (dyn Fn() -> bool + Sync),
    pub hooks: &'a dyn Hooks,
}
impl Context<'_> {
    fn at(&self, point: Point) -> Result<(), Error> {
        self.hooks.point(point);
        phase::check(self.stopped)
    }
}
/// No reservation is attempted while capacity is unavailable. Call again only
/// after receiving work that releases capacity; busy is not an audited refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Busy,
    Drained(Report),
}
pub fn drive(cx: &Context<'_>) -> Result<Admission, Error> {
    phase::on_pool(|| cx.at(Point::Admission))?;
    if cx.account.snapshot().input.used != 0 {
        return Ok(Admission::Busy);
    }
    if !cx
        .account
        .admits(working_set(cx.handoff.metadata.is_some()))
    {
        return Err(Error::internal());
    }
    cx.account.publication(|| run(cx)).map(Admission::Drained)
}
fn run(cx: &Context<'_>) -> Result<Report, Error> {
    let _overhead = cx.account.meter().charge(OVERHEAD)?;
    let mut report = Report::default();
    if cx.handoff.publication_blocked {
        cx.at(Point::Drained)?;
        return Ok(report);
    }
    let mut winners = Cursor::read(
        cx.workspace,
        cx.handoff.winners.as_ref(),
        &By::Key,
        cx.account.meter(),
        cx.stopped,
        true,
    )?;
    let mut buffer = phase::Buffer::new(cx.account.meter(), BUFFER)?;
    buffer.fill(0);
    let mut index = 0;
    while let Some(row) = winners.row()? {
        if row.state != winners::State::Publish {
            return Err(Error::internal());
        }
        let result = local::one(cx, row, index, &mut buffer);
        match result {
            Ok(Code::Written) => report.written += 1,
            Ok(Code::Present) => report.present += 1,
            Ok(Code::PublishFailed) => report.failed += 1,
            Ok(_) => return Err(Error::internal()),
            Err(e) => return Err(e),
        }
        index = index.checked_add(1).ok_or_else(Error::internal)?;
        winners.advance(cx.stopped)?;
    }
    if index != cx.handoff.counts.winners {
        return Err(Error::internal());
    }
    drop(winners);
    archives(cx, &mut buffer)?;
    cx.at(Point::Drained)?;
    Ok(report)
}
/// Archives follow winners, in archive-key order, through the same buffer.
fn archives(cx: &Context<'_>, buffer: &mut [u8]) -> Result<(), Error> {
    let Some(metadata) = cx.handoff.metadata.as_ref() else {
        return Ok(());
    };
    let mut reader =
        winners::archives::Reader::open(cx.workspace, metadata, cx.account.meter(), cx.stopped)?;
    let mut slot = archive::Descriptor::new(cx.account.meter())?;
    while let Some(next) = reader.next(cx.stopped)? {
        slot.take(next)?;
        archive::one(cx, &slot, buffer)?;
    }
    Ok(())
}
fn sink_error(e: crate::sink::Error) -> Error {
    match e.class {
        crate::sink::Class::HostMemory => Error::memory(),
        crate::sink::Class::InvalidPath => Error::internal(),
        _ => Error::Run(crate::job::RunFailure::new(
            crate::attempt::ReasonCode::HostIo,
        )),
    }
}
