use super::{
    BUFFER, Error, Member, Record, Status, Summary,
    encode::{self, Count, Sink},
};
use crate::{
    digest::Hashes,
    spool::{Directory, SpoolFile},
    work::{Work, rounded},
};
use std::sync::Arc;
use unityrs_core::limits::{Charge, Meter, MeteredVec};

/// A charged 64 KiB buffer. Drop discards its pending bytes. After any
/// failure append and finish refuse, and never issue another flush.
pub struct Writer {
    file: SpoolFile,
    buffer: Vec<u8>,
    _charge: Charge,
    failed: bool,
}
impl Writer {
    /// Charge before allocating, using the caller's attempt meter (or
    /// the failure writer's phase meter). No per-line allocation.
    pub fn new(directory: &Directory, meter: &Arc<Meter>) -> Result<Self, Error> {
        let (buffer, charge) = MeteredVec::<u8>::with_capacity(meter, BUFFER)?.into_parts();
        Ok(Self {
            file: directory.records()?,
            buffer,
            _charge: charge,
            failed: false,
        })
    }
    /// Ordinary records. Members must use `member`, which also accounts
    /// their largest final-manifest encoding before appending.
    pub fn append(&mut self, r: Record<'_>, work: &mut Work) -> Result<(), Error> {
        if matches!(r, Record::Member(_)) {
            self.failed = true;
            return Err(Error::Internal);
        }
        self.append_inner(r, work)
    }
    /// One call per expansion, in expansion order. Archive is the final
    /// key from the spec, never its compact numeric artifact index.
    pub fn member(&mut self, archive: &str, m: Member<'_>, work: &mut Work) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Internal);
        }
        let result = (|| {
            work.member(encode::member_work(archive, m)?)?;
            self.append_inner(Record::Member(m), work)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn append_inner(&mut self, r: Record<'_>, work: &mut Work) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Internal);
        }
        let result = (|| {
            encode::record(&mut Count(0), r)?; // reject an invalid line before any part is written
            let mut sink = Buffered {
                file: &mut self.file,
                buffer: &mut self.buffer,
                work,
                failure: false,
                stopped: &|| false,
            };
            encode::record(&mut sink, r)
        })();
        if result.is_err() {
            self.failed = true;
        }
        #[cfg(feature = "test-hooks")]
        if result.is_ok() {
            crate::work::tap::tell(crate::work::tap::Event::Record);
        }
        result
    }
    /// Complete and close a successful artifact. The returned summary
    /// contains exactly the figures encoded, including its own last flush.
    pub fn finish<'a>(self, summary: Summary<'a>, work: &mut Work) -> Result<Summary<'a>, Error> {
        self.finish_stoppable(summary, work, &|| false)
    }
    /// Successful seal with cancellation at each records claim and flush.
    /// On cancellation, pending bytes are discarded and claims stay owned.
    pub fn finish_stoppable<'a>(
        self,
        summary: Summary<'a>,
        work: &mut Work,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Summary<'a>, Error> {
        if summary.status == Status::Failed || summary.figures.is_none() || summary.reason.is_some()
        {
            return Err(Error::Internal);
        }
        self.seal(summary, work, false, stopped)
    }
    fn seal<'a>(
        mut self,
        mut summary: Summary<'a>,
        work: &mut Work,
        failure: bool,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Summary<'a>, Error> {
        if self.failed {
            return Err(Error::Internal);
        }
        // A failure's figures come from its end message. Normalize once so
        // the returned figures and the serialized line have the same values.
        if failure && let Some(f) = &mut summary.figures {
            f.skips = 0;
            f.unresolved_scripts = 0;
        }
        let spooled_part = if failure {
            summary.spooled_part
        } else {
            work.spooled_part()
        };
        // Integer slots have fixed width, so measured figures cannot change
        // the final flush's size. Converge the figures and the cut flag before
        // writing; count actual encoded bytes, including slot whitespace.
        let mut previous = None;
        for _ in 0..128 {
            let mut count = Count(0);
            summary.cut |= encode::summary(&mut count, summary)?;
            let pending = self
                .buffer
                .len()
                .checked_add(count.0)
                .ok_or(Error::Internal)?;
            let cost = final_cost(pending)?;
            if !failure && let Some(f) = &mut summary.figures {
                f.export_work = work.export_work().saturating_add(cost);
            }
            summary.spooled_part = spooled_part.saturating_add(cost);
            summary.claimed_bytes = self.file.claimed_with_append(pending as u64)?;
            let state = (
                count.0,
                summary.figures,
                summary.spooled_part,
                summary.claimed_bytes,
                summary.cut,
            );
            if previous == Some(state) {
                let mut sink = Buffered {
                    file: &mut self.file,
                    buffer: &mut self.buffer,
                    work,
                    failure,
                    stopped,
                };
                encode::summary(&mut sink, summary)?;
                sink.flush()?;
                self.file.finish_stoppable(stopped)?;
                return Ok(summary);
            }
            previous = Some(state);
        }
        Err(Error::Internal)
    }
}
fn final_cost(pending: usize) -> Result<u64, Error> {
    let full = (pending / BUFFER) as u64 * BUFFER as u64;
    let tail = pending % BUFFER;
    full.checked_add(if tail == 0 {
        0
    } else {
        rounded(tail as u64, 4096).ok_or(Error::Internal)?.max(4096)
    })
    .ok_or(Error::Internal)
}
struct Buffered<'a> {
    file: &'a mut SpoolFile,
    buffer: &'a mut Vec<u8>,
    work: &'a mut Work,
    failure: bool,
    stopped: &'a dyn Fn() -> bool,
}
impl Buffered<'_> {
    fn flush(&mut self) -> Result<(), Error> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if self.failure {
            self.work.failure_record(self.buffer.len() as u64)?;
        } else {
            self.work.records(self.buffer.len() as u64)?;
        }
        self.file.append_stoppable(self.buffer, self.stopped)?;
        self.buffer.clear();
        Ok(())
    }
}
impl Sink for Buffered<'_> {
    fn put(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            let n = (BUFFER - self.buffer.len()).min(bytes.len());
            self.buffer
                .extend_from_slice(bytes.get(..n).ok_or(Error::Internal)?);
            bytes = bytes.get(n..).ok_or(Error::Internal)?;
            if self.buffer.len() == BUFFER {
                self.flush()?;
            }
        }
        Ok(())
    }
}
/// Borrow synthetic or exported charged bytes, hash and synchronously copy
/// them. Count the whole file before its first disk claim. No preallocation.
pub fn copy_output(
    directory: &Directory,
    plan_index: u64,
    bytes: &[u8],
    work: &mut Work,
) -> Result<Hashes, Error> {
    let hashes = Hashes::of(bytes);
    work.file(bytes.len() as u64)?;
    let mut file = directory.output(plan_index)?;
    file.append(bytes)?;
    file.finish()?;
    Ok(hashes)
}
/// Separate one-line failed-input artifact, for the controller's blocking
/// pool. Pass the failed attempt's end-message figures in `summary`, with
/// `None` figures before dispatch. Preserve those measured figures (except
/// zeroing skips and unresolved scripts); add only this record's flush to
/// `spooled_part`, leaving `export_work` unchanged. Never renames. Failed
/// attempt candidates remain in their original owner.
pub fn write_failure_record<'a>(
    directory: &Directory,
    meter: &Arc<Meter>,
    summary: Summary<'a>,
) -> Result<Summary<'a>, Error> {
    write_failure_record_stoppable(directory, meter, summary, &|| false)
}
/// Failure writer with the same cooperative flush checks as successful seals.
pub fn write_failure_record_stoppable<'a>(
    directory: &Directory,
    meter: &Arc<Meter>,
    summary: Summary<'a>,
    stopped: &dyn Fn() -> bool,
) -> Result<Summary<'a>, Error> {
    if summary.status != Status::Failed {
        return Err(Error::Internal);
    }
    Writer::new(directory, meter)?.seal(summary, &mut Work::new(0), true, stopped)
}
