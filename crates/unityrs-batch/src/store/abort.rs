//! Noncommitting aborts and spool-start recovery (spec 1.6, 6.2 step 4).
//! Recovery is serial, hence holds one identity and stays below every positive
//! `download_concurrency`. B11d owns enumeration/scheduling of old run journals.
use super::{
    Kind,
    classify::{Action, Ended},
    put::{self, Error, Request},
};
use crate::spool::journal::Journal;
use object_store::{MultipartId, multipart::MultipartStore, path::Path};

/// `NoSuchUpload` is an acknowledged abort, even if the client returned an error.
pub async fn upload(request: &Request<'_>, key: &Path, id: &MultipartId) -> Result<(), Error> {
    let done = request
        .run(Kind::Abort, |_| async {
            request
                .store
                .abort_multipart(key, id)
                .await
                .map_err(|_| Ended::Error)
        })
        .await?;
    match put::action(&done) {
        Some(Action::Success) => Ok(()),
        _ if done.stopped => Err(Error::Stopped),
        _ => Err(Error::PublishFailed),
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub aborted: u64,
    pub failed: u64,
}
/// Never returns an error to startup. Another endpoint/bucket is ignored;
/// malformed lines are skipped by the policy-free journal reader. Failed
/// requests/removals leave their line intact and contribute `abort_failed`.
pub async fn recover_journal(
    request: &Request<'_>,
    journal: &Journal,
    endpoint: Option<&str>,
    bucket: &str,
) -> Report {
    let mut report = Report::default();
    let Ok(mut reader) = journal.reader() else {
        report.failed = 1;
        return report;
    };
    loop {
        let record = match reader.next() {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(_) => {
                report.failed = report.failed.saturating_add(1);
                break;
            }
        };
        let entry = record.entry();
        if entry.endpoint != endpoint || entry.bucket != bucket {
            continue;
        }
        let result = async {
            let key = Path::parse(entry.key).map_err(|_| Error::PublishFailed)?;
            let mut id = String::new();
            id.try_reserve_exact(entry.upload_id.len())
                .map_err(|_| Error::Fault(super::attempt::Fault::Memory))?;
            id.push_str(entry.upload_id);
            upload(request, &key, &id).await
        }
        .await;
        if result.is_ok() && journal.remove(entry).is_ok() {
            report.aborted = report.aborted.saturating_add(1);
        } else {
            report.failed = report.failed.saturating_add(1);
        }
    }
    report
}
