//! Test-hooks-only CLI bridge. All Rust allocations in this process
//! are engine/CLI allocations; corpus readers, drains and peers are OTHER
//! processes. Keep the ledger live from process initialization: late frees of
//! pre-main allocations then balance, and parent-side spawn costs are included.
//! No registration filtering, progress sampling, live-cap deduction or history.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use unityrs_batch::alloc::Hook;
use unityrs_batch::run::hooks::memory::{self, Observer, Point};
use unityrs_core::limits::heap_cost;
use unityrs_testkit::counted::Tally;
use unityrs_testkit::ledger::Ledger;

static WHOLE: Ledger = Ledger::new(heap_cost);
static HELD: Ledger = Ledger::new(heap_cost);
static ENABLED: AtomicBool = AtomicBool::new(false);
static REPORTED: AtomicBool = AtomicBool::new(false);
static PHASES: AtomicU64 = AtomicU64::new(0);
static WORKERS: AtomicU64 = AtomicU64::new(0);
static DC: AtomicU64 = AtomicU64::new(0);
static NETWORK: AtomicU64 = AtomicU64::new(0);

pub struct Bridge;
impl Hook for Bridge {
    fn growing(&self, size: usize) {
        WHOLE.growing(size);
        HELD.growing(size);
    }
    fn failed(&self, size: usize) {
        WHOLE.failed(size);
        HELD.failed(size);
    }
    fn shrank(&self, size: usize) {
        WHOLE.shrank(size);
        HELD.shrank(size);
    }
    fn shrunk(&self, old: usize, new: usize) {
        WHOLE.shrunk(old, new);
        HELD.shrunk(old, new);
    }
}

/// The environment and hook setup allocate ordinary bounded CLI ownership.
/// That heap is retained in the maxima, never subtracted. Ledger callbacks
/// themselves only use the testkit's fixed atomic state.
pub fn install() -> bool {
    if std::env::var_os("UNITYRS_TEST_WHOLE_MEMORY").is_none_or(|v| v != "1") {
        return true;
    }
    if unityrs_batch::sys::allocator(1).is_err() {
        return false;
    }
    ENABLED.store(true, SeqCst);
    memory::install(observer()).is_ok()
}

fn observer() -> Observer {
    Observer {
        charge: |bytes, held| {
            if held {
                HELD.charge(bytes);
            } else {
                HELD.release(bytes);
            }
        },
        network: |bytes, held| {
            if held {
                NETWORK.fetch_add(bytes, SeqCst);
            } else {
                NETWORK.fetch_sub(bytes, SeqCst);
            }
        },
        point,
    }
}

pub fn configure(workers: u32, dc: u32) {
    WORKERS.store(u64::from(workers), SeqCst);
    DC.store(u64::from(dc), SeqCst);
}

fn point(point: Point) {
    match point {
        Point::Phase(phase) => {
            PHASES.fetch_or(1 << phase as u32, SeqCst);
        }
        Point::Exit => report(),
        Point::SortMerge => {}
    }
}

// Stack formatting only. No format!/String/Vec, paths, input identities,
// heap walks, file opening, resolver work, locks held across I/O, or callbacks
// into accounts. The regular private stderr file is opened by the parent.
struct Line {
    bytes: [u8; 512],
    used: usize,
}
impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; 512],
            used: 0,
        }
    }
    fn text(&mut self, text: &[u8]) {
        if let Some(end) = self.used.checked_add(text.len())
            && let Some(dst) = self.bytes.get_mut(self.used..end)
        {
            dst.copy_from_slice(text);
            self.used = end;
        }
    }
    fn number(&mut self, mut number: u64) {
        let mut digits = [0_u8; 20];
        let mut first = digits.len();
        loop {
            first -= 1;
            if let Some(digit) = digits.get_mut(first) {
                *digit = b'0' + u8::try_from(number % 10).unwrap_or(0);
            }
            number /= 10;
            if number == 0 {
                break;
            }
        }
        if let Some(text) = digits.get(first..) {
            self.text(text);
        }
    }
}

fn report() {
    report_to(|bytes| {
        let _ = unityrs_batch::sys::testing::write_stderr(bytes);
    });
}

fn report_to(write: impl FnOnce(&[u8])) {
    if !ENABLED.load(SeqCst) || REPORTED.swap(true, SeqCst) {
        return;
    }
    let whole = WHOLE.observe();
    let held = HELD.observe();
    let Ok(peak) = u64::try_from(whole.peak_above_arm) else {
        return; // Missing line makes the evidence ineligible.
    };
    let workers = WORKERS.load(SeqCst);
    let dc = DC.load(SeqCst);
    let mut line = Line::new();
    // Only the canonical one-worker, one-download baseline is a c_base probe.
    // Eight-worker peaks are validation observations, never a fitted slope.
    if workers == 1 && dc == 1 {
        line.text(b"B12_COEFFICIENT c_base ");
        line.number(peak);
        line.text(b"\n");
    }
    // Separate closed private protocol, deliberately not a B12_* extension.
    // Signed fields stay private, encoded as sign (0/1) plus magnitude.
    line.text(b"WHOLE_MEMORY_V1");
    for value in [
        peak,
        whole.events,
        PHASES.load(SeqCst),
        workers,
        dc,
        NETWORK.load(SeqCst),
        u64::from(held.heap < 0),
        held.heap.unsigned_abs(),
        u64::from(held.charged < 0),
        held.charged.unsigned_abs(),
        u64::from(held.max_residual < 0),
        held.max_residual.unsigned_abs(),
    ] {
        line.text(b" ");
        line.number(value);
    }
    line.text(b"\n");
    if let Some(bytes) = line.bytes.get(..line.used) {
        write(bytes);
    }
}

#[cfg(test)]
mod tests;
