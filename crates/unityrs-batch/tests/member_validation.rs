//! A corrupt committed path cannot enter B8's included-member list.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
use std::sync::Arc;
use unityrs_batch::{
    phase::{Account, Error},
    sort::{NoHooks, Options},
    spool::{Counter, Host, workspace::Workspace},
    winners::{self, Context, Prepared},
};
use unityrs_testkit::scratch::Scratch;
#[test]
fn invalid_member_paths_marked_valid_refuse_the_complete_handoff() {
    for path in ["/bad", "../bad", "a//b", "a\0b", "a/", "."] {
        let scratch = Scratch::new("invalid-member");
        let mut case = harness::Case::new(vec![harness::candidate("a", "key", None, 1)]);
        case.archives.push("archive".into());
        case.expansions.push(harness::Expansion {
            input: "a".into(),
            key: "key".into(),
            archive: 0,
            path: path.into(),
            reason: None,
            cut: false,
        });
        let spec = case.spec().spec;
        let account = Account::new(64 << 20);
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
        let prepared = Prepared::from_spec(&spec).unwrap();
        let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
        let result = winners::complete(&mut Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options::default(),
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &winners::NoObserver,
        });
        assert_eq!(result.unwrap_err(), Error::internal(), "path={path:?}");
        assert_eq!(account.snapshot().input.used, 0);
    }
}
