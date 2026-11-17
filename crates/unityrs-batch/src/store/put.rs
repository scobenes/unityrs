//! Single PUT and the operations' deadline boundary (spec 1.6; batch 2.9).
//! Preparation runs on the caller's blocking pool before the first timed try.
use super::{
    Kind,
    attempt::{Done, Fault, Observer, Tries, TrySlot, Unobserved},
    classify::{Action, Ended},
    client::Store,
    etag::Etag,
};
use crate::phase::{Account, Buffer};
use hyper::body::Bytes;
use object_store::{
    Attribute, AttributeValue, Attributes, ObjectStore, PutOptions, PutPayload, path::Path,
};
use std::{future::Future, io::Read, sync::Arc};
use tokio::time::Instant;

pub const PART_SIZE: usize = 16 * 1024 * 1024;

/// Closed failures: no third-party error, URL, body, key or header renders.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Fault(Fault),
    Phase(crate::phase::Error),
    Stopped,
    PublishFailed,
    CredentialsExpired,
    ArchiveTooLarge,
}
impl From<Fault> for Error {
    fn from(e: Fault) -> Self {
        Self::Fault(e)
    }
}
impl From<crate::phase::Error> for Error {
    fn from(e: crate::phase::Error) -> Self {
        Self::Phase(e)
    }
}

/// A single contiguous phase buffer, shared without copying across retries.
pub struct Payload(Buffer<u8>);
impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Payload {
    pub(crate) fn from_buffer(buffer: Buffer<u8>) -> Self {
        Self(buffer)
    }
    /// Charge before allocating or reading. The immutable spool length is known.
    /// The caller executes this synchronous operation on its blocking pool.
    pub fn read(account: &Account, size: usize, reader: &mut dyn Read) -> Result<Self, Error> {
        if size > PART_SIZE {
            return Err(Error::ArchiveTooLarge);
        }
        let mut buffer = Buffer::new(account.meter(), size)?;
        buffer.fill(0);
        reader
            .read_exact(&mut buffer)
            .map_err(crate::phase::Error::from)?;
        Ok(Self(buffer))
    }
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        Bytes::from_owner(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Written {
    pub etag: Etag,
    /// Signed bytes were accepted, but this store's tag differs from our digest.
    pub verify_by_size: bool,
}

/// One borrowed permit and try policy. Every operation requires a live hard
/// boundary, including aborts. Finisher requests read B9's live budget.
pub struct Request<'a> {
    pub store: &'a Store,
    pub tries: &'a Tries<'a>,
    pub slot: &'a Arc<TrySlot>,
    pub deadline: Instant,
    finish: Option<(crate::finish::budget::Budget<'a>, bool)>,
}
impl<'a> Request<'a> {
    #[must_use]
    pub const fn new(
        store: &'a Store,
        tries: &'a Tries<'a>,
        slot: &'a Arc<TrySlot>,
        deadline: Instant,
    ) -> Self {
        Self {
            store,
            tries,
            slot,
            deadline,
            finish: None,
        }
    }
    #[must_use]
    pub fn finishing(mut self, budget: crate::finish::budget::Budget<'a>, head: bool) -> Self {
        self.finish = Some((budget, head));
        self
    }
    pub(crate) fn reconcile_open(&self) -> bool {
        self.finish.is_none_or(|(budget, head)| {
            !head
                || budget.head_deadline().is_none_or(|cut| {
                    crate::lifetime::Boot::now()
                        .is_ok_and(|now| cut.0.saturating_sub(now.0) >= 1000)
                })
        })
    }
    /// Includes waits for quarantined lookups and backoff in the deadline.
    pub async fn run<T, F, Fut>(&self, kind: Kind, op: F) -> Result<Done<T>, Error>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<T, Ended>>,
    {
        self.run_observed(kind, &Unobserved, op).await
    }
    pub async fn run_observed<T, F, Fut>(
        &self,
        kind: Kind,
        observer: &dyn Observer,
        op: F,
    ) -> Result<Done<T>, Error>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<T, Ended>>,
    {
        if self.finish.is_none() && Instant::now() >= self.deadline {
            return Err(Error::Stopped);
        }
        let mut policy = self.tries.policy;
        if self.finish.is_some() {
            policy.cutoff = None;
        } else {
            policy.cutoff = Some(
                policy
                    .cutoff
                    .map_or(self.deadline, |t| t.min(self.deadline)),
            );
        }
        let tries = Tries {
            policy,
            control: self.tries.control,
            unsettled: self.tries.unsettled,
        };
        if let Some((budget, head)) = self.finish {
            let control = super::finishing::ControlAt::new(budget.hard);
            let tracking = super::finishing::Observed {
                control: &control,
                observer,
            };
            let tries = super::finishing::tries(&tries, &control);
            return super::finishing::drive(
                budget,
                head,
                tries.run_observed(self.slot, kind, &tracking, op),
            )
            .await?
            .map_err(Into::into);
        }
        tokio::time::timeout_at(
            self.deadline,
            tries.run_observed(self.slot, kind, observer, op),
        )
        .await
        .map_err(|_| Error::Stopped)?
        .map_err(Into::into)
    }
    pub async fn put(
        &self,
        path: &Path,
        payload: Payload,
        content_type: &'static str,
        expected: &[u8],
    ) -> Result<Written, Error> {
        self.bytes(path, payload.into_bytes(), content_type, expected)
            .await
    }
    /// `bytes` must already have an owning phase/finisher reservation. This
    /// borrowed-buffer seam also serves the finisher's existing chunk buffer.
    pub async fn bytes(
        &self,
        path: &Path,
        bytes: Bytes,
        content_type: &'static str,
        expected: &[u8],
    ) -> Result<Written, Error> {
        if bytes.len() > PART_SIZE {
            return Err(Error::ArchiveTooLarge);
        }
        let done = self
            .run(Kind::Put, |_| {
                let payload = PutPayload::from(bytes.clone());
                async move {
                    let result = self
                        .store
                        .put_opts(path, payload, options(content_type))
                        .await
                        .map_err(|_| Ended::Error)?;
                    // Output PUTs need no tag for fencing. Preserve its absence
                    // as an empty tag for outcome 7's size-only verification.
                    tag(Some(result.e_tag.as_deref().unwrap_or_default()))
                }
            })
            .await?;
        let etag = success(done)?;
        Ok(Written {
            etag,
            verify_by_size: !etag.matches(expected),
        })
    }
}
pub(crate) fn options(content_type: &'static str) -> PutOptions {
    let mut attributes = Attributes::new();
    attributes.insert(Attribute::ContentType, AttributeValue::from(content_type));
    PutOptions {
        attributes,
        ..PutOptions::default()
    }
}
pub(crate) fn tag(raw: Option<&str>) -> Result<Etag, Ended> {
    raw.and_then(|v| Etag::new(v.as_bytes()))
        .ok_or(Ended::Rejected)
}
pub(crate) fn action<T>(done: &Done<T>) -> Option<Action> {
    done.verdict.map(|v| v.action)
}
pub(crate) fn success<T>(done: Done<T>) -> Result<T, Error> {
    if done.stopped {
        return Err(Error::Stopped);
    }
    match action(&done) {
        Some(Action::Success) => done.value.ok_or(Error::PublishFailed),
        Some(Action::CredentialsExpired) => Err(Error::CredentialsExpired),
        _ => Err(Error::PublishFailed),
    }
}
