//! Bucket proof, bounded identity reads and conditional fencing (spec 7.2,
//! 7.4; batch 2.9, 2.11). The caller keeps the last attempted precondition
//! across cancellation for the finisher; no run or finisher is wired here.
use super::{
    HEAD_CAP, Kind,
    classify::{Action, Ended},
    etag::Etag,
    put::{self, Request},
};
use crate::manifest::head::{Identity, recognize};
use futures_util::StreamExt;
use hyper::body::Bytes;
use object_store::{
    ObjectStore, ObjectStoreExt, PutMode, PutPayload, UpdateVersion,
    list::{PaginatedListOptions, PaginatedListStore},
    path::Path,
};
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Operation(put::Error),
    BucketMissing,
    SinkUnreachable,
    ManifestUnreadable,
    PreviousAttemptLive,
    Superseded,
    PublishFailed,
}
impl From<put::Error> for Error {
    fn from(e: put::Error) -> Self {
        Self::Operation(e)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Head {
    pub identity: Identity,
    pub etag: Etag,
}

/// A LIST of one whose connector drains the body, without scanning/parsing it.
/// The client's parse may fail on the empty body; only the status proves it.
pub async fn prove(request: &Request<'_>, prefix: &str) -> Result<(), Error> {
    let done = request
        .run(Kind::ListProof, |_| async {
            request
                .store
                .list_paginated(
                    Some(prefix),
                    PaginatedListOptions {
                        max_keys: Some(1),
                        ..Default::default()
                    },
                )
                .await
                .map(|_| ())
                .map_err(|_| Ended::Error)
        })
        .await?;
    if put::action(&done) == Some(Action::Success) {
        Ok(())
    } else if done.record.status == Some(404) {
        Err(Error::BucketMissing)
    } else {
        Err(Error::SinkUnreachable)
    }
}

/// `buffer` is the caller's already charged `C_base`/`C_fin` 1 MiB+1 buffer.
/// Bytes at the cap mean unreadable, including a capped longer response.
pub async fn read(
    request: &Request<'_>,
    key: &Path,
    run_id: &str,
    buffer: &mut [u8],
) -> Result<Option<Head>, Error> {
    let cap = usize::try_from(HEAD_CAP)
        .map_err(|_| put::Error::Fault(super::attempt::Fault::Internal))?;
    if buffer.len() < cap {
        return Err(put::Error::Fault(super::attempt::Fault::Internal).into());
    }
    let buffer = Mutex::new(buffer);
    let done = request
        .run(Kind::HeadRead, |_| async {
            let result = request.store.get(key).await.map_err(|_| Ended::Error)?;
            let etag = put::tag(result.meta.e_tag.as_deref())?;
            let mut stream = result.into_stream();
            let mut len = 0;
            while let Some(piece) = stream.next().await {
                let piece = piece.map_err(|_| Ended::Error)?;
                let count = piece.len().min(cap - len);
                buffer
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_mut(len..len + count)
                    .ok_or(Ended::Rejected)?
                    .copy_from_slice(piece.get(..count).ok_or(Ended::Rejected)?);
                len += count;
                if len == cap {
                    break;
                }
            }
            let buffer = buffer.lock().unwrap_or_else(PoisonError::into_inner);
            Ok(recognize(buffer.get(..len).ok_or(Ended::Rejected)?, run_id)
                .map(|identity| Head { identity, etag }))
        })
        .await?;
    if put::action(&done) == Some(Action::NoHead) {
        return Ok(None);
    }
    match put::success(done) {
        Ok(Ok(head)) => Ok(Some(head)),
        Ok(Err(_)) => Err(Error::ManifestUnreadable),
        Err(put::Error::Stopped) => Err(put::Error::Stopped.into()),
        Err(_) => Err(Error::SinkUnreachable),
    }
}

pub async fn preflight(
    request: &Request<'_>,
    prefix: &str,
    key: &Path,
    run_id: &str,
    attempt: u32,
    buffer: &mut [u8],
) -> Result<Option<Head>, Error> {
    prove(request, prefix).await?;
    let head = read(request, key, run_id, buffer).await?;
    if head.is_some_and(|h| h.identity.attempt >= attempt) {
        Err(Error::Superseded)
    } else {
        Ok(head)
    }
}

pub use super::head_fence::{Fence, History};
fn mode(etag: Option<Etag>) -> Result<PutMode, put::Error> {
    match etag {
        None => Ok(PutMode::Create),
        Some(etag) => {
            let raw =
                std::str::from_utf8(etag.as_bytes()).map_err(|_| put::Error::PublishFailed)?;
            let mut text = String::new();
            text.try_reserve_exact(raw.len())
                .map_err(|_| put::Error::Fault(super::attempt::Fault::Memory))?;
            text.push_str(raw);
            Ok(PutMode::Update(UpdateVersion {
                e_tag: Some(text),
                version: None,
            }))
        }
    }
}
/// A fixed write's identity and payload, already encoded and charged.
pub struct Write<'a> {
    pub key: &'a Path,
    pub run_id: &'a str,
    pub identity: Identity,
    pub bytes: Bytes,
    pub conditional: bool,
}
impl Write<'_> {
    fn failure(&self, running: Error) -> Error {
        if self.identity.write_seq == 0 {
            running
        } else {
            Error::PublishFailed
        }
    }
    /// Four read-back rewrites after the initial write, each with the same
    /// bytes and `write_seq`. All reads and retries share the live deadline.
    pub async fn publish(
        &self,
        request: &Request<'_>,
        fence: &Fence,
        buffer: &mut [u8],
    ) -> Result<Etag, Error> {
        let mut next = fence.last();
        for rewrite in 0..=4 {
            let precondition = next;
            let observer = fence.observer(precondition);
            let kind = Kind::HeadWrite {
                if_match: self.conditional && precondition.is_some(),
            };
            let done = request
                .run_observed(kind, &observer, |_| {
                    let bytes = self.bytes.clone();
                    async move {
                        let mut options = put::options("application/json");
                        options.mode = if self.conditional {
                            mode(precondition).map_err(|error| {
                                if let put::Error::Fault(fault) = error {
                                    request.slot.fail(fault);
                                }
                                Ended::Error
                            })?
                        } else {
                            PutMode::Overwrite
                        };
                        let result = request
                            .store
                            .put_opts(self.key, PutPayload::from(bytes), options)
                            .await
                            .map_err(|_| Ended::Error)?;
                        put::tag(result.e_tag.as_deref())
                    }
                })
                .await?;
            match put::action(&done) {
                Some(Action::Success) => {
                    fence.acknowledge();
                    return put::success(done).map_err(Into::into);
                }
                Some(Action::ReadBack) if self.conditional => (),
                _ => return Err(self.failure(Error::SinkUnreachable)),
            }
            let head = match read(request, self.key, self.run_id, buffer).await {
                Ok(head) => head,
                Err(Error::ManifestUnreadable) => {
                    return Err(self.failure(Error::ManifestUnreadable));
                }
                Err(Error::Operation(error @ (put::Error::Fault(_) | put::Error::Phase(_)))) => {
                    return Err(error.into());
                }
                Err(_) => return Err(self.failure(Error::SinkUnreachable)),
            };
            if let Some(head) = head {
                if head.identity == self.identity {
                    fence.acknowledge();
                    return Ok(head.etag);
                }
                if head.identity.attempt > self.identity.attempt {
                    return Err(Error::Superseded);
                }
            }
            if rewrite == 4 {
                return Err(self.failure(Error::PreviousAttemptLive));
            }
            next = head.map(|h| h.etag);
        }
        Err(put::Error::Fault(super::attempt::Fault::Internal).into())
    }
}
