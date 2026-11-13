mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
#[path = "winners_support/member_scene.rs"]
mod member_scene;
#[path = "winners_support/probe.rs"]
mod probe;
use harness::Case;
use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use unityrs_batch::{
    attempt::{DetailCode, ReasonCode},
    phase::{Account, Error},
    sort::{Hooks, NoHooks, Options, Point},
    spool::{Counter, Host, workspace::Workspace},
    winners::{self, Context, Observer, Pass, Prepared},
};
use unityrs_testkit::scratch::Scratch;

fn scene() -> Case {
    member_scene::scene(false)
}
struct StopAt<'a> {
    target: Point,
    pass: Cell<Pass>,
    wanted: Pass,
    stop: &'a AtomicBool,
}
impl Observer for StopAt<'_> {
    fn pass(&self, pass: Pass, _: u64) {
        self.pass.set(pass);
    }
}
impl Hooks for StopAt<'_> {
    fn point(&self, p: Point) {
        if p == self.target && self.pass.get() == self.wanted {
            self.stop.store(true, Ordering::Release);
        }
    }
}
#[test]
fn stops_at_pass_boundaries_and_stream_operations_never_return_readiness() {
    for wanted in [
        Pass::Roots,
        Pass::Claims,
        Pass::Keepers,
        Pass::UnitConflicts,
        Pass::Reservations,
        Pass::Keys,
        Pass::Prefix,
        Pass::Propagation,
        Pass::Winners,
        Pass::Rows,
        Pass::Expansions,
        Pass::Membership,
        Pass::MemberRows,
        Pass::Handoff,
    ] {
        {
            let target = Point::Pass;
            let case = scene();
            let scratch = Scratch::new("winner-stop");
            let spec = case.spec().spec;
            let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
            let account = Account::new(64 << 20);
            let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
            let prepared = Prepared::from_spec(&spec).unwrap();
            let stop = AtomicBool::new(false);
            let hooks = StopAt {
                target,
                wanted,
                pass: Cell::new(Pass::Roots),
                stop: &stop,
            };
            let mut workspace =
                Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
            let result = winners::complete(&mut Context {
                spec: &spec,
                prepared: &prepared,
                inputs: &commits,
                workspace: &mut workspace,
                account: &account,
                options: Options {
                    run_records: 7,
                    fan_in: 2,
                    ..Options::default()
                },
                stopped: &|| stop.load(Ordering::Acquire),
                hooks: &hooks,
                observer: &hooks,
            });
            assert!(
                stop.load(Ordering::Acquire),
                "unreached {wanted:?}/{target:?}"
            );
            assert_eq!(result.unwrap_err(), Error::Stopped);
            assert_eq!(account.snapshot().input.used, 0);
            let used = counter.used();
            drop(workspace);
            assert_eq!(counter.used(), used);
        }
    }
}
#[test]
fn faults_on_decision_and_winner_paths_keep_exact_class_and_claims() {
    for pass in [
        Pass::Rows,
        Pass::Expansions,
        Pass::Membership,
        Pass::MemberRows,
        Pass::Handoff,
    ] {
        for operation in [1, 2, 3, 5, 6, 7, 8] {
            for errno in [libc::EIO, libc::ENOSPC, libc::EDQUOT] {
                let case = scene();
                let scratch = Scratch::new("winner-fault");
                let spec = case.spec().spec;
                let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
                let account = Account::new(64 << 20);
                let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
                let probe = probe::Probe::new(counter.clone(), counter.used(), 4096);
                probe.operation.store(operation, Ordering::Relaxed);
                probe.errno.store(errno, Ordering::Relaxed);
                probe.fail_pass.store(pass as usize, Ordering::Relaxed);
                let mut workspace = Workspace::create(
                    scratch.join("w"),
                    counter.clone(),
                    Arc::new(probe::Fs(probe.clone())),
                )
                .unwrap();
                let prepared = Prepared::from_spec(&spec).unwrap();
                let result = winners::complete(&mut Context {
                    spec: &spec,
                    prepared: &prepared,
                    inputs: &commits,
                    workspace: &mut workspace,
                    account: &account,
                    options: Options::default(),
                    stopped: &|| false,
                    hooks: &NoHooks,
                    observer: probe.as_ref(),
                });
                if let Err(Error::Run(failure)) = result {
                    let read = matches!(operation, 7 | 8);
                    assert_eq!(
                        failure.detail,
                        if errno == libc::EIO || read {
                            None
                        } else {
                            Some(if errno == libc::ENOSPC {
                                DetailCode::Enospc
                            } else {
                                DetailCode::Edquot
                            })
                        }
                    );
                    assert_eq!(
                        failure.reason,
                        if errno == libc::EIO || read {
                            ReasonCode::HostIo
                        } else {
                            ReasonCode::SpoolFull
                        }
                    );
                } else {
                    panic!("unreached {pass:?}/{operation}: {result:?}");
                }
                assert_eq!(account.snapshot().input.used, 0);
                let used = counter.used();
                drop(workspace);
                assert_eq!(counter.used(), used);
            }
        }
    }
}

