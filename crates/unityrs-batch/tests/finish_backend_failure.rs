//! Backend run failures must survive the journal's best-effort abort policy.
use std::{path::PathBuf, sync::Arc, time::Duration};
use unityrs_batch::{
    attempt::ReasonCode,
    finish::{
        self,
        budget::{Backend, Close},
    },
    job::RunFailure,
    lifetime::Stop,
    phase::Account,
    progress::Phase,
    reason::{Code, StopReason},
};
struct Refused(ReasonCode);
impl Backend for Refused {
    fn try_close(&mut self) -> Close {
        Close::Closed
    }
    fn next_upload(&mut self) -> Option<u64> {
        None
    }
    fn abort(&mut self, _: u64, _: Duration) -> bool {
        false
    }
    fn failure(&mut self) -> Option<RunFailure> {
        Some(RunFailure::new(self.0))
    }
}
#[test]
fn journal_backend_run_failures_override_a_stop_before_running() {
    for (reason, code, name) in [
        (ReasonCode::HostMemory, Code::Host, "host_memory"),
        (ReasonCode::Internal, Code::Internal, "internal"),
    ] {
        let mut handover = finish::HandOver::new(finish::FinishCause::Stop, Phase::Start);
        handover.spool = Some(finish::Spool {
            run: PathBuf::new(),
            done: PathBuf::new(),
            fs: Arc::new(unityrs_batch::spool::Host),
            lease: None,
        });
        let account = Account::new(unityrs_batch::bound::C_FIN);
        let identity = finish::Identity {
            started_at: "2026-01-01T00:00:00Z",
            memory_bound: 0,
            spool_peak: 0,
            vm_hwm: 0,
            warnings: unityrs_batch::finish::Warnings::default(),
        };
        let finished = finish::run(
            &handover,
            Some(Stop::external(StopReason::Sigterm)),
            &mut finish::Finisher {
                account: &account,
                identity: &identity,
                budget: finish::budget::Budget::UNARMED,
                clock: &finish::budget::BootClock,
                backend: &mut Refused(reason),
            },
        );
        assert_eq!(finished.end.code, code);
        assert_eq!(finished.end.reason, Some(name));
        assert_eq!(finished.end.status, None);
    }
}
