//! Sequential low-level multipart transfers (spec 1.6, 6.1; batch 2.9).
//! The caller assembles each charged Payload on its blocking pool before
//! calling part. No archive puller, scheduler or finisher is wired here.
use super::{
    Kind, abort,
    attempt::{Fault, Tries},
    classify::{Action, Ended},
    etag::{ETAG_MAX, Etag},
    put::{self, Error, PART_SIZE, Payload, Request, Written},
};
use crate::{
    phase::{Account, Buffer},
    spool::journal::{Entry, Journal},
};
use object_store::{
    ObjectStoreExt,
    multipart::{MultipartStore, PartId},
    path::Path,
};
use unityrs_core::limits::heap_cost;

pub const MAX_PARTS: u64 = 10000;
pub const UPLOAD_ID_MAX: usize = 1024;
/// Zero bytes is a single PUT, and exact multiples have no empty final part.
pub fn part_count(size: u64) -> Result<usize, Error> {
    let parts = size.div_ceil(PART_SIZE as u64).max(1);
    if parts > MAX_PARTS {
        return Err(Error::ArchiveTooLarge);
    }
    usize::try_from(parts).map_err(|_| Error::Fault(Fault::Internal))
}
/// Phase admission covers the larger of the payload and completion peaks.
/// The original part table has its separate `C_net` allowance (batch 5.3).
pub fn reservation(size: u64) -> Result<u64, Error> {
    let count = part_count(size)?;
    if count == 1 {
        return Ok(heap_cost(size));
    }
    Ok(heap_cost(PART_SIZE as u64).max(completion_reservation(count)?))
}
/// Bound `object_store` 0.14.2 / quick-xml 0.41.0 before copying any IDs.
/// Batch 2.9 caps IDs at `ETAG_MAX` and uploads at `MAX_PARTS` (spec 1.6, 8).
/// Partial XML escaping expands at most 5x. Each part adds 51 markup bytes,
/// five number digits, and at most 72 optional checksum markup bytes if an
/// ID parses as the dependency's `PartMetadata`; the root adds 51 bytes.
/// quick-xml's Struct buffers children (<2B capacity), then copies to the
/// output String; output growth overlaps old (<B) and new (<2B) blocks.
/// Thus two 2B blocks plus B cover both growth and final-copy peaks.
/// Also cover both tables (`PartId` and the dependency's 80-byte `MultipartPart`
/// on 64-bit), the input string plus three metadata fields per part
/// (metadata growth at `<2 * ETAG_MAX` capacity, separately rounded), and
/// 64 KiB for the one-part serializer and metadata parser scratch. Request/signing/response state stays `C_net`'s.
pub fn completion_reservation(count: usize) -> Result<u64, Error> {
    if count == 0 || count as u64 > MAX_PARTS {
        return Err(Error::ArchiveTooLarge);
    }
    let n = count as u64;
    let xml = 51 + n * (5 * ETAG_MAX as u64 + 51 + 5 + 72);
    Ok(2 * heap_cost(2 * xml)
        + heap_cost(xml)
        + heap_cost(n * size_of::<PartId>() as u64)
        + heap_cost(n * (10 * size_of::<usize>()) as u64)
        + n * (heap_cost(ETAG_MAX as u64) + 3 * heap_cost(2 * ETAG_MAX as u64))
        + 64 * 1024)
}
/// One per controller transfer slot, prepared at startup and reused. This is
/// batch 5.3's separate 2 MiB `C_net` term, not its connection's 4 MiB permit.
pub struct PartBudget(Account);
impl PartBudget {
    #[must_use]
    pub fn new() -> Self {
        Self(Account::new(2 * 1024 * 1024))
    }
    #[must_use]
    pub fn used(&self) -> u64 {
        self.0.snapshot().input.used
    }
    #[cfg(feature = "test-hooks")]
    pub(crate) fn visit_meter(
        &self,
        visit: &mut impl FnMut(&std::sync::Arc<unityrs_core::limits::Meter>),
    ) {
        visit(self.0.meter());
    }
}
impl Default for PartBudget {
    fn default() -> Self {
        Self::new()
    }
}
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub key: &'a Path,
    pub endpoint: Option<&'a str>,
    pub bucket: &'a str,
}
/// The journal entry survives Drop. Only an acknowledged complete/abort
/// removes it; cancellation runs no network or filesystem cleanup.
pub struct Transfer<'a> {
    account: &'a Account,
    journal: &'a Journal,
    target: Target<'a>,
    id: String,
    parts: Buffer<Etag>,
    count: usize,
    remaining: u64,
    live: bool,
}
impl<'a> Transfer<'a> {
    /// Part-ID storage is charged before create to its separate `C_net` account.
    /// The connection permit and phase admission keep their existing bounds.
    /// Journal mutations are synchronous durability boundaries, outside tries.
    pub async fn start(
        request: &Request<'_>,
        account: &'a Account,
        part_budget: &PartBudget,
        journal: &'a Journal,
        target: Target<'a>,
        size: u64,
    ) -> Result<Self, Error> {
        let count = part_count(size)?;
        if count < 2 {
            return Err(Error::Fault(Fault::Internal));
        }
        if !account.admits(reservation(size)?) {
            return Err(crate::phase::Error::internal().into());
        }
        let parts = Buffer::new(part_budget.0.meter(), count)?;
        let done = request
            .run(Kind::Create, |_| async {
                request
                    .store
                    .create_multipart(target.key)
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await?;
        let id = put::success(done)?;
        if id.len() > UPLOAD_ID_MAX {
            return Err(cleanup_failure(
                Error::PublishFailed,
                abort_once(request, target.key, &id).await,
            ));
        }
        let transfer = Self {
            account,
            journal,
            target,
            id,
            parts,
            count,
            remaining: size,
            live: true,
        };
        if let Err(error) = journal.append(transfer.entry()) {
            return Err(cleanup_failure(
                crate::phase::Error::from(error).into(),
                abort_once(request, target.key, &transfer.id).await,
            ));
        }
        Ok(transfer)
    }
    fn entry(&self) -> Entry<'_> {
        Entry {
            endpoint: self.target.endpoint,
            bucket: self.target.bucket,
            key: self.target.key.as_ref(),
            upload_id: &self.id,
        }
    }
    /// Populate an already-created transfer without uploading its payloads.
    /// Allocation gates still exercise the real completion and journal paths.
    #[cfg(feature = "test-hooks")]
    pub fn preload_parts_for_test(&mut self, etag: Etag) {
        self.parts.fill(etag);
        self.remaining = 0;
    }
    pub async fn part(&mut self, request: &Request<'_>, payload: Payload) -> Result<(), Error> {
        let bytes = payload.into_bytes();
        let result = async {
            let expected = self.remaining.min(PART_SIZE as u64);
            if !self.live
                || self.parts.len() >= self.count
                || expected == 0
                || bytes.len() as u64 != expected
            {
                return Err(Error::Fault(Fault::Internal));
            }
            let done = request
                .run(Kind::Part, |_| {
                    let bytes = bytes.clone();
                    let (key, id, index) = (self.target.key, &self.id, self.parts.len());
                    async move {
                        let part = request
                            .store
                            .put_part(key, id, index, bytes.into())
                            .await
                            .map_err(|_| Ended::Error)?;
                        put::tag(Some(&part.content_id))
                    }
                })
                .await?;
            let part = put::success(done)?;
            self.parts.push(part)?;
            self.remaining -= expected;
            Ok(())
        }
        .await;
        drop(bytes);
        match result {
            Ok(()) => Ok(()),
            Err(error) => Err(cleanup_failure(error, self.abort(request).await)),
        }
    }
    pub async fn complete(
        mut self,
        request: &Request<'_>,
        expected: &[u8],
    ) -> Result<Written, Error> {
        let result = async {
            if !self.live || self.remaining != 0 || self.parts.len() != self.count {
                return Err(Error::Fault(Fault::Internal));
            }
            // The payload has gone; charge the derived completion peak before
            // copying IDs or asking the dependency to construct its XML body.
            let _complete = self
                .account
                .meter()
                .charge(completion_reservation(self.count)?)
                .map_err(crate::phase::Error::from)?;
            let done = request
                .run(Kind::Complete, |_| async {
                    let parts = copy_parts(&self.parts).map_err(|fault| {
                        request.slot.fail(fault);
                        Ended::Error
                    })?;
                    let result = request
                        .store
                        .complete_multipart(self.target.key, &self.id, parts)
                        .await
                        .map_err(|_| Ended::Error)?;
                    put::tag(result.e_tag.as_deref())
                })
                .await?;
            if put::action(&done) == Some(Action::HeadCheck) {
                let head = request
                    .run(Kind::Head, |_| async {
                        let meta = request
                            .store
                            .head(self.target.key)
                            .await
                            .map_err(|_| Ended::Error)?;
                        put::tag(meta.e_tag.as_deref())
                    })
                    .await?;
                let etag = put::success(head).map_err(|error| match error {
                    Error::Fault(_) | Error::Phase(_) | Error::Stopped => error,
                    _ => Error::PublishFailed,
                })?;
                if !etag.matches(expected) {
                    return Err(Error::PublishFailed);
                }
                Ok(Written {
                    etag,
                    verify_by_size: false,
                })
            } else {
                let etag = put::success(done)?;
                Ok(Written {
                    etag,
                    verify_by_size: !etag.matches(expected),
                })
            }
        }
        .await;
        match result {
            Ok(written) => {
                self.live = false;
                self.journal
                    .remove(self.entry())
                    .map_err(crate::phase::Error::from)?;
                Ok(written)
            }
            Err(error) => Err(cleanup_failure(error, self.abort(request).await)),
        }
    }
    /// Failed aborts keep the durable entry. `NoSuchUpload` is success.
    pub async fn abort(&mut self, request: &Request<'_>) -> Result<(), Error> {
        if !self.live {
            return Ok(());
        }
        abort::upload(request, self.target.key, &self.id).await?;
        self.live = false;
        self.journal
            .remove(self.entry())
            .map_err(crate::phase::Error::from)?;
        Ok(())
    }
}
/// Remote abort failures are best effort; a run failure is never hidden by
/// a record failure. Among run failures keep the spec's severity precedence.
fn cleanup_failure(original: Error, cleanup: Result<(), Error>) -> Error {
    fn run(error: Error) -> Option<crate::job::RunFailure> {
        use crate::{attempt::ReasonCode, job::RunFailure, phase};
        match error {
            Error::Fault(Fault::Internal) => Some(RunFailure::new(ReasonCode::Internal)),
            Error::Fault(Fault::Memory) => Some(RunFailure::new(ReasonCode::HostMemory)),
            Error::Phase(phase::Error::Run(failure)) => Some(failure),
            _ => None,
        }
    }
    let Err(cleanup) = cleanup else {
        return original;
    };
    match (run(original), run(cleanup)) {
        (None, Some(_)) => cleanup,
        (Some(old), Some(new)) if new.code().severity() < old.code().severity() => cleanup,
        _ => original,
    }
}
async fn abort_once(request: &Request<'_>, key: &Path, id: &String) -> Result<(), Error> {
    let mut policy = request.tries.policy;
    policy.attempts = 1;
    let tries = Tries {
        policy,
        control: request.tries.control,
        unsettled: request.tries.unsettled,
    };
    let request = Request::new(request.store, &tries, request.slot, request.deadline);
    abort::upload(&request, key, id).await
}
fn copy_parts(parts: &[Etag]) -> Result<Vec<PartId>, Fault> {
    let mut ids = Vec::new();
    ids.try_reserve_exact(parts.len())
        .map_err(|_| Fault::Memory)?;
    for etag in parts {
        let text = std::str::from_utf8(etag.as_bytes()).map_err(|_| Fault::Internal)?;
        let mut content_id = String::new();
        content_id
            .try_reserve_exact(text.len())
            .map_err(|_| Fault::Memory)?;
        content_id.push_str(text);
        ids.push(PartId { content_id });
    }
    Ok(ids)
}

#[cfg(test)]
#[path = "multipart_tests.rs"]
mod tests;
