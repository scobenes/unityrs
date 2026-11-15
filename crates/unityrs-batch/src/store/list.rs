//! One `ListObjectsV2` page at a time (spec 1.6; batch 2.9). The page owns
//! its phase reservation until its last consumer drops it. No prefetch.
use super::{
    Kind,
    classify::Ended,
    put::{self, Request},
};
use crate::phase::Account;
use object_store::{
    ObjectMeta,
    list::{PaginatedListOptions, PaginatedListStore},
};
use std::cell::Cell;
use unityrs_core::limits::Charge;

pub const RESERVATION: u64 = 40 * 1024 * 1024;
const TEXT_MAX: usize = 1024;

/// Closed vocabulary: server errors and strings are never rendered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Retry only after another phase owner releases capacity. No charge tried.
    Busy,
    /// Even an otherwise idle phase cannot hold a page: fail this LIST.
    Reservation,
    PageHeld,
    Failed,
    Order,
    Stopped,
    Fault(super::attempt::Fault),
    Phase(crate::phase::Error),
}
impl From<put::Error> for Error {
    fn from(error: put::Error) -> Self {
        match error {
            put::Error::Stopped => Self::Stopped,
            put::Error::Fault(f) => Self::Fault(f),
            put::Error::Phase(e) => Self::Phase(e),
            _ => Self::Failed,
        }
    }
}
#[derive(Clone, Copy)]
struct Text {
    bytes: [u8; TEXT_MAX],
    len: usize,
}
impl Text {
    fn new(text: &str) -> Option<Self> {
        let mut out = Self {
            bytes: [0; TEXT_MAX],
            len: text.len(),
        };
        out.bytes
            .get_mut(..text.len())?
            .copy_from_slice(text.as_bytes());
        Some(out)
    }
    fn text(&self) -> Result<&str, Error> {
        self.bytes
            .get(..self.len)
            .and_then(|b| std::str::from_utf8(b).ok())
            .ok_or(Error::Fault(super::attempt::Fault::Internal))
    }
}
/// A page's dependency-owned storage is prepaid by the parse envelope.
pub struct Page<'a> {
    objects: Vec<ObjectMeta>,
    _charge: Charge,
    _lease: Lease<'a>,
}
impl Page<'_> {
    #[must_use]
    pub fn entries(&self) -> &[ObjectMeta] {
        &self.objects
    }
}
struct Lease<'a>(&'a Cell<bool>);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
/// Fixed state, with no heap allocation. A borrowed lease rejects overlapping
/// pages on the controller's current-thread runtime and survives cancellation.
pub struct Listing {
    prefix: Text,
    position: Cell<Position>,
    held: Cell<bool>,
}
#[derive(Clone, Copy)]
struct Position {
    token: Option<Text>,
    previous: Option<Text>,
    ended: bool,
    failed: bool,
}
impl Listing {
    pub fn new(prefix: &str) -> Result<Self, Error> {
        Ok(Self {
            prefix: Text::new(prefix).ok_or(Error::Failed)?,
            position: Cell::new(Position {
                token: None,
                previous: None,
                ended: false,
                failed: false,
            }),
            held: Cell::new(false),
        })
    }
    pub async fn page(
        &self,
        request: &Request<'_>,
        account: &Account,
    ) -> Result<Option<Page<'_>>, Error> {
        if self.held.get() {
            return Err(Error::PageHeld);
        }
        let mut position = self.position.get();
        if position.failed {
            return Err(Error::Failed);
        }
        if request.tries.control.stopped() {
            return Err(Error::Stopped);
        }
        if position.ended {
            return Ok(None);
        }
        if !account.admits(RESERVATION) {
            return Err(if account.snapshot().input.used == 0 {
                Error::Reservation
            } else {
                Error::Busy
            });
        }
        let charge = account
            .meter()
            .charge(RESERVATION)
            .map_err(|e| Error::Phase(e.into()))?;
        let prefix = self.prefix.text()?;
        let token = position.token.as_ref().map(Text::text).transpose()?;
        self.held.set(true);
        let lease = Lease(&self.held);
        let done = request
            .run(Kind::ListPage, |_| async {
                let mut page_token = None;
                if let Some(token) = token {
                    let mut copy = String::new();
                    copy.try_reserve_exact(token.len()).map_err(|_| {
                        request.slot.fail(super::attempt::Fault::Memory);
                        Ended::Rejected
                    })?;
                    copy.push_str(token);
                    page_token = Some(copy);
                }
                let page = request
                    .store
                    .list_paginated(
                        Some(prefix),
                        PaginatedListOptions {
                            max_keys: Some(1000),
                            page_token,
                            ..PaginatedListOptions::default()
                        },
                    )
                    .await
                    .map_err(|_| Ended::Error)?;
                // Reject kept fields inside the try so a 2xx uses 1.6's retry row.
                if request.slot.record().list_truncated.unwrap_or(false)
                    != page.page_token.is_some()
                    || page.page_token.as_ref().is_some_and(|t| {
                        t.is_empty() || t.len() > TEXT_MAX || Some(t.as_str()) == token
                    })
                    || page.result.objects.iter().any(|o| {
                        o.location.as_ref().len() > TEXT_MAX
                            || (!o.location.as_ref().starts_with(prefix)
                                && o.location.as_ref() != prefix.trim_end_matches('/'))
                            || o.e_tag
                                .as_ref()
                                .is_none_or(|t| t.len() > super::etag::ETAG_MAX)
                    })
                {
                    return Err(Ended::Rejected);
                }
                Ok(page)
            })
            .await;
        let result = done.and_then(put::success).map_err(Error::from);
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                position.failed = e != Error::Stopped;
                self.position.set(position);
                return Err(e);
            }
        };
        if request.tries.control.stopped() {
            return Err(Error::Stopped);
        }
        let mut objects = page.result.objects;
        if let Err(e) = self.ordered(&mut position, &mut objects) {
            position.failed = true;
            self.position.set(position);
            return Err(e);
        }
        position.token = page.page_token.as_deref().and_then(Text::new);
        position.ended = position.token.is_none();
        self.position.set(position);
        Ok(Some(Page {
            objects,
            _charge: charge,
            _lease: lease,
        }))
    }
    fn ordered(&self, position: &mut Position, objects: &mut Vec<ObjectMeta>) -> Result<(), Error> {
        let prefix = self.prefix;
        let prefix = prefix.text()?;
        let mut bad = false;
        objects.retain(|object| {
            let key = object.location.as_ref();
            if object.size == 0 && key == prefix.trim_end_matches('/') {
                return false;
            }
            if position
                .previous
                .as_ref()
                .is_some_and(|p| p.text().is_ok_and(|p| key <= p))
            {
                // Path::parse normalizes k/ to k. Only zero-size regressions
                // are folder markers; never let them move the order watermark.
                if object.size == 0 {
                    return false;
                }
                bad = true;
            }
            position.previous = Text::new(key);
            true
        });
        if bad {
            return Err(Error::Order);
        }
        Ok(())
    }
}
