//! Normal runs: phase 1's checks of the subset, then the process host
//! (`unityrs_batch::run::process`, `docs/plans/batch.md` 2.11 steps 3 to
//! 8), which never returns.
use super::{PreparedRun, SELECTED, alloc, memory_bound};
use std::process::ExitCode;
use unityrs_batch::{bound::Certification, progress::clock::Clock, run::process};
#[cfg(test)]
use unityrs_batch::{reason::Code, run};

/// A spec outside the subset is 78 `usage`, before any host step; any
/// other run is the host's.
pub(super) fn run(
    clock: &Clock,
    startup: process::Startup,
    prepared: PreparedRun,
    measured: alloc::Measured,
) -> ExitCode {
    #[cfg(feature = "test-hooks")]
    let store_hook = std::env::var_os("UNITYRS_TEST_STORE").is_some_and(|v| v == "1");
    #[cfg(not(feature = "test-hooks"))]
    let store_hook = false;
    if unityrs_batch::start::supported(&prepared.spec).is_err() && !store_hook {
        drop(startup);
        return super::refused(
            clock,
            &super::BadSpec::usage(),
            "unityrs batch",
            &format!("{}\n", super::USAGE),
        );
    }
    #[cfg(feature = "test-hooks")]
    crate::whole_memory::configure(prepared.spec.threads, prepared.spec.download_concurrency);
    let bound = memory_bound(
        &prepared.spec,
        measured,
        unityrs_batch::bound::execution_env_block(),
        &SELECTED,
    )
    .total();
    // Uncertified off 4 KiB pages (`unityrs_batch::bound`): the run says so
    // at its start, on stderr alone, before any progress thread, and goes on
    // (`docs/BATCH_SPEC.md` section 10). The host writes it as the main
    // thread's lines, within the deadline's bound.
    let note = Certification::of(unityrs_batch::sys::page_size()).note("unityrs batch");
    process::run(startup, *clock, prepared, bound, note.as_deref())
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
            detail: Some("fsync EIO".into()),
        };
        crate::test_alloc::REFUSE.with(|r| r.set(true));
        let exit = process::exit_of(Err(error));
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
            detail: None,
        };
        // Refuse the allocation that used to copy publish_failed. Borrowing
        // the closed reason set must leave this refusal armed.
        crate::test_alloc::REFUSE.with(|r| r.set(true));
        let exit = process::exit_of(Ok(end));
        let still_armed = crate::test_alloc::REFUSE.with(|r| r.replace(false));
        assert!(still_armed, "static terminal reasons must not allocate");
        assert_eq!(exit.status, Some("running"));
        assert_eq!(exit.reason, Some("publish_failed"));
        assert_eq!(exit.code.value(), 4);
    }
}
