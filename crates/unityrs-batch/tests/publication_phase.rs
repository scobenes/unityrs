use unityrs_batch::{
    phase::{Account, Buffer, Error},
    phase1::{AttemptFacts, InputPhase, InputState},
    publish,
    records::Figures,
    spool::{DirectoryClaim, Host},
    winners::{Inputs, committed::Committed},
};
use unityrs_testkit::scratch::Scratch;
#[test]
fn capacity_wait_is_not_a_refusal_and_phase_audits_are_distinct() {
    let account = Account::new(64 << 20);
    let block = Buffer::<u8>::new(account.meter(), 48 << 20).unwrap();
    assert!(!account.admits(publish::WORKING_SET));
    assert_eq!(account.snapshot().refusals(), 0);
    drop(block);
    assert!(account.admits(publish::WORKING_SET));
    assert_eq!(
        account.publication(|| {
            let _large = Buffer::<u8>::new(account.meter(), 48 << 20)?;
            Ok(())
        }),
        Ok(())
    );
    // B6's original proof remains enforced even on an account publication used.
    assert_eq!(account.execute(|| Ok(())), Err(Error::internal()));
    assert_eq!(account.snapshot().input.used, 0);
}
#[test]
fn committed_failures_use_receipts_and_never_probe_payload_presence() {
    let scratch = Scratch::new("committed-receipts");
    let facts = AttemptFacts {
        state: unityrs_batch::attempt::State::Failed,
        figures: Figures::default(),
        spooled_part: 0,
        claimed_bytes: 0,
        outputs: 0,
        skips_expected: 0,
        diagnostics: 0,
        code: unityrs_batch::reason::Code::Incomplete,
    };
    let mut states = vec![
        InputState {
            phase: InputPhase::Committed,
            failure_bundle: None,
            received: Some(facts),
            committed: Some(DirectoryClaim { claimed_bytes: 0 }),
        },
        InputState {
            phase: InputPhase::Committed,
            failure_bundle: Some(2),
            received: None,
            committed: Some(DirectoryClaim { claimed_bytes: 0 }),
        },
    ];
    let input = Committed::received(scratch.path(), &states, &Host, &|| false).unwrap();
    assert!(input.failed(0).unwrap());
    assert!(input.failed(1).unwrap());
    states[0].received.as_mut().unwrap().state = unityrs_batch::attempt::State::Done;
    let input = Committed::received(scratch.path(), &states, &Host, &|| false).unwrap();
    assert!(!input.failed(0).unwrap());
    states[0].committed = None;
    assert!(Committed::received(scratch.path(), &states, &Host, &|| false).is_err());
}
