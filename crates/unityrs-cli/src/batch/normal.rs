//! Temporary local normal runs; B9 supplies lifetime ownership.
use super::{CHECK_ENV_BLOCK, PROVISIONAL, PreparedRun, alloc, code, early, memory_bound};
use std::{fs::File, os::fd::AsFd, process::ExitCode, sync::Arc, time::Duration};
use unityrs_batch::{
    progress::{self, Exit, Phase, Snapshot, clock::Clock, live::Service},
    reason::Code,
    run,
};

pub(super) fn run(clock: &Clock, prepared: PreparedRun, measured: alloc::Measured) -> ExitCode {
    if unityrs_batch::start::supported(&prepared.spec).is_err() {
        return super::refused(
            clock,
            &super::BadSpec::usage(),
            "unityrs batch",
            &format!("{}\n", super::USAGE),
        );
    }
    // Provisional until B12 measures the inherited environment block: normal
    // runs currently use --check's conservative 6 MiB allowance as well.
    let bound = memory_bound(&prepared.spec, measured, CHECK_ENV_BLOCK, &PROVISIONAL).total();
    if let Err(error) = run::host_check(&prepared, bound, &unityrs_batch::start::host::System) {
        return failed(clock, error);
    }
    let file = match std::io::stdout().as_fd().try_clone_to_owned() {
        Ok(fd) => File::from(fd),
        Err(_) => return failed(clock, error(Code::Host, "host_io")),
    };
    let mut snapshot = Snapshot::validate(clock.ms());
    snapshot.phase = Phase::Preflight;
    snapshot.inputs.total = prepared.inputs.len() as u64;
    snapshot.inputs.queued = snapshot.inputs.total;
    snapshot.memory.inflight_limit = prepared.spec.max_inflight_uncompressed_bytes;
    snapshot.spool.size = prepared.spec.spool.size_bytes;
    let service = match Service::start(file, snapshot) {
        Ok(service) => service,
        Err(source) => {
            let failure = if source.kind() == std::io::ErrorKind::OutOfMemory {
                error(Code::Host, "host_memory")
            } else {
                error(Code::CouldNotStart, "thread_spawn_failed")
            };
            return failed(clock, failure);
        }
    };
    let result = run::drive(
        prepared,
        bound,
        &service.sender(),
        &unityrs_batch::start::spool::Host,
        Arc::new(unityrs_batch::sink::fs::Host),
    );
    let exit = terminal(result);
    let end_code = exit.code;
    let mut snapshot = service.sender().snapshot().unwrap_or(snapshot);
    snapshot.ms = clock.ms();
    let _ = service.exit(snapshot, exit, Duration::from_secs(1));
    code(end_code)
}
fn terminal(result: Result<unityrs_batch::finish::ResultEnd, run::Error>) -> Exit {
    match result {
        Ok(end) => Exit {
            status: end.status,
            code: end.code,
            reason: end.reason,
            detail: None,
        },
        Err(error) => Exit {
            status: error.status,
            code: error.code,
            reason: Some(error.reason),
            detail: error.detail,
        },
    }
}
fn error(code: Code, reason: &'static str) -> run::Error {
    run::Error {
        code,
        reason,
        detail: None,
        status: None,
    }
}
fn failed(clock: &Clock, error: run::Error) -> ExitCode {
    let mut snapshot = Snapshot::validate(clock.ms());
    snapshot.phase = Phase::Preflight;
    snapshot.memory.rss = progress::rss();
    let exit = Exit {
        status: error.status,
        code: error.code,
        reason: Some(error.reason),
        detail: error.detail,
    };
    early::write(snapshot.line(Some(&exit)).as_bytes(), b"");
    code(error.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_error_moves_detail_and_keeps_the_acknowledged_status() {
        let error = run::Error {
            status: Some("running"),
            code: Code::Host,
            reason: "host_io",
            detail: Some("fsync EIO".to_owned()),
        };
        crate::test_alloc::REFUSE.with(|r| r.set(true));
        let exit = terminal(Err(error));
        let still_armed = crate::test_alloc::REFUSE.with(|r| r.replace(false));
        assert!(still_armed);
        assert_eq!(exit.status, Some("running"));
        assert_eq!(exit.reason, Some("host_io"));
        assert_eq!(exit.detail.as_deref(), Some("fsync EIO"));
        assert_eq!(exit.code.value(), 74);
    }
    #[test]
    fn final_head_reason_survives_an_allocation_refusal() {
        let end = unityrs_batch::finish::ResultEnd {
            code: Code::Incomplete,
            reason: Some("publish_failed"),
            status: Some("running"),
        };
        // Refuse the allocation that used to copy publish_failed. Borrowing
        // the closed reason set must leave this refusal armed.
        crate::test_alloc::REFUSE.with(|r| r.set(true));
        let exit = terminal(Ok(end));
        let still_armed = crate::test_alloc::REFUSE.with(|r| r.replace(false));
        assert!(still_armed, "static terminal reasons must not allocate");
        assert_eq!(exit.status, Some("running"));
        assert_eq!(exit.reason, Some("publish_failed"));
        assert_eq!(exit.code.value(), 4);
    }
}
