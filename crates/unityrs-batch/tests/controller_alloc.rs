mod controller_support;
mod stage_support;
use controller_support::*;
use std::{
    cell::Cell,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use unityrs_batch::{
    alloc::{Counting, Hook},
    attempt,
    controller::{Boundary, Event, Hooks, Point},
    peek, records,
};
use unityrs_core::limits::Meter;
use unityrs_testkit::{child, guard};
thread_local! { static REFUSE: Cell<usize> = const { Cell::new(0) }; }
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, size: usize) -> bool {
        REFUSE
            .try_with(|r| {
                size != 0 && size == r.get() && {
                    r.set(0);
                    true
                }
            })
            .unwrap_or(false)
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);
struct Fault {
    mode: String,
    armed: AtomicBool,
    used: AtomicBool,
}
impl unityrs_export::ngui::Hooks for Fault {
    fn meter(&self, meter: &Meter) {
        if self.armed.load(Ordering::Acquire)
            && self.mode.ends_with("leak")
            && !self.used.swap(true, Ordering::AcqRel)
        {
            std::mem::forget(meter.reopen().charge(7).unwrap());
        }
    }
}
impl Hooks for Fault {
    fn event(&self, e: Event) {
        if matches!(
            (self.mode.as_str(), e),
            ("peek_leak", Event::PeekDispatched(_)) | ("attempt_leak", Event::AttemptDispatched(_))
        ) {
            self.armed.store(true, Ordering::Release);
        }
    }
    fn point(&self, p: Point) {
        if self.mode == "failure_buffer" && matches!(p, Point::FailureCreated(_)) {
            REFUSE.with(|r| r.set(records::BUFFER));
        }
        if self.mode == "buffer" && matches!(p, Point::Attempt(_, attempt::Point::Registered)) {
            REFUSE.with(|r| r.set(records::BUFFER));
        }
        if self.mode == "adapter" {
            assert!(
                !matches!(p, Point::Peek(_, peek::Point::Open)),
                "adapter panic"
            );
        }
    }
}
#[test]
fn fault_child() {
    let Ok(mode) = std::env::var("UNITYRS_CONTROLLER_FAULT") else {
        return;
    };
    let () = guard::finishes("isolated controller fault", move || {
        let mut h = Harness::new(
            &[
                (
                    "a",
                    if mode == "failure_buffer" {
                        b"bad".to_vec()
                    } else {
                        clear(1)
                    },
                    vec![],
                ),
                ("b", clear(1), vec![]),
            ],
            &[],
            8,
            1,
        );
        let fault = Arc::new(Fault {
            mode: mode.clone(),
            armed: AtomicBool::new(false),
            used: AtomicBool::new(false),
        });
        let mut c = h.controller(fault);
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::HandOver
        );
        assert!(h.control.load(Ordering::Acquire));
        assert_eq!(c.reserved_slots(), usize::from(mode != "failure_buffer"));
        assert_eq!(
            std::fs::read_dir(h.scratch.join("done")).unwrap().count(),
            0
        );
        if mode.ends_with("leak") {
            assert_eq!(
                c.failure().map(|f| f.reason),
                None,
                "unreceived end has no controller failure fact"
            );
        }
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}
#[test]
fn allocator_refusals_adapter_panics_and_leaks_stop_without_refilling_slots() {
    for mode in [
        "peek_leak",
        "attempt_leak",
        "buffer",
        "failure_buffer",
        "adapter",
    ] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "fault_child", "--nocapture", "--test-threads=1"])
                .env("UNITYRS_CONTROLLER_FAULT", mode),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
