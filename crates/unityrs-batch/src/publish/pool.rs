//! One streaming dispatcher and bounded pool drives. Each drive retains its
//! buffer across winners and lends it to resume before copying a miss.
use super::{BUFFER, Context, Gate, Hooks, OVERHEAD, Point, Report, archive, local, payload};
use crate::{
    control::{Control, Culprit},
    execution::Execution,
    phase::{self, Account, Buffer, Error},
    phase1::PreparedRun,
    sink::directory::Directory,
    spool::{FileSystem, workspace::Workspace},
    sys::{Lock, Shared},
    winners::{
        self,
        stream::{By, Cursor},
    },
};
use std::{path::PathBuf, sync::Arc};

pub struct Owned {
    pub prepared: Lock<PreparedRun>,
    pub done: PathBuf,
    pub workspace: Workspace,
    pub handoff: winners::Handoff,
    pub directory: Arc<Directory>,
    pub outcomes: crate::outcomes::Outcomes,
    pub account: Arc<Account>,
    pub gate: Gate,
    pub fs: Arc<dyn FileSystem>,
    pub control: Arc<dyn Control>,
    pub hooks: Arc<dyn Hooks + Send>,
    pub progress: Option<crate::progress::live::Sender>,
    pub spool_lease: Option<Arc<crate::start::spool::Started>>,
}
struct Queue {
    cursor: Cursor<'static>,
    next: u64,
    /// Archives follow every winner, in archive-key order.
    archives: Option<winners::archives::Reader<'static>>,
    taken: u64,
}
impl Queue {
    fn open(owned: &Owned, stopped: &dyn Fn() -> bool) -> Result<Self, Error> {
        let meter = owned.account.meter();
        Ok(Self {
            cursor: Cursor::read(
                &owned.workspace,
                owned.handoff.winners.as_ref(),
                &By::Key,
                meter,
                stopped,
                true,
            )?,
            next: 0,
            archives: owned
                .handoff
                .metadata
                .as_ref()
                .map(|m| winners::archives::Reader::open(&owned.workspace, m, meter, stopped))
                .transpose()?,
            taken: 0,
        })
    }
    fn take(
        &mut self,
        frame: &mut crate::sort::Frame,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Option<u64>, Error> {
        phase::check(stopped)?;
        if self.cursor.row()?.is_none() {
            return Ok(None);
        }
        frame.set(self.cursor.bytes())?;
        let index = self.next;
        self.next = self.next.checked_add(1).ok_or_else(Error::internal)?;
        self.cursor.advance(stopped)?;
        Ok(Some(index))
    }
    /// Copies one bounded descriptor while holding the lock; nothing borrowed
    /// from the reader's frame leaves it.
    fn archive(
        &mut self,
        slot: &mut archive::Descriptor,
        stopped: &dyn Fn() -> bool,
    ) -> Result<bool, Error> {
        phase::check(stopped)?;
        let Some(reader) = self.archives.as_mut() else {
            return Ok(false);
        };
        let Some(next) = reader.next(stopped)? else {
            return Ok(false);
        };
        slot.take(next)?;
        self.taken = self.taken.checked_add(1).ok_or_else(Error::internal)?;
        Ok(true)
    }
}
/// Concurrent pool work uses thread-local allocator audits and one final
/// shared zero-charge audit after every task has been received. On failure
/// owners remain with tasks; the caller must bypass normal finishing.
pub fn parallel(
    owned: &(impl std::ops::Deref<Target = Owned> + Clone + Send + Sync + 'static),
    execution: &Execution,
    concurrency: u32,
) -> Result<Report, Error> {
    let result = phase::on_pool(|| schedule(owned, execution, concurrency));
    if result.is_ok() {
        owned.account.publication(|| Ok(()))?;
    }
    result
}
fn schedule(
    owned: &(impl std::ops::Deref<Target = Owned> + Clone + Send + Sync + 'static),
    execution: &Execution,
    concurrency: u32,
) -> Result<Report, Error> {
    let stopped = || owned.control.stopped();
    owned.hooks.point(Point::Admission);
    phase::check(&stopped)?;
    if owned.handoff.publication_blocked {
        return Ok(Report::default());
    }
    let (declared, drives) = drives(owned, concurrency)?;
    if drives == 0 {
        return Ok(Report::default());
    }
    // The second MiB in each envelope covers the shared readers and all task,
    // frame and adapter storage. No descriptor is queued per winner/archive.
    // Keep half a MiB for fixed dispatcher bookkeeping. Its readers and the
    // per-drive frames have separate exact charges within the other half.
    let _dispatcher = owned.account.meter().charge(OVERHEAD / 2)?;
    let queue = Shared::new(Lock::new(Queue::open(owned, &stopped)?))?;
    let mut tasks = Buffer::new(
        owned.account.meter(),
        usize::try_from(drives).map_err(|_| Error::internal())?,
    )?;
    for _ in 0..drives {
        phase::check(&stopped)?;
        let state = owned.clone();
        let queue = queue.clone();
        tasks.push(Some(
            execution
                .spawn(move || {
                    let result = phase::on_pool(|| drive(&state, &queue));
                    if let Err(Error::Run(failure)) = result {
                        state.control.request_stop(failure, Culprit::None);
                    }
                    result
                })
                .map_err(Error::from)?,
        ))?;
    }
    let mut report = Report::default();
    let mut left = drives;
    let mut failure = None;
    while left != 0 {
        for task in tasks.iter_mut() {
            if task
                .as_ref()
                .is_some_and(crate::execution::Task::is_finished)
            {
                let result = execution
                    .finish(&task.take().ok_or_else(Error::internal)?)
                    .map_err(|()| Error::internal())?;
                left -= 1;
                match result {
                    Ok(end) => {
                        report.written += end.written;
                        report.present += end.present;
                        report.failed += end.failed;
                    }
                    Err(Error::Run(f)) => crate::job::RunFailure::reduce(&mut failure, f),
                    Err(Error::Stopped) => {}
                }
            }
        }
        if let Some(f) = failure {
            return Err(Error::Run(f));
        }
        phase::check(&stopped)?;
        if left != 0 {
            execution.wait()?;
        }
    }
    let drained = queue.lock();
    if drained.next != owned.handoff.counts.winners || drained.taken != declared {
        return Err(Error::internal());
    }
    drop(drained);
    owned.hooks.point(Point::Drained);
    phase::check(&stopped)?;
    Ok(report)
}
/// Declared archives and drive count. Every archive is taken once, so an
/// archive-only run still starts a drive; zero means there is no work.
fn drives(owned: &Owned, concurrency: u32) -> Result<(u64, u64), Error> {
    let declared =
        u64::try_from(owned.prepared.lock().spec.archives.len()).map_err(|_| Error::internal())?;
    if (declared == 0) != owned.handoff.metadata.is_none() {
        return Err(Error::internal());
    }
    let work = owned
        .handoff
        .counts
        .winners
        .checked_add(declared)
        .ok_or_else(Error::internal)?;
    let drives = u64::from(concurrency)
        .min(owned.account.limit() / super::working_set(declared != 0))
        .min(work);
    if drives == 0 && work != 0 {
        return Err(Error::internal());
    }
    Ok((declared, drives))
}
fn drive(owned: &Owned, queue: &Lock<Queue>) -> Result<Report, Error> {
    let _overhead = owned.account.meter().charge(OVERHEAD)?;
    let stopped = || owned.control.stopped();
    let spec = owned.prepared.lock().spec.clone();
    let cx = Context {
        handoff: &owned.handoff,
        workspace: &owned.workspace,
        payloads: owned,
        directory: &owned.directory,
        outcomes: &owned.outcomes,
        gate: &owned.gate,
        account: &owned.account,
        run_id: &spec.run_id,
        resume: spec.resume,
        stopped: &stopped,
        hooks: &*owned.hooks,
    };
    let mut frame = crate::sort::Frame::new(owned.account.meter())?;
    let mut buffer = Buffer::new(owned.account.meter(), BUFFER)?;
    buffer.fill(0);
    let mut report = Report::default();
    loop {
        let next = queue.lock().take(&mut frame, &stopped)?;
        let Some(index) = next else {
            break;
        };
        let row = winners::codec::decode(frame.bytes())?;
        if row.state != winners::State::Publish {
            return Err(Error::internal());
        }
        if let Some(p) = &owned.progress {
            p.update(|s| s.transfers.uploads_active += 1);
        }
        let result = local::one(&cx, row, index, &mut buffer);
        if let Some(p) = &owned.progress {
            p.update(|s| {
                s.transfers.uploads_active -= 1;
                match result {
                    Ok(crate::outcomes::Code::Written) => {
                        s.outputs.published += 1;
                        s.transfers.uploads_done += 1;
                    }
                    Ok(crate::outcomes::Code::Present) => s.outputs.present += 1,
                    Ok(crate::outcomes::Code::PublishFailed) => {
                        s.outputs.failed += 1;
                        s.transfers.uploads_failed += 1;
                    }
                    _ => {}
                }
            });
        }
        match result? {
            crate::outcomes::Code::Written => report.written += 1,
            crate::outcomes::Code::Present => report.present += 1,
            crate::outcomes::Code::PublishFailed => report.failed += 1,
            _ => return Err(Error::internal()),
        }
    }
    if owned.handoff.metadata.is_some() {
        let mut slot = archive::Descriptor::new(owned.account.meter())?;
        while queue.lock().archive(&mut slot, &stopped)? {
            archive_one(owned, &cx, &slot, &mut buffer)?;
        }
    }
    Ok(report)
}
/// Archive transfers count as uploads; `outputs` stays the loose winners'.
fn archive_one(
    owned: &Owned,
    cx: &Context<'_>,
    slot: &archive::Descriptor,
    buffer: &mut [u8],
) -> Result<(), Error> {
    let progress = owned.progress.as_ref().filter(|_| {
        slot.eligibility(owned.handoff.publication_blocked) == crate::archive::Eligibility::Eligible
    });
    if let Some(p) = progress {
        p.update(|s| s.transfers.uploads_active += 1);
    }
    let result = archive::one(cx, slot, buffer);
    if let Some(p) = progress {
        p.update(|s| {
            s.transfers.uploads_active -= 1;
            match result {
                Ok(Some(crate::outcomes::Code::Written)) => s.transfers.uploads_done += 1,
                Ok(Some(crate::outcomes::Code::PublishFailed)) => s.transfers.uploads_failed += 1,
                _ => {}
            }
        });
    }
    result.map(|_| ())
}

impl payload::Payloads for Owned {
    fn open(&self, input: u32, plan_index: u64) -> Result<Box<dyn std::io::Read + Send>, Error> {
        let prepared = self.prepared.lock();
        payload::Committed {
            done: &self.done,
            states: &prepared.inputs,
            fs: &*self.fs,
        }
        .open(input, plan_index)
    }
}
