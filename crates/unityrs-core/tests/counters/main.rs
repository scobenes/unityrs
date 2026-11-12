//! CR8's test-only counters (`docs/plans/batch.md` section 6, CR8;
//! `unityrs_core::testing`), built under the `test-counters` feature, which
//! CI's `cargo test -p unityrs-core -p unityrs-testkit --features
//! unityrs-testkit/test-counters` turns on. The counters and tables are the
//! process's, so each test runs its probe in a child process of its own
//! (`unityrs_testkit::child`), whose refusals, load ids and tables start
//! from nothing: [`refusals`] holds the refusal counts' tests, [`spans`]
//! the span reads', [`keys`] a bundle's key's.

#![cfg(feature = "test-counters")]
// Test code: a failed step is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::{BundleFile, File};
use unityrs_core::limits::{Meter, MeteredVec, heap_cost};
use unityrs_core::testing::{self, Refusals};
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::counted::{Allocator, Tally};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

mod keys;
mod refusals;
mod spans;

/// Set in a probe's child: a probe run any other way does nothing.
const PROBE: &str = "UNITYRS_CR8_PROBE";

/// Whether a test armed one refusal of an allocation inside the core's
/// marked `try_reserve_exact` ([`Arming`]).
static ARMED: AtomicBool = AtomicBool::new(false);

/// How many allocations the allocator saw inside a marked site.
static FALLIBLE: AtomicU64 = AtomicU64::new(0);

/// How many blocks were made on a counted thread
/// (`unityrs_testkit::counted::count_this_thread`).
static MADE: AtomicU64 = AtomicU64::new(0);

/// While set, each block freed on a counted thread reads the tables first
/// ([`testing::span_reads`]), as an engine's allocator may run any code
/// when a block is freed, and [`FREE_READS`] counts those reads.
static READ_ON_FREE: AtomicBool = AtomicBool::new(false);

/// How many reads [`READ_ON_FREE`] made.
static FREE_READS: AtomicU64 = AtomicU64::new(0);

/// A refusal armed as the engine's `point` hook arms one: the next
/// allocation made inside a marked site gets NULL, and no other allocation
/// is ever refused (one would abort the probe). It also counts the blocks
/// a counted thread makes, and reads the tables on a free while
/// [`READ_ON_FREE`] is set.
struct Arming;

impl Tally for Arming {
    fn refuses(&self, _size: usize) -> bool {
        if !testing::in_fallible_reserve() {
            return false;
        }
        FALLIBLE.fetch_add(1, Ordering::Relaxed);
        ARMED.swap(false, Ordering::AcqRel)
    }
    fn grew(&self, _size: usize, _zeroed: bool) {
        MADE.fetch_add(1, Ordering::Relaxed);
    }
    fn shrank(&self, _size: usize) {
        if READ_ON_FREE.load(Ordering::Acquire) {
            testing::span_reads(|_| ());
            FREE_READS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[global_allocator]
static ALLOC: Allocator<Arming> = Allocator(Arming);

/// Runs the ignored test `probe` (its path in this binary) in a child
/// process and checks that it ran and passed.
fn in_child(probe: &str) {
    let exe = std::env::current_exe().unwrap();
    let out = unityrs_testkit::child::output(
        Command::new(exe)
            .args(["--exact", probe, "--ignored", "--test-threads=1"])
            .env(PROBE, "1"),
    )
    .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "{probe}: {}\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Whether this is a probe's child.
fn probing() -> bool {
    std::env::var_os(PROBE).is_some()
}

fn src(name: &str, data: &[u8]) -> Source {
    Source::Bytes {
        name: RawStr::from(name),
        data: Bytes::copy_from_slice(data),
    }
}

/// A bundle of `CAB-<cab>` and its `.resS`.
fn cab_bundle(cab: &str) -> Vec<u8> {
    let name = format!("CAB-{cab}");
    bundle(&[
        (&name, serialized(&[], &[Obj::named(1, cab, &[])])),
        (&format!("{name}.resS"), res_bytes(64, 1)),
    ])
}

/// A bundle whose row 0 is `inner`, a bundle of its own.
fn nesting(inner: &[u8]) -> Vec<u8> {
    bundle(&[("inner", inner.to_vec())])
}

fn the_bundle(f: &File) -> &Arc<BundleFile> {
    match f {
        File::Bundle(b) => b,
        other => panic!("{other:?}"),
    }
}

fn nested(b: &BundleFile, row: usize) -> Arc<BundleFile> {
    the_bundle(&b.entry(row).unwrap()).clone()
}

/// One span-read slot: load id (`None` bare), path, whole, count.
type Slot = (Option<u64>, Vec<usize>, bool, u64);

/// One load-name entry: load id, name.
type Name = (u64, Vec<u8>);

/// The tables now: the slots, the names, and the overflow flag.
fn tables() -> (Vec<Slot>, Vec<Name>, bool) {
    testing::span_reads(|r| {
        let slots = r
            .reads()
            .map(|s| (s.load, s.path.to_vec(), s.whole, s.count))
            .collect();
        let names = r.names().map(|(id, n)| (id, n.to_vec())).collect();
        (slots, names, r.overflowed())
    })
}

/// The id the load named `name` drew.
fn id_of(name: &str) -> u64 {
    let (_, names, _) = tables();
    let ids: Vec<u64> = names
        .iter()
        .filter(|(_, n)| n == name.as_bytes())
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(ids.len(), 1, "{name}: {names:?}");
    ids[0]
}

fn row(load: Option<u64>, path: &[usize], count: u64) -> Slot {
    (load, path.to_vec(), false, count)
}

fn whole(load: Option<u64>, path: &[usize], count: u64) -> Slot {
    (load, path.to_vec(), true, count)
}

fn lazy() -> Environment {
    Environment::new(EnvOptions::batch(Arc::new(FileContext::unitypy())))
}

fn eager(ctx: Arc<FileContext>) -> Environment {
    let mut o = EnvOptions::unitypy();
    o.context = ctx;
    o.load = LoadMode::Eager;
    Environment::new(o)
}

/// `f`'s refusals.
fn counted(f: impl FnOnce()) -> Refusals {
    let before = testing::refusals();
    f();
    testing::refusals().since(&before)
}

/// A vector of 1,000 bytes, full, on `meter`: its next push asks for room
/// for 2,000, and then for 1,500.
fn full_vec(meter: &Arc<Meter>) -> MeteredVec<u8> {
    let mut v = MeteredVec::with_capacity(meter, 1000).unwrap();
    v.extend_from_slice(&[0; 1000]).unwrap();
    v
}

/// What [`full_vec`]'s growth to 1,500 holds while it moves.
fn room_for_1500() -> u64 {
    heap_cost(1000) + heap_cost(1500)
}

/// The test's dependency pool: once armed with `n`, refuses the `n`-th
/// dependency reservation alone, so the `with_room` around the load
/// retries it and the retry succeeds; counts every one.
#[derive(Debug, Default)]
struct Pool {
    asked: AtomicU64,
    n: AtomicU64,
}

impl MemoryBudget for Pool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Dependency {
            let asked = self.asked.fetch_add(1, Ordering::AcqRel) + 1;
            if asked == self.n.load(Ordering::Acquire) {
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
        }
        Ok(Permit::detached(bytes))
    }
}
