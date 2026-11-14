//! Normal-end ownership only. A stopped controller must preserve the running
//! head and bypass this module until B9 supplies lifetime finishing.
mod encode;
mod reduce;
use crate::{
    manifest,
    phase::{Account, Error},
    phase1::PreparedRun,
    reason::Code,
    sink::directory::Directory,
};

pub struct Identity<'a> {
    pub started_at: &'a str,
    pub memory_bound: u64,
    pub spool_peak: u64,
    pub vm_hwm: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultEnd {
    pub code: Code,
    /// Last acknowledged head, even when the final rename may have happened.
    pub status: Option<&'static str>,
    pub reason: Option<&'static str>,
}
pub fn running(
    prepared: &PreparedRun,
    directory: &Directory,
    account: &Account,
    identity: &Identity<'_>,
) -> Result<(), Error> {
    account.publication(|| {
        let mut json = encode::head(prepared, account, identity, None, None)?;
        directory.write_head(json.end()?, true).map_err(sink_error)
    })
}
pub fn normal(
    cx: &manifest::rows::Context<'_>,
    directory: &Directory,
    identity: &Identity<'_>,
    ended_at: &str,
) -> Result<ResultEnd, Error> {
    // Canonical stop was checked before acquiring Frozen. Later stops cannot
    // reinterpret this snapshot. The finisher's own budget is a separate query.
    let reduction = cx.account.publication(|| reduce::normal(cx));
    let mut code = reduction.unwrap_or(Code::Complete);
    let mut destination = manifest::chunks::Directory {
        directory,
        run_id: &cx.prepared.spec.run_id,
        attempt: cx.prepared.spec.attempt,
    };
    let mut report = manifest::finishing(cx, &mut destination);
    if let Err(error) = reduction {
        crate::job::RunFailure::reduce(
            &mut report.failure,
            match error {
                Error::Run(f) => f,
                Error::Stopped => crate::job::RunFailure::new(crate::attempt::ReasonCode::Internal),
            },
        );
    }
    if report.publication_failed {
        code = code.worst(Code::Incomplete);
    }
    if directory.syncfs().is_err() {
        report.sync_failed();
    }
    if let Some(failure) = report.failure {
        code = code.worst(failure.code());
    }
    let status = code.status();
    let reason = report.failure.map(crate::job::RunFailure::name);
    let published = cx.account.publication(|| {
        let mut json = encode::head(
            cx.prepared,
            cx.account,
            identity,
            Some((&report, code)),
            Some(ended_at),
        )?;
        directory.write_head(json.end()?, false).map_err(sink_error)
    });
    match published {
        Ok(()) => Ok(ResultEnd {
            code,
            status,
            reason,
        }),
        Err(error) => {
            let failure_code = match error {
                Error::Run(f) => f.code(),
                Error::Stopped => Code::Incomplete,
            };
            // Ordinary final-head filesystem failures contribute 4; allocation
            // and internal failures keep their stronger classification.
            let final_code = code.worst(Code::Incomplete).worst(failure_code);
            Ok(ResultEnd {
                code: final_code,
                status: Some("running"),
                reason: if final_code == Code::Incomplete
                    && matches!(code, Code::Complete | Code::CompleteWithSkips)
                {
                    Some("publish_failed")
                } else if failure_code.severity() < code.severity() {
                    match error {
                        Error::Run(f) => Some(f.name()),
                        Error::Stopped => None,
                    }
                } else {
                    reason
                },
            })
        }
    }
}
fn sink_error(e: crate::sink::Error) -> Error {
    match e.class {
        crate::sink::Class::PublishFailed => Error::Stopped,
        crate::sink::Class::HostMemory => Error::memory(),
        crate::sink::Class::InvalidPath => Error::internal(),
        _ => Error::Run(crate::job::RunFailure::new(
            crate::attempt::ReasonCode::HostIo,
        )),
    }
}
