//! Startup recovery and B9's journal backend. Readers hold one fixed record;
//! network aborts never run under the journal's mutation mutex.
use super::Store;
use crate::{
    finish::budget::{Backend, Close},
    spool::journal::{Journal, Reader, Record},
    store::{abort, attempt::Tries, put::Request},
};
use std::{fs::File, path::Path, sync::atomic::AtomicBool, time::Duration};
impl Store {
    pub(crate) fn recover(
        &self,
        root: &File,
        canonical: &Path,
        runtime: &tokio::runtime::Runtime,
        control: &dyn crate::control::Control,
    ) {
        let result = (|| -> Result<(), crate::phase::Error> {
            let runs = crate::sys::rooted_open(
                root,
                c"runs",
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                false,
            )?;
            let base = crate::spool::path::join(canonical, Path::new("runs"))?;
            crate::sys::visit_directory(&runs, &mut |name| {
                let Some(name_text) = name
                    .to_str()
                    .ok()
                    .filter(|n| crate::sink::path::valid_run(n).is_ok())
                else {
                    return Ok(());
                };
                let Ok(held) = crate::sys::rooted_open(
                    &runs,
                    name,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                    false,
                ) else {
                    return Ok(());
                };
                let result = (|| -> Result<(), crate::phase::Error> {
                    let path = crate::spool::path::join(&base, Path::new(name_text))?;
                    let journal = Journal::new(&path)?;
                    let tries = self.tries(control);
                    let request =
                        Request::new(&self.clients[1], &tries, self.slot(0)?, self.deadline());
                    let report = runtime.block_on(abort::recover_journal(
                        &request,
                        &journal,
                        self.settings().endpoint.as_deref(),
                        &self.settings().bucket,
                    ));
                    self.figures.lock().abort_failed += report.failed;
                    Ok(())
                })();
                drop(held);
                if result.is_err() {
                    self.figures.lock().abort_failed += 1;
                }
                Ok(())
            })?;
            Ok(())
        })();
        if result.is_err() {
            self.figures.lock().abort_failed += 1;
        }
    }
}
pub(crate) struct JournalBackend<'a> {
    store: &'a Store,
    reader: Option<Reader>,
    current: Option<Record>,
    failed: bool,
    budget: crate::finish::budget::Budget<'a>,
    failure: Option<crate::job::RunFailure>,
}
impl<'a> JournalBackend<'a> {
    pub(crate) fn new(store: &'a Store, budget: crate::finish::budget::Budget<'a>) -> Self {
        Self {
            store,
            reader: None,
            current: None,
            failed: false,
            budget,
            failure: None,
        }
    }
}
impl Backend for JournalBackend<'_> {
    fn failure(&mut self) -> Option<crate::job::RunFailure> {
        self.failure.take()
    }
    fn try_close(&mut self) -> Close {
        let journal = self.store.journal.lock();
        let Some(journal) = journal.as_ref() else {
            return Close::Absent;
        };
        if !journal.try_close() {
            return Close::Busy;
        }
        match memory_retry(&mut self.failure, || {
            crate::phase::on_pool_named("finisher journal reader", || {
                journal.reader().map_err(Into::into)
            })
        }) {
            Ok(reader) => self.reader = Some(reader),
            Err(crate::phase::Error::Run(f)) => {
                crate::job::RunFailure::reduce(&mut self.failure, f);
                self.failed = true;
            }
            Err(crate::phase::Error::Stopped) => self.failed = true,
        }
        Close::Closed
    }
    fn next_upload(&mut self) -> Option<u64> {
        if self.failed {
            self.failed = false;
            return Some(u64::MAX);
        }
        loop {
            let reader = self.reader.as_mut()?;
            let result = crate::phase::on_pool_named("finisher journal read", || {
                reader.next().map_err(Into::into)
            });
            let record = if let Ok(record) = result {
                record?
            } else {
                if let Err(crate::phase::Error::Run(f)) = result {
                    crate::job::RunFailure::reduce(&mut self.failure, f);
                }
                self.reader = None;
                return Some(u64::MAX);
            };
            let entry = record.entry();
            if entry.endpoint == self.store.settings().endpoint.as_deref()
                && entry.bucket == self.store.settings().bucket
            {
                self.current = Some(record);
                return Some(0);
            }
        }
    }
    fn abort(&mut self, upload: u64, timeout: Duration) -> bool {
        if upload == u64::MAX {
            return false;
        }
        let Some(record) = self.current.take() else {
            return false;
        };
        let deadline = tokio::time::Instant::now() + timeout;
        let result = memory_retry(&mut self.failure, || {
            crate::phase::on_pool_named("finisher abort", || {
                let entry = record.entry();
                let key = object_store::path::Path::parse(entry.key)
                    .map_err(|_| crate::phase::Error::internal())?;
                let mut id = String::new();
                id.try_reserve_exact(entry.upload_id.len())
                    .map_err(|_| crate::phase::Error::memory())?;
                id.push_str(entry.upload_id);
                let control = AtomicBool::new(false);
                let mut policy = self.store.policy;
                policy.attempts = 1;
                policy.timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
                if policy.timeout.is_zero() {
                    return Err(crate::phase::Error::Stopped);
                }
                policy.cutoff = None;
                let tries = Tries {
                    policy,
                    control: &control,
                    unsettled: &self.store.unsettled,
                };
                let request = Request::new(
                    &self.store.finisher,
                    &tries,
                    self.store.fin_slot().map_err(operation)?,
                    deadline,
                )
                .finishing(self.budget, false);
                self.store
                    .runtime
                    .get()
                    .ok_or_else(crate::phase::Error::internal)?
                    .block_on(abort::upload(&request, &key, &id))
                    .map_err(operation)?;
                self.store
                    .journal
                    .lock()
                    .as_ref()
                    .ok_or_else(crate::phase::Error::internal)?
                    .remove_closed(entry)?;
                Ok(())
            })
        });
        if let Err(crate::phase::Error::Run(f)) = result {
            crate::job::RunFailure::reduce(&mut self.failure, f);
        }
        result.is_ok()
    }
}
/// A transient allocator refusal must still decide the run as `host_memory`,
/// while one bounded cleanup retry can release an acknowledged upload and
/// its durable identity. Repeated refusal leaves recovery's journal intact.
/// Network tries remain subject to the finisher's original budget.
fn memory_retry<T>(
    failure: &mut Option<crate::job::RunFailure>,
    mut body: impl FnMut() -> Result<T, crate::phase::Error>,
) -> Result<T, crate::phase::Error> {
    let result = body();
    if let Err(crate::phase::Error::Run(f)) = result {
        crate::job::RunFailure::reduce(failure, f);
        if f.name() == "host_memory" {
            return body();
        }
    }
    result
}
fn operation(error: crate::store::put::Error) -> crate::phase::Error {
    use crate::store::put::Error;
    match error {
        Error::Fault(f) => super::errors::fault(f),
        Error::Phase(e) => e,
        _ => crate::phase::Error::Stopped,
    }
}
