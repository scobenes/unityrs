//! What the phase-1 measurements share (`measure.rs`, `slowest.rs`): a
//! counting allocator that counts the test's thread, in glibc chunks
//! (`heap_cost`), the head of a spec, short ids, and a measured row.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use unityrs_batch::spec::{self, MAX_SPEC_BYTES};
use unityrs_core::limits::heap_cost;
use unityrs_testkit::counted::{Allocator, Tally};
use unityrs_testkit::scratch::Scratch;

pub static LIVE: AtomicUsize = AtomicUsize::new(0);
pub static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Peak;

impl Tally for Peak {
    fn grew(&self, size: usize, _zeroed: bool) {
        let size = cost(size);
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(now, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(cost(size), Ordering::Relaxed);
    }
}

/// A block's glibc chunk, the unit `C_spec` and `p` are counted in
/// (`docs/plans/batch.md` 2.7).
fn cost(size: usize) -> usize {
    usize::try_from(heap_cost(size as u64)).unwrap_or(usize::MAX)
}

#[global_allocator]
static ALLOC: Allocator<Peak> = Allocator(Peak);

pub const HEAD: &str = r#"{"spec_version":1,"run_id":"m","attempt":1,"threads":8,"download_concurrency":8,
"max_inflight_uncompressed_bytes":1099511627776,"max_decoded_bytes":1073741824,
"dep_cache_bytes":4294967296,"max_view_bundles":65536,
"spool":{"dir":"/spool","size_bytes":1099511627776,"reserve_bytes":0},
"outputs":{"textures":{"key":"{input}/{name}.png"}},"sink":{"kind":"directory","root":"/out"},"#;

/// Base-62 ids, one to three bytes.
pub fn short_id(mut i: usize) -> String {
    const A: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut s = Vec::new();
    loop {
        s.push(A[i % 62]);
        i /= 62;
        if i == 0 {
            break;
        }
        i -= 1;
    }
    String::from_utf8(s).unwrap()
}

pub struct Measure {
    pub bytes: usize,
    pub secs: f64,
    pub peak: usize,
    pub kept: usize,
}

/// `prefix`, then `item(i)` for each `i` joined by `,`, then `suffix`, as
/// long as the whole fits 64 MiB.
pub fn fill(prefix: &str, item: impl Fn(usize) -> String, suffix: &str) -> String {
    let cap = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits");
    let mut s = String::from(prefix);
    let mut i = 0;
    loop {
        let it = item(i);
        if s.len() + it.len() + 1 + suffix.len() > cap {
            break;
        }
        if i > 0 {
            s.push(',');
        }
        s.push_str(&it);
        i += 1;
    }
    s.push_str(suffix);
    s
}

/// Phase 1 on `text`, written to `dir`, with the views' budget
/// `budget`: its time, peak and kept heap, and its outcome.
pub fn measure_at(dir: &Scratch, name: &str, text: &str, budget: u64) -> (Measure, String) {
    assert!(
        text.len() as u64 <= MAX_SPEC_BYTES,
        "{name}: {} bytes",
        text.len()
    );
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, text).unwrap();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let t = Instant::now();
    let r = spec::read::read_job(&path).and_then(|b| spec::parse_with(b, budget));
    let secs = t.elapsed().as_secs_f64();
    let peak = PEAK.load(Ordering::Relaxed) - base;
    let kept = LIVE.load(Ordering::Relaxed) - base;
    let outcome = match &r {
        Ok(_) => "accepted".to_owned(),
        Err(e) => e.detail(),
    };
    drop(r);
    (
        Measure {
            bytes: text.len(),
            secs,
            peak,
            kept,
        },
        outcome,
    )
}

/// One printed row: shape, budget, outcome, size, time, peak, kept, in a
/// form a script turns into a table.
pub fn row(shape: &str, budget: u64, outcome: &str, m: &Measure) {
    #[allow(clippy::cast_precision_loss)]
    let mib = |b: usize| b as f64 / f64::from(1 << 20);
    println!(
        "ROW | {shape} | 2^{} | {outcome} | {:.1} | {:.3} | {:.1} | {:.1}",
        budget.ilog2(),
        mib(m.bytes),
        m.secs,
        mib(m.peak),
        mib(m.kept)
    );
    assert!(
        m.peak < 512 << 20,
        "{shape}: phase 1 peaked at {:.1} MiB, over 512",
        mib(m.peak)
    );
}
