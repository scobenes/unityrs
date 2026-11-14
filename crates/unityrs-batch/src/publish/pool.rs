//! One streaming dispatcher and bounded pool drives. Each drive retains its
//! buffer across winners and lends it to resume before copying a miss.
use super::{BUFFER, Context, Gate, Hooks, OVERHEAD, Point, Report, WORKING_SET, local, payload};
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
}
impl Queue {
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
    phase::check(&stopped)?;
    if owned.handoff.publication_blocked {
        return Ok(Report::default());
    }
    let drives = u64::from(concurrency)
        .min(owned.account.limit() / WORKING_SET)
        .min(owned.handoff.counts.winners);
    if drives == 0 {
        return if owned.handoff.counts.winners == 0 {
            Ok(Report::default())
        } else {
            Err(Error::internal())
        };
    }
    // The second MiB in each envelope covers this shared reader and all task,
    // frame and adapter storage. No descriptor is queued per winner.
    // Keep half a MiB for fixed dispatcher bookkeeping. Its reader and the
    // per-drive frames have separate exact charges within the other half.
    let _dispatcher = owned.account.meter().charge(OVERHEAD / 2)?;
    let queue = Shared::new(Lock::new(Queue {
        cursor: Cursor::read(
            &owned.workspace,
            owned.handoff.winners.as_ref(),
            &By::Key,
            owned.account.meter(),
            &stopped,
            true,
        )?,
        next: 0,
    }))?;
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
    if queue.lock().next != owned.handoff.counts.winners {
        return Err(Error::internal());
    }
    owned.hooks.point(Point::Drained);
    phase::check(&stopped)?;
    Ok(report)
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
    Ok(report)
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
