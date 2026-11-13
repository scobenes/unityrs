mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
#[path = "winners_support/probe.rs"]
mod probe;
use harness::{Case, candidate};
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
    let mut rows = Vec::new();
    for input in ["a", "b", "c"] {
        for unit in ["a", "b"] {
            rows.push(candidate(input, unit, Some(unit), 1));
            for i in 0..24 {
                rows.push(candidate(input, &format!("{unit}/{i}"), Some(unit), 2));
            }
            rows.push(candidate(input, "shared", Some(unit), 3));
        }
        for i in 0..24 {
            rows.push(candidate(input, &format!("plain/{i}"), None, 3));
        }
    }
    // One key per input is required by B4; units may share keys across inputs,
    // but not within one input. Alternate claimants supply the shared key.
    rows.retain(|c| {
        c.key != "shared" || c.unit.as_deref() == Some(if c.input == "a" { "a" } else { "b" })
    });
    let mut case = Case::new(rows);
    case.archives.push("z/archive".into());
    case
}
#[test]
fn actual_pass_peaks_and_analytic_sort_envelopes_fit_the_revised_rule() {
    for block in [3072, 4096, 5003, 65_536] {
        let case = scene();
        let scratch = Scratch::new("winner-ledger");
        let spec = case.spec().spec;
        let counter = Counter::new(u64::MAX, 0, block).unwrap();
        let account = Account::new(64 << 20);
        let (commits, spooled) = case.commit(&scratch, &counter, &account, &spec);
        let baseline = counter.used();
        let k = if block < 4096 {
            2
        } else {
            block.div_ceil(4096)
        };
        let s = spooled * k;
        let a = spec
            .archives
            .iter()
            .map(|a| {
                unityrs_batch::work::rounded(u64::try_from(a.key.len()).unwrap() + 512, block)
                    .unwrap()
            })
            .sum::<u64>();
        let allowance = 6 * s + 2 * a;
        let probe = probe::Probe::new(counter.clone(), baseline, block);
        let mut workspace = Workspace::create(
            scratch.join("w"),
            counter.clone(),
            Arc::new(probe::Fs(probe.clone())),
        )
        .unwrap();
        let prepared = Prepared::from_spec(&spec).unwrap();
        let mut cx = Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options::default(),
            stopped: &|| false,
            hooks: &NoHooks,
            observer: probe.as_ref(),
        };
        let artifacts = winners::run(&mut cx).unwrap();
        probe.sample();
        assert_eq!(
            case.decisions(&cx, &artifacts).len(),
            case.candidates
                .iter()
                .map(|c| &c.key)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
        for (pass, (peak, bound)) in probe.peaks.iter().zip(&probe.bounds).enumerate() {
            let peak = peak.load(Ordering::Relaxed);
            let bound = bound.load(Ordering::Relaxed);
            println!(
                "PASS_LEDGER block={block} pass={pass} S={s} A={a} allowance={allowance} peak={peak} sort_bound={bound}"
            );
            assert!(peak <= allowance);
            assert!(bound <= allowance);
        }
        assert_eq!(
            counter.peak() - baseline,
            probe
                .peaks
                .iter()
                .map(|p| p.load(Ordering::Relaxed))
                .max()
                .unwrap()
        );
        assert_eq!(account.snapshot().input.used, 0);
    }
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
    ] {
        for target in [Point::Pass, Point::BeforeWrite, Point::BeforeReturn] {
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
            let result = winners::run(&mut Context {
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
            if stop.load(Ordering::Acquire) {
                assert!(matches!(result, Err(Error::Stopped)));
            } else {
                assert!(
                    matches!(wanted, Pass::Keepers | Pass::UnitConflicts | Pass::Winners),
                    "unreached {wanted:?}/{target:?}"
                );
            }
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
        Pass::Roots,
        Pass::Keepers,
        Pass::UnitConflicts,
        Pass::Keys,
        Pass::Prefix,
        Pass::Propagation,
        Pass::Winners,
    ] {
        for operation in [1, 2, 3, 5, 6, 7, 8] {
            if pass == Pass::Winners && operation == 6 {
                continue;
            }
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
                let result = winners::run(&mut Context {
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
                    assert!(
                        pass == Pass::UnitConflicts && matches!(operation, 6..=8),
                        "unreached {pass:?}/{operation}: {result:?}"
                    );
                }
                assert_eq!(account.snapshot().input.used, 0);
                let used = counter.used();
                drop(workspace);
                assert_eq!(counter.used(), used);
            }
        }
    }
}
