//! Fixed owning future slab, polled on the controller runtime, never spawned.
//! Stopped futures stay here through the finisher; outcome writes happen in
//! their final poll. Capacity and phase admission are taken together (W19).
use super::{
    Context, Source,
    source::Merge,
    transfer::{self, Item},
};
use crate::{
    outcomes::Code,
    phase::{self, Buffer, Error},
    publish::{Owned, Point, Report, archive::Descriptor},
    sink::store::Store,
    store::{multipart, put::Request},
    sys::Shared,
    winners::{
        self,
        stream::{By, Cursor},
    },
};
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
};
type FutureRun = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
pub(crate) struct Run {
    future: FutureRun,
    transfer: bool,
}
const ENVELOPE: u64 = 256 * 1024;
const TRANSFER_OVERHEAD: u64 = 1280 * 1024;

pub(crate) fn run(
    owned: &Shared<Owned>,
    store: &Shared<Store>,
    execution: &crate::execution::Execution,
) -> Result<Report, Error> {
    let result = phase::on_pool_named("store publication", || {
        execution
            .runtime()
            .ok_or_else(Error::internal)?
            .block_on(schedule(owned, store, execution.handle()))
    });
    if result.is_ok() {
        owned.account.publication(|| Ok(()))?;
    }
    result
}
async fn schedule(
    owned: &Shared<Owned>,
    store: &Shared<Store>,
    pool: &tokio::runtime::Handle,
) -> Result<Report, Error> {
    let stopped = || owned.control.stopped();
    owned.hooks.point(Point::Admission);
    phase::check(&stopped)?;
    if owned.handoff.publication_blocked {
        return Ok(Report::default());
    }
    let _fixed = owned.account.meter().charge(ENVELOPE)?;
    let mut zeros = Buffer::new(owned.account.meter(), 64 * 1024)?;
    zeros.fill(0);
    let root = &owned.spool_lease.as_ref().ok_or_else(Error::internal)?.run;
    *store.tags.lock() = Some(crate::spool::random::Mutable::create(
        &crate::spool::path::join(root, std::path::Path::new("store_tags"))?,
        owned
            .handoff
            .counts
            .winners
            .checked_mul(101)
            .ok_or_else(Error::internal)?,
        &owned
            .spool_lease
            .as_ref()
            .ok_or_else(Error::internal)?
            .counter,
        &*owned.fs,
        &zeros,
        &stopped,
    )?);
    drop(zeros);
    if owned.facts.spec.resume {
        install(store, 0, resume(owned.clone()), false)?;
        drain(owned, store).await?;
    }
    wave(owned, store, pool, false).await?;
    wave(owned, store, pool, true).await?;
    owned.hooks.point(Point::Drained);
    phase::check(&stopped)?;
    install(store, 0, verify(owned.clone()), false)?;
    drain(owned, store).await?;
    Ok(Report::default())
}
fn boxed(
    future: impl Future<Output = Result<(), Error>> + Send + 'static,
) -> Result<FutureRun, Error> {
    // Included in ENVELOPE/each admitted slot's fixed envelope, checked
    // before the allocation, independent of the number of keys.
    if size_of_val(&future) > 128 * 1024 {
        return Err(Error::internal());
    }
    let boxed: Box<dyn Future<Output = Result<(), Error>> + Send> = crate::alloc::try_box(future)?;
    Ok(Box::into_pin(boxed))
}
fn install(
    store: &Store,
    slot: usize,
    future: impl Future<Output = Result<(), Error>> + Send + 'static,
    transfer: bool,
) -> Result<(), Error> {
    let future = boxed(future)?;
    let mut runs = store.runs.lock();
    let entry = runs.get_mut(slot).ok_or_else(Error::internal)?;
    if entry.is_some() {
        return Err(Error::internal());
    }
    *entry = Some(Run { future, transfer });
    Ok(())
}
fn turn(owned: &Owned, store: &Store, cx: &mut std::task::Context<'_>) -> Result<usize, Error> {
    while let Poll::Ready(result) = store.wake.poll_wait(cx) {
        result?;
    }
    phase::check(&|| owned.control.stopped())?;
    let mut active = 0;
    for entry in store.runs.lock().iter_mut() {
        let Some(future) = entry.as_mut() else {
            continue;
        };
        match future.future.as_mut().poll(cx) {
            Poll::Pending => active += 1,
            Poll::Ready(result) => {
                result?;
                if future.transfer
                    && let Some(p) = &owned.progress
                {
                    p.update(|s| s.transfers.uploads_active -= 1);
                }
                *entry = None;
            }
        }
        phase::check(&|| owned.control.stopped())?;
    }
    Ok(active)
}
async fn drain(owned: &Owned, store: &Store) -> Result<(), Error> {
    poll_fn(|cx| match turn(owned, store, cx) {
        Ok(0) => Poll::Ready(Ok(())),
        Ok(_) => Poll::Pending,
        Err(e) => Poll::Ready(Err(e)),
    })
    .await
}
enum Queue {
    Loose(Cursor<'static>, u64),
    Archives(winners::archives::Reader<'static>),
}
impl Queue {
    fn next(&mut self, owned: &Owned) -> Result<Option<Item>, Error> {
        let meter = owned.account.meter();
        let stopped = || owned.control.stopped();
        Ok(match self {
            Self::Loose(cursor, index) => {
                let Some(_) = cursor.row()? else {
                    return Ok(None);
                };
                let mut frame = crate::sort::Frame::new(meter)?;
                frame.set(cursor.bytes())?;
                let item = Item::Loose {
                    frame,
                    index: *index,
                };
                *index += 1;
                cursor.advance(&stopped)?;
                Some(item)
            }
            Self::Archives(reader) => {
                let Some(next) = reader.next(&stopped)? else {
                    return Ok(None);
                };
                let mut descriptor = Descriptor::new(meter)?;
                descriptor.take(next)?;
                Some(Item::Archive(descriptor))
            }
        })
    }
}
#[allow(clippy::too_many_lines)] // One admission turn owns slot availability, phase capacity and its first poll.
async fn wave(
    owned: &Shared<Owned>,
    store: &Store,
    pool: &tokio::runtime::Handle,
    archives: bool,
) -> Result<(), Error> {
    let stopped = || owned.control.stopped();
    let mut queue = if archives {
        let Some(metadata) = &owned.handoff.metadata else {
            return Ok(());
        };
        Queue::Archives(winners::archives::Reader::open(
            &owned.workspace,
            metadata,
            owned.account.meter(),
            &stopped,
        )?)
    } else {
        Queue::Loose(
            Cursor::read(
                &owned.workspace,
                owned.handoff.winners.as_ref(),
                &By::Key,
                owned.account.meter(),
                &stopped,
                true,
            )?,
            0,
        )
    };
    let mut next = queue.next(owned)?;
    poll_fn(|cx| {
        let result = (|| {
            let mut active = turn(owned, store, cx)?;
            while let Some(item) = &next {
                let listed = match item {
                    Item::Loose { index, .. } if owned.outcomes.loose(*index)? == Code::Present => {
                        next = queue.next(owned)?;
                        continue;
                    }
                    Item::Archive(d) => match d.eligibility(false) {
                        crate::archive::Eligibility::Empty
                        | crate::archive::Eligibility::Withheld => {
                            next = queue.next(owned)?;
                            continue;
                        }
                        crate::archive::Eligibility::Oversized => {
                            super::source::mark(
                                &owned.outcomes,
                                super::Target::Archive(u64::from(d.index())),
                                Code::ArchiveTooLarge,
                                None,
                                None,
                            )?;
                            next = queue.next(owned)?;
                            continue;
                        }
                        crate::archive::Eligibility::Eligible => store
                            .listed
                            .lock()
                            .get(d.index() as usize)
                            .copied()
                            .flatten(),
                    },
                    Item::Loose { .. } => None,
                };
                // A timed-out lookup can outlive its previous key. No payload
                // preparation or reservation starts until that slot is idle.
                let mut available = None;
                for (index, entry) in store.runs.lock().iter().enumerate() {
                    if entry.is_none() {
                        match store.slot(index)?.poll_lookup(cx) {
                            Poll::Ready(Ok(())) => {
                                available = Some(index);
                                break;
                            }
                            Poll::Ready(Err(f)) => {
                                return Err(crate::sink::store::errors::fault(f));
                            }
                            Poll::Pending => {}
                        }
                    }
                }
                let Some(slot) = available else {
                    break;
                };
                let working = working(item.size()?)?;
                if !owned.account.admits(working) {
                    if active == 0 {
                        return Err(Error::internal());
                    }
                    break;
                }
                let charge = owned.account.meter().charge(working)?;
                let item = next.take().ok_or_else(Error::internal)?;
                install(
                    store,
                    slot,
                    transfer::run(owned.clone(), slot, item, listed, pool.clone(), charge),
                    true,
                )?;
                if let Some(p) = &owned.progress {
                    p.update(|s| s.transfers.uploads_active += 1);
                }
                next = queue.next(owned)?;
                // Poll newly admitted work before returning Pending. Its IO,
                // pool task or timer installs the wake that will resume us.
                active = turn(owned, store, cx)?;
            }
            Ok(active == 0 && next.is_none())
        })();
        match result {
            Ok(true) => Poll::Ready(Ok(())),
            Ok(false) => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    })
    .await
}
async fn resume(owned: Shared<Owned>) -> Result<(), Error> {
    let crate::sink::Sink::Store(store) = &owned.sink else {
        return Err(Error::internal());
    };
    let tries = store.tries(&*owned.control);
    let request = Request::new(&store.clients[1], &tries, store.slot(0)?, store.deadline());
    let cx = Context {
        request: &request,
        account: &owned.account,
        workspace: &owned.workspace,
        prefix: &store.settings().prefix,
        stopped: &|| owned.control.stopped(),
    };
    let mut source = Merge::open(&owned, store)?;
    let mut listed = Buffer::new(owned.account.meter(), owned.facts.spec.archives.len())?;
    listed.fill(None);
    let report = super::resume::run(&cx, &mut source, &owned.outcomes, &mut listed).await?;
    store
        .listed
        .lock()
        .get_mut(..listed.len())
        .ok_or_else(Error::internal)?
        .copy_from_slice(&listed);
    store.figures.lock().resume_list_failed = u64::from(report.resume_list_failed);
    if report.present > 0 {
        store.figures.lock().verified = Some(false);
    }
    if report.stopped {
        return Err(Error::Stopped);
    }
    if let Some(p) = &owned.progress {
        p.update(|s| s.outputs.present += report.present);
    }
    Ok(())
}
async fn verify(owned: Shared<Owned>) -> Result<(), Error> {
    let crate::sink::Sink::Store(store) = &owned.sink else {
        return Err(Error::internal());
    };
    let mut source = Merge::open(&owned, store)?;
    let mut published = 0;
    while let Some(row) = source.current()? {
        published += u64::from(row.code.published());
        source.advance()?;
    }
    store.figures.lock().verified = (published > 0).then_some(false);
    drop(source);
    let mut source = Merge::open(&owned, store)?;
    let tries = store.tries(&*owned.control);
    let request = Request::new(&store.clients[1], &tries, store.slot(0)?, store.deadline());
    let cx = Context {
        request: &request,
        account: &owned.account,
        workspace: &owned.workspace,
        prefix: &store.settings().prefix,
        stopped: &|| owned.control.stopped(),
    };
    let report = super::verify::run(&cx, &mut source, &owned.outcomes, published, 0).await?;
    drop(source);
    if let Some(p) = &owned.progress {
        let mut source = Merge::open(&owned, store)?;
        let (mut written, mut present, mut failed) = (0, 0, 0);
        while let Some(row) = source.current()? {
            if matches!(row.target, super::Target::Loose(_)) {
                match row.code {
                    Code::Written | Code::WrittenBySize => written += 1,
                    Code::Present => present += 1,
                    code if code.reason().is_some() => failed += 1,
                    _ => {}
                }
            }
            source.advance()?;
        }
        p.update(|s| {
            s.outputs.published = written;
            s.outputs.present = present;
            s.outputs.failed = failed;
        });
    }
    let mut figures = store.figures.lock();
    figures.verified = report.verified;
    figures.verified_by_size = report.verified_by_size;
    figures.verify_list_failed = u64::from(report.verify_list_failed);
    Ok(())
}
fn working(size: u64) -> Result<u64, Error> {
    multipart::reservation(size)
        .map_err(|e| transfer::failure(e).err().unwrap_or_else(Error::internal))?
        .checked_add(TRANSFER_OVERHEAD)
        .ok_or_else(Error::internal)
}
#[cfg(test)]
mod tests {
    #[test]
    fn the_largest_legal_part_table_fits_the_minimum_phase_budget() {
        let size = crate::store::put::PART_SIZE as u64 * 10_000;
        assert!(
            super::working(size).unwrap() + super::ENVELOPE + 384 * 1024
                <= crate::phase::WORKING_SET
        );
    }
}
