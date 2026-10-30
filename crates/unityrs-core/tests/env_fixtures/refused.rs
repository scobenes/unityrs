//! A container pass the budget refused (contention, never the input's
//! fault) is walked again by the next ask, so after it the replay answers
//! as UnityPy recorded (plan section 3.16, P1; review round 5: the refused
//! pass was marked built, and the preload's paths stayed missing for the
//! generation). UnityPy has no budget: what it recorded is the answer of a
//! pass nothing refused, which the retried pass must give.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use unityrs_core::FileContext;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{EnvOptions, LoadMode};

use super::EAGER_DIFFERS;
use super::run::{Run, ops, recorded, scene_by_name};

/// Refuses every reservation while it is switched on.
#[derive(Debug, Default)]
struct Switch(AtomicBool);

impl MemoryBudget for Switch {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if self.0.load(Ordering::SeqCst) {
            Err(BudgetExceeded {
                requested: bytes,
                what,
            })
        } else {
            Ok(Permit::detached(bytes))
        }
    }
}

/// Every scene whose recorded container lists a row, under each mode: the
/// loads, a pass the budget refuses (when the pass charges anything before
/// it meets an error of the input's), then
/// UnityPy's steps from the first read on, whose `container` and `paths` answers are UnityPy's (a step
/// pinned in `PARITY.md` section 19.2 is left out, as the Eager replay
/// leaves it).
#[test]
fn a_pass_the_budget_refused_is_walked_again() {
    let mut checked = 0;
    for (name, steps) in recorded() {
        let lists_a_row = steps.iter().any(|s| {
            s["op"][0] == "container" && s["result"].as_array().is_some_and(|r| !r.is_empty())
        });
        if !lists_a_row {
            continue;
        }
        let ops = ops(&steps);
        let loads = ops.iter().take_while(|op| op[0] == "load").count();
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let switch = Arc::new(Switch::default());
            let mut ctx = FileContext::unitypy();
            ctx.budget = switch.clone();
            let mut options = EnvOptions::unitypy();
            options.context = Arc::new(ctx);
            let run = Run::with(scene_by_name(&name), mode, options);
            drop(run.steps(&ops[..loads]));
            switch.0.store(true, Ordering::SeqCst);
            let refused = run.env.container().map(|_| ());
            switch.0.store(false, Ordering::SeqCst);
            // A pass that charges nothing new is not refused, and one that
            // fails on the input first is the scene's own answer.
            let by_budget = |e: &unityrs_core::Error| e.to_string().starts_with("memory budget");
            if !refused.as_ref().is_err_and(by_budget) {
                continue;
            }
            let ours = run.steps(&ops[loads..]);
            for (want, got) in steps[loads..].iter().zip(&ours) {
                let op = &want["op"][0];
                let pinned = EAGER_DIFFERS.iter().any(|(n, o)| *n == name && op == *o);
                if pinned || !(op == "container" || op == "paths") {
                    continue;
                }
                assert_eq!(got["result"], want["result"], "{name} {mode:?} {op}");
            }
            checked += 1;
        }
    }
    assert!(checked >= 6, "{checked} refused passes checked");
}
