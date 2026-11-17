//! Thread-local refusal injection for the CLI test binary.
use std::cell::Cell;
use unityrs_batch::alloc::{Counting, Hook};
thread_local! {
    pub(crate) static LARGEST: Cell<Option<usize>> = const { Cell::new(None) };
    pub(crate) static REFUSE: Cell<bool> = const { Cell::new(false) };
}
struct Refusal;
impl Hook for Refusal {
    fn refuses(&self, size: usize) -> bool {
        let _ = LARGEST.try_with(|r| {
            if let Some(largest) = r.get() {
                r.set(Some(largest.max(size)));
            }
        });
        REFUSE.try_with(|r| r.replace(false)).unwrap_or(false)
    }
}
#[global_allocator]
static ALLOC: Counting<Refusal> = Counting(Refusal);
