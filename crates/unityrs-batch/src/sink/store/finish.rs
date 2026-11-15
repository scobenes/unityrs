//! Synchronous finisher adapters. No runtime is entered on this thread
//! before these calls; all requests use the finisher's independent service.
use super::{Store, errors};
use crate::{
    finish::budget::{Budget, Rows},
    lifetime::Boot,
    manifest::{Family, WriteError, chunks::Destination},
    phase::{Account, Buffer, Error},
    store::{attempt::Tries, put::Request, rows},
};
use std::sync::atomic::AtomicBool;

pub(crate) struct Adapter<'a> {
    pub store: &'a Store,
    pub account: &'a Account,
    pub budget: Budget<'a>,
}
impl Store {
    fn fin_policy(&self, budget: Budget<'_>) -> crate::store::attempt::Policy {
        let mut policy = self.policy;
        policy.cutoff = None;
        if budget.hard.hard().is_some() {
            policy.attempts = 1;
        }
        policy
    }
    pub(crate) fn final_head(
        &self,
        bytes: &[u8],
        account: &Account,
        budget: Budget<'_>,
    ) -> Result<(), crate::sink::Error> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(errors::phase(Error::internal()));
        }
        let control = AtomicBool::new(false);
        let tries = Tries {
            policy: self.fin_policy(budget),
            control: &control,
            unsettled: &self.unsettled,
        };
        let request = Request::new(
            &self.finisher,
            &tries,
            self.fin_slot()
                .map_err(|e| errors::head(crate::store::head::Error::Operation(e)))?,
            deadline(budget.head_deadline(), self.policy.timeout),
        )
        .finishing(budget, true);
        self.runtime
            .get()
            .ok_or_else(|| errors::phase(Error::internal()))?
            .block_on(self.write(&request, bytes, false, account))
    }
}
fn deadline(boot: Option<Boot>, timeout: std::time::Duration) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    boot.map_or_else(
        || now + timeout.saturating_mul(50),
        |d| {
            now + std::time::Duration::from_millis(
                d.0.saturating_sub(Boot::now().map_or(u64::MAX, |n| n.0)),
            )
        },
    )
}
impl Adapter<'_> {
    fn rows(
        &self,
        until: Option<Boot>,
        f: impl FnOnce(&mut rows::Destination<'_>) -> Result<Option<Buffer<u8>>, WriteError>,
    ) -> Result<Option<Buffer<u8>>, WriteError> {
        let control = AtomicBool::new(false);
        let tries = Tries {
            policy: self.store.fin_policy(self.budget),
            control: &control,
            unsettled: &self.store.unsettled,
        };
        let request = Request::new(
            &self.store.finisher,
            &tries,
            self.store.fin_slot().map_err(crate::store::rows::map)?,
            deadline(until, self.store.policy.timeout),
        )
        .finishing(self.budget, false);
        f(&mut rows::Destination {
            runtime: self.store.runtime.get().ok_or_else(Error::internal)?,
            request,
            account: self.account,
            prefix: &self.store.settings().prefix,
            run_id: &self.store.spec.run_id,
            attempt: self.store.spec.attempt,
        })
    }
}
impl Rows for Adapter<'_> {
    fn put(
        &mut self,
        family: Family,
        chunk: u64,
        bytes: &[u8],
        until: Option<Boot>,
    ) -> Result<(), WriteError> {
        self.rows(until, |rows| {
            rows.publish(family, chunk, bytes).map(|()| None)
        })
        .map(drop)
    }
    fn put_owned(
        &mut self,
        family: Family,
        chunk: u64,
        bytes: Buffer<u8>,
        until: Option<Boot>,
    ) -> Result<Option<Buffer<u8>>, WriteError> {
        self.rows(until, |rows| rows.publish_owned(family, chunk, bytes))
    }
}