#[test]
fn member_pass_writes_and_final_return_observe_stop() {
    for wanted in [
        Pass::Rows,
        Pass::Expansions,
        Pass::Membership,
        Pass::MemberRows,
        Pass::Handoff,
    ] {
        for target in [Point::BeforeWrite, Point::BeforeReturn] {
            if target == Point::BeforeReturn && wanted != Pass::Handoff {
                continue;
            }
            let scratch = Scratch::new("member-operation-stop");
            let case = scene();
            let spec = case.spec().spec;
            let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
            let account = Account::new(64 << 20);
            let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
            let prepared = Prepared::from_spec(&spec).unwrap();
            let stop = AtomicBool::new(false);
            let hooks = StopAt {
                target,
                wanted,
                pass: Cell::new(Pass::Roots),
                stop: &stop,
            };
            let mut workspace =
                Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
            let result = winners::complete(&mut Context {
                spec: &spec,
                prepared: &prepared,
                inputs: &commits,
                workspace: &mut workspace,
                account: &account,
                options: Options::default(),
                stopped: &|| stop.load(Ordering::Acquire),
                hooks: &hooks,
                observer: &hooks,
            });
            assert!(
                stop.load(Ordering::Acquire),
                "unreached {wanted:?}/{target:?}"
            );
            assert_eq!(result.unwrap_err(), Error::Stopped);
            assert_eq!(account.snapshot().input.used, 0);
            let used = counter.used();
            drop(workspace);
            assert_eq!(counter.used(), used);
        }
    }
}

struct Audit<'a> {
    account: &'a Account,
    retained: std::cell::RefCell<Option<unityrs_core::limits::Charge>>,
    panic: bool,
    reported: Cell<Option<ReasonCode>>,
}
impl Observer for Audit<'_> {
    fn pass(&self, pass: Pass, _: u64) {
        if pass == Pass::Handoff {
            assert!(!self.panic, "injected complete seam panic");
            *self.retained.borrow_mut() = Some(self.account.meter().charge(1).unwrap());
        }
    }
}
impl Hooks for Audit<'_> {
    fn request_stop(&self, f: unityrs_batch::job::RunFailure) {
        self.reported.set(Some(f.reason));
    }
}
#[test]
fn complete_audits_panics_and_retained_charges_before_readiness() {
    for panic in [true, false] {
        let scratch = Scratch::new("complete-audit");
        let case = scene();
        let spec = case.spec().spec;
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let account = Account::new(64 << 20);
        let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
        let prepared = Prepared::from_spec(&spec).unwrap();
        let hooks = Audit {
            account: &account,
            retained: std::cell::RefCell::new(None),
            panic,
            reported: Cell::new(None),
        };
        let mut workspace =
            Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
        let result = winners::complete(&mut Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options::default(),
            stopped: &|| false,
            hooks: &hooks,
            observer: &hooks,
        });
        assert_eq!(result.unwrap_err(), Error::internal());
        assert_eq!(hooks.reported.get(), Some(ReasonCode::Internal));
        drop(hooks.retained.take());
        assert_eq!(account.snapshot().input.used, 0);
        let used = counter.used();
        drop(workspace);
        assert_eq!(counter.used(), used);
    }
}
