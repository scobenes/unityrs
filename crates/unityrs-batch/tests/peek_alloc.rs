//! One refusal/leak scenario per child, with the real counting allocator.
mod peek_support;
use peek_support::{Harness, bytes};
use std::{
    cell::{Cell, RefCell},
    process::Command,
    sync::Arc,
};
use unityrs_batch::{
    alloc::{Counting, Hook},
    attempt::ReasonCode,
    peek::{Hooks, PeekEnd, Point, State, word::Outcome},
};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_export::ngui;
use unityrs_testkit::child;
thread_local! { static REFUSE: Cell<usize> = const { Cell::new(0) }; }
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, size: usize) -> bool {
        REFUSE
            .try_with(|r| {
                if size != 0 && r.get() == size {
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
    injected: Cell<bool>,
    requested: Cell<bool>,
    meter: RefCell<Option<Arc<Meter>>>,
}
impl ngui::Hooks for Fault {
    fn meter(&self, meter: &Meter) {
        if self.mode.starts_with("before_") || self.mode.starts_with("after_") {
            *self.meter.borrow_mut() = Some(meter.reopen());
            return;
        }
        if self.injected.replace(true) {
            return;
        }
        match self.mode.as_str() {
            "leak" => std::mem::forget(meter.reopen().charge(7).unwrap()),
            "panic" => panic!("peek meter observer"),
            "absorbed" => {
                let meter = meter.reopen();
                let mut values = MeteredVec::<u8>::with_capacity(&meter, 16).unwrap();
                for _ in 0..16 {
                    values.push(0).unwrap();
                }
                REFUSE.with(|r| r.set(32));
                values.push(1).unwrap(); // The smaller growth ladder absorbs it.
                assert_eq!(values.len(), 17);
                REFUSE.with(|r| assert_eq!(r.get(), 0));
            }
            _ => unreachable!(),
        }
    }
}
impl Hooks for Fault {
    fn point(&self, point: Point) {
        let trigger = (self.mode.starts_with("before_") && point == Point::BeforeWord)
            || (self.mode.starts_with("after_") && point == Point::AfterWord);
        if !trigger {
            return;
        }
        let meter = self.meter.borrow();
        let meter = meter.as_ref().unwrap();
        if self.mode.ends_with("leak") {
            std::mem::forget(meter.charge(7).unwrap());
        } else {
            let mut values = MeteredVec::<u8>::with_capacity(meter, 16).unwrap();
            for _ in 0..16 {
                values.push(0).unwrap();
            }
            REFUSE.with(|r| r.set(32));
            values.push(1).unwrap();
            REFUSE.with(|r| assert_eq!(r.get(), 0));
        }
    }
    fn request_stop(&self, _: &PeekEnd) {
        self.requested.set(true);
    }
}
#[test]
fn audit_child() {
    let Ok(mode) = std::env::var("UNITYRS_PEEK_AUDIT") else {
        return;
    };
    let mut h = Harness::new(&bytes(false));
    let fault = Fault {
        mode: mode.clone(),
        injected: Cell::new(false),
        requested: Cell::new(false),
        meter: RefCell::new(None),
    };
    let before = unityrs_batch::alloc::refusals();
    let end = h.run(0, 1, 1 << 20, &fault);
    assert_eq!(end.state, State::RunFailure, "{end:?}");
    assert!(fault.requested.get());
    assert_eq!(
        h.word.load(),
        Ok(if mode.starts_with("after_") {
            Outcome::Clear(11)
        } else {
            Outcome::Pending
        })
    );
    if mode.ends_with("absorbed") {
        assert_eq!(end.failure.unwrap().reason, ReasonCode::HostMemory);
        assert!(unityrs_batch::alloc::refusals() > before);
    } else {
        assert_eq!(end.failure.unwrap().reason, ReasonCode::Internal);
    }
    assert_eq!(end.slot_releasable, !mode.ends_with("leak"));
    assert_eq!(end.accounts.is_clear(), !mode.ends_with("leak"));
}
#[test]
fn allocator_refusals_leaks_and_panics_are_audited_before_publication() {
    for mode in [
        "absorbed",
        "leak",
        "panic",
        "before_leak",
        "after_leak",
        "before_absorbed",
        "after_absorbed",
    ] {
        let result = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "audit_child", "--nocapture", "--test-threads=1"])
                .env("UNITYRS_PEEK_AUDIT", mode),
        )
        .unwrap();
        assert!(
            result.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn allocator_observation_prevents_peek_word_publication_without_a_rust_null() {
    struct Native(Cell<bool>);
    impl ngui::Hooks for Native {
        fn meter(&self, meter: &Meter) {
            if !self.0.replace(true) {
                let _ = meter.allocator_refused(4096);
            }
        }
    }
    impl Hooks for Native {}
    let mut h = Harness::new(&bytes(false));
    let before = unityrs_batch::alloc::refusals();
    let end = h.run(0, 1, 1 << 20, &Native(Cell::new(false)));
    assert_eq!(end.state, State::RunFailure);
    assert_eq!(end.failure.unwrap().reason, ReasonCode::HostMemory);
    assert_eq!(end.accounts.allocator_refusals, 1);
    assert_eq!(end.accounts.refusals(), 0);
    assert_eq!(unityrs_batch::alloc::refusals(), before);
    assert!(end.slot_releasable);
    assert!(end.accounts.is_clear());
    assert_eq!(h.word.load(), Ok(Outcome::Pending));
}
