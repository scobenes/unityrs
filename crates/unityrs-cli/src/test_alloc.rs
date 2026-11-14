//! Thread-local refusal injection for the CLI test binary.
use std::cell::Cell;
use unityrs_batch::alloc::{Counting, Hook};
thread_local! {
    pub(crate) static REFUSE: Cell<bool> = const { Cell::new(false) };
}
struct Refusal;
impl Hook for Refusal {
    fn refuses(&self, _: usize) -> bool {
        REFUSE.try_with(|r| r.replace(false)).unwrap_or(false)
    }
}
#[global_allocator]
static ALLOC: Counting<Refusal> = Counting(Refusal);
