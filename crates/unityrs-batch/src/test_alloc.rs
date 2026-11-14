//! The library test binary's one global allocator. A binary has exactly one,
//! so every unit test that measures or refuses allocation shares this one:
//! it counts like the engine's (`alloc::measured`) and refuses only when a
//! test arms `REFUSE` on its own thread.
use crate::alloc::{Counting, Hook};
use std::cell::Cell;

thread_local! {
    /// The allocation, counted from 1, at which this thread's next refusal
    /// falls; 0 never refuses.
    pub(crate) static REFUSE: Cell<usize> = const { Cell::new(0) };
}

pub(crate) struct Refusal;
impl Hook for Refusal {
    fn refuses(&self, _: usize) -> bool {
        REFUSE
            .try_with(|r| {
                let left = r.get();
                r.set(left.saturating_sub(1));
                left == 1
            })
            .unwrap_or(false)
    }
}

#[global_allocator]
static ALLOC: Counting<Refusal> = Counting(Refusal);
