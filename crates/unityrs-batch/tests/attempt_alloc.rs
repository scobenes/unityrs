//! Whole-attempt refusals, including a successful growth's absorbed refusal.
mod attempt_support;
use attempt_support::*;
use std::{cell::Cell, process::Command};
use unityrs_batch::{
    alloc::{Counting, Hook},
    attempt::{AttemptEnd, Hooks, Point, ReasonCode, State},
};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_export::ngui;
use unityrs_testkit::{child, ngui::texture};
thread_local! { static REFUSE: Cell<usize> = const { Cell::new(0) }; }
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, size: usize) -> bool {
        REFUSE
            .try_with(|r| {
                if r.get() == size && size != 0 {
                    r.set(0);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);
struct Fault {
    mode: String,
    used: Cell<bool>,
    armed: Cell<bool>,
    requested: Cell<bool>,
}
impl ngui::Hooks for Fault {
    fn meter(&self, meter: &Meter) {
        if self.mode == "callback" {
            if self.armed.get()
                && (meter.reservation() == unityrs_core::budget::Reservation::Dependency
                    || self.used.get())
            {
                self.used.set(true);
                panic!("observer in nested meter guards");
            }
            return;
        }
        if self.mode == "buffer" || self.used.replace(true) {
            return;
        }
        if self.mode == "leak" {
            std::mem::forget(meter.reopen().charge(7).unwrap());
            return;
        }
        let meter = meter.reopen();
        let mut values = MeteredVec::<u8>::with_capacity(&meter, 16).unwrap();
        for _ in 0..16 {
            values.push(0).unwrap();
        }
        REFUSE.with(|r| r.set(32));
        // The 32-byte doubling is refused, but the growth ladder fits 24.
        values.push(1).unwrap();
        assert_eq!(values.len(), 17);
        REFUSE.with(|r| assert_eq!(r.get(), 0, "the allocator actually refused"));
    }
}
impl Hooks for Fault {
    fn point(&self, p: Point) {
        if p == Point::Registered {
            self.armed.set(true);
        }
        if self.mode == "buffer" && p == Point::Registered {
            REFUSE.with(|r| r.set(unityrs_batch::records::BUFFER));
        }
    }
    fn request_stop(&self, _: &AttemptEnd) {
        self.requested.set(true);
    }
}
#[test]
fn refusal_child() {
    let Ok(mode) = std::env::var("UNITYRS_ATTEMPT_REFUSAL") else {
        return;
    };
    let scene = if mode == "callback" {
        unityrs_testkit::ngui::scenes()
            .unwrap()
            .into_iter()
            .find(|(n, _)| *n == "struct_maintex_in_dependency")
            .unwrap()
            .1
    } else {
        simple(&[texture(1, b"image", 4, (1, 1), vec![1, 2, 3, 4])])
    };
    let h = Harness::new(&scene, &scene.inputs[0]);
    let fault = Fault {
        mode: mode.clone(),
        used: Cell::new(false),
        armed: Cell::new(false),
        requested: Cell::new(false),
    };
    let before = unityrs_batch::alloc::refusals();
    let end = h.run(limits(), &fault);
    assert_eq!(end.state, State::RunFailure, "{end:?}");
    assert!(fault.requested.get());
    if mode == "callback" {
        clear(&end);
        assert_eq!(end.reason, Some(ReasonCode::Internal));
        assert!(fault.used.get());
    } else if mode == "leak" {
        assert_eq!(end.reason, Some(ReasonCode::Internal));
        assert!(!end.slot_releasable);
        assert!(!end.accounts.is_clear());
    } else {
        clear(&end);
        assert_eq!(end.reason, Some(ReasonCode::HostMemory));
        assert!(unityrs_batch::alloc::refusals() > before);
        if mode == "absorbed" {
            assert_eq!(end.outputs, 1);
            assert!(h.directory.path().join("o0").exists());
        }
    }
}
#[test]
fn allocator_refusals_and_leaks_in_isolated_children() {
    for mode in ["buffer", "absorbed", "leak", "callback"] {
        let result = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "refusal_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("UNITYRS_ATTEMPT_REFUSAL", mode),
        )
        .unwrap();
        assert!(
            result.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
}
