use super::*;
use crate::{
    lifetime::{Boot, Policy, State},
    manifest::{Family, WriteError, chunks::Destination},
};
use std::cell::{Cell, RefCell};

/// A scripted clock: time moves only when a scripted operation spends it.
#[derive(Default)]
struct Script {
    now: Cell<u64>,
}
impl Clock for Script {
    fn now(&self) -> Boot {
        Boot(self.now.get())
    }
    fn sleep(&self, ms: u64) {
        self.now.set(self.now.get() + ms);
    }
}
/// A store whose every chunk costs `cost` ms, ending early at its deadline
/// as a store request's timeout does.
struct Store<'a> {
    clock: &'a Script,
    cost: u64,
    tries: RefCell<Vec<(u64, u64, Option<Boot>)>>,
}
impl Rows for Store<'_> {
    fn put(
        &mut self,
        _: Family,
        _: u64,
        _: &[u8],
        deadline: Option<Boot>,
    ) -> Result<(), WriteError> {
        let start = self.clock.now.get();
        let end = deadline.map_or(start + self.cost, |d| (start + self.cost).min(d.0));
        self.clock.now.set(end);
        self.tries.borrow_mut().push((start, end, deadline));
        if end < start + self.cost {
            return Err(WriteError::Publish);
        }
        Ok(())
    }
}

#[test]
fn a_cut_row_cannot_take_the_heads_reserve() {
    for grace in [10_000_u64, 20_000, 600_000] {
        for reconcile in [false, true] {
            for cost in [1, 700, 2_900, 9_000] {
                let take = 1_000_000;
                let policy = Policy::at(None, grace, 0, Duration::from_millis(take)).unwrap();
                let hard = policy.hard(Boot(take)).unwrap();
                assert_eq!(hard.0, take + grace - 500);
                let budget = Budget {
                    hard: &Fixed(Some(hard)),
                    reconcile,
                };
                let clock = Script::default();
                clock.now.set(take);
                let mut store = Store {
                    clock: &clock,
                    cost,
                    tries: RefCell::new(Vec::new()),
                };
                let mut rows = Budgeted {
                    rows: &mut store,
                    budget,
                    clock: &clock,
                    cut: false,
                };
                // Eight families of unbounded chunks: more than any grace holds.
                for family in Family::ALL {
                    for chunk in 0.. {
                        if rows.publish(family, chunk, b"x\n").is_err() {
                            break;
                        }
                    }
                }
                let reserve = if reconcile { 5_000 } else { 3_000 };
                let cut = hard.0 - reserve;
                assert_eq!(budget.row_deadline(), Some(Boot(cut)));
                assert!(rows.cut, "grace {grace} cost {cost}: nothing cut");
                for &(start, end, deadline) in store.tries.borrow().iter() {
                    assert!(start < cut, "a chunk started inside the reserve");
                    assert!(end <= cut, "a chunk ran into the reserve");
                    assert_eq!(deadline, Some(Boot(cut)));
                }
                // The head starts at once after the rows, with its reserve
                // whole: at least 3 s (5 s reconciling) before T_hard.
                let head = clock.now.get();
                assert!(head <= cut, "grace {grace} cost {cost}");
                assert!(budget.head_deadline().unwrap().0 >= head + 1_000);
                assert_eq!(budget.head_deadline(), Some(Boot(hard.0 - 2_000)));
            }
        }
    }
}

#[test]
fn unarmed_rows_are_never_cut() {
    let clock = Script::default();
    clock.now.set(u64::MAX - 10);
    let mut store = Store {
        clock: &Script::default(),
        cost: 0,
        tries: RefCell::new(Vec::new()),
    };
    let mut rows = Budgeted {
        rows: &mut store,
        budget: Budget::UNARMED,
        clock: &clock,
        cut: false,
    };
    assert_eq!(rows.publish(Family::Inputs, 0, b"x\n"), Ok(()));
    assert!(!rows.cut);
    assert_eq!(Budget::UNARMED.head_deadline(), None);
    assert_eq!(
        Budget::UNARMED.abort_timeout(Boot(0)),
        Some(Duration::from_secs(5))
    );
}

/// A journal busy for `busy` polls, then holding `uploads` ids; each abort
/// costs `cost` ms of its timeout and fails when the timeout is shorter.
struct Journal<'a> {
    clock: &'a Script,
    busy: u32,
    uploads: u64,
    cost: u64,
    timeouts: Vec<Duration>,
}
impl Backend for Journal<'_> {
    fn try_close(&mut self) -> Close {
        if self.busy == 0 {
            return Close::Closed;
        }
        self.busy -= 1;
        Close::Busy
    }
    fn next_upload(&mut self) -> Option<u64> {
        self.uploads = self.uploads.checked_sub(1)?;
        Some(self.uploads)
    }
    fn abort(&mut self, _: u64, timeout: Duration) -> bool {
        self.timeouts.push(timeout);
        let spent = u64::try_from(timeout.as_millis()).unwrap().min(self.cost);
        self.clock.now.set(self.clock.now.get() + spent);
        spent == self.cost
    }
}

#[test]
fn the_journal_close_polls_every_ten_ms_and_gives_up_at_the_heads_reserve() {
    let clock = Script::default();
    let hard = Boot(20_000);
    let budget = Budget {
        hard: &Fixed(Some(hard)),
        reconcile: false,
    };
    let mut journal = Journal {
        clock: &clock,
        busy: 3,
        uploads: 2,
        cost: 100,
        timeouts: Vec::new(),
    };
    let settled = settle(&mut journal, budget, &clock);
    assert_eq!(clock.now.get(), 30 + 200);
    assert!(settled.closed);
    assert_eq!(
        (settled.aborted, settled.abort_failed, settled.left),
        (2, 0, 0)
    );
    assert!(
        journal
            .timeouts
            .iter()
            .all(|t| *t == Duration::from_secs(5))
    );

    // Held forever: give up at T_hard - head_reserve, skipping the aborts.
    let clock = Script::default();
    let mut journal = Journal {
        clock: &clock,
        busy: u32::MAX,
        uploads: 2,
        cost: 100,
        timeouts: Vec::new(),
    };
    let settled = settle(&mut journal, budget, &clock);
    assert!(!settled.closed);
    assert_eq!(clock.now.get(), 17_000);
    assert_eq!(journal.timeouts, []);
}

#[test]
fn aborts_shrink_to_the_reserve_and_stop_under_half_a_second() {
    let clock = Script::default();
    clock.now.set(10_000);
    let budget = Budget {
        hard: &Fixed(Some(Boot(19_000))),
        reconcile: false,
    };
    // Cut at 16 s: 6 s left, then 1 s, then 0.4 s (none).
    let mut journal = Journal {
        clock: &clock,
        busy: 0,
        uploads: 4,
        cost: 5_000,
        timeouts: Vec::new(),
    };
    let settled = settle(&mut journal, budget, &clock);
    assert_eq!(
        journal.timeouts,
        [Duration::from_secs(5), Duration::from_secs(1)]
    );
    assert_eq!(
        (settled.aborted, settled.abort_failed, settled.left),
        (1, 1, 2)
    );
    let mut none = NoJournal;
    assert_eq!(settle(&mut none, budget, &clock), Settled::default());
}

/// A store that arms `T_hard` from inside its third chunk, as a signal
/// after the finisher began would: every later chunk sees it.
struct Arming<'a> {
    clock: &'a Script,
    state: &'a State,
    hard: Boot,
    starts: Vec<(u64, Option<Boot>)>,
}
impl Rows for Arming<'_> {
    fn put(
        &mut self,
        _: Family,
        _: u64,
        _: &[u8],
        deadline: Option<Boot>,
    ) -> Result<(), WriteError> {
        self.starts.push((self.clock.now.get(), deadline));
        if self.starts.len() == 3 {
            self.state.arm(self.hard);
        }
        self.clock.sleep(1_000);
        Ok(())
    }
}

#[test]
fn a_deadline_armed_after_the_finish_began_still_cuts_the_rows() {
    let state = State::new();
    let clock = Script::default();
    let hard = Boot(20_000);
    let mut store = Arming {
        clock: &clock,
        state: &state,
        hard,
        starts: Vec::new(),
    };
    let budget = Budget {
        hard: &state,
        reconcile: false,
    };
    assert_eq!(budget.row_deadline(), None);
    let mut rows = Budgeted {
        rows: &mut store,
        budget,
        clock: &clock,
        cut: false,
    };
    for chunk in 0.. {
        if rows.publish(Family::Inputs, chunk, b"x\n").is_err() {
            break;
        }
    }
    assert!(rows.cut);
    let cut = Boot(hard.0 - 3_000);
    assert_eq!(budget.row_deadline(), Some(cut));
    // Three unarmed chunks, then every later one carries the cut and
    // starts before it; the head's reserve is left whole.
    assert_eq!(store.starts[..3], [(0, None), (1_000, None), (2_000, None)]);
    assert_eq!(store.starts.len(), 17);
    for &(start, deadline) in &store.starts[3..] {
        assert!(start < cut.0);
        assert_eq!(deadline, Some(cut));
    }
    assert!(clock.now.get() <= cut.0);
}

/// A journal held busy, whose holder arms `T_hard` on its fifth poll.
struct LateArm<'a> {
    state: &'a State,
    polls: u32,
}
impl Backend for LateArm<'_> {
    fn try_close(&mut self) -> Close {
        self.polls += 1;
        if self.polls == 5 {
            self.state.arm(Boot(20_000));
        }
        Close::Busy
    }
    fn next_upload(&mut self) -> Option<u64> {
        None
    }
    fn abort(&mut self, _: u64, _: Duration) -> bool {
        true
    }
}

#[test]
fn settle_built_unarmed_gives_up_at_a_later_armed_reserve() {
    let state = State::new();
    let clock = Script::default();
    let budget = Budget {
        hard: &state,
        reconcile: true,
    };
    let mut journal = LateArm {
        state: &state,
        polls: 0,
    };
    let settled = settle(&mut journal, budget, &clock);
    assert!(!settled.closed);
    // Reconciling keeps 5 s: given up at 15 s, not polled forever.
    assert_eq!(clock.now.get(), 15_000);
    assert_eq!(budget.abort_timeout(clock.now()), None);
}
