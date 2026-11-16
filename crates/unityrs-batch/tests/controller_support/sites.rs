//! The url sweep's sites (`controller_alloc.rs`): an allocation named by
//! its call stack, not its ordinal. A url scope's allocation count follows
//! its peer's timing (how a head or a body splits across reads, where a
//! stop lands, which read meets a cut), so one ordinal names different
//! allocations in different children, and a new allocation could fall
//! past the ordinals its children reach. A stack names the same code path
//! in every child, whatever came before it.
//!
//! The census child lists each key with its count, and a child aimed at a
//! key refuses its nth allocation, or meets fewer and is unreached (a path
//! that run's timing skipped). Then the scope settles: one child at a time
//! refuses the first allocation whose key no child has refused yet, until
//! a child runs through its armed span meeting none. Every allocation of
//! that last run was refused in some child, so a new allocation on a path
//! every run takes is refused, however the reads fall.
use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering::SeqCst},
};
#[path = "site_diagnostics.rs"]
mod diagnostics;
pub use diagnostics::{census_only, diagnose};

/// Return addresses a key reads, from the hook's own frames down.
const DEPTH: usize = 64;
/// What a child does with each armed allocation's key: nothing (an
/// ordinal scope's), count it, refuse the nth of one, or refuse the first
/// beyond the covered keys.
static MODE: AtomicU8 = AtomicU8::new(OFF);
const OFF: u8 = 0;
const CENSUS: u8 = 1;
const AIMED: u8 = 2;
const BEYOND: u8 = 3;
static TARGET: AtomicU64 = AtomicU64::new(0);
static NTH: AtomicUsize = AtomicUsize::new(0);
/// The target's occurrences so far; past the covered keys, the refusals.
static MET: AtomicUsize = AtomicUsize::new(0);
/// The census: each key in the order first seen, with its count.
static COUNTS: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());
/// The keys some child refused, sorted.
static COVERED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The calling thread's return addresses, innermost first: how many it
/// filled.
fn walk(frames: &mut [*mut libc::c_void; DEPTH]) -> usize {
    // SAFETY: `frames` holds `DEPTH` pointers for `backtrace` to fill.
    let filled = unsafe { libc::backtrace(frames.as_mut_ptr(), DEPTH.try_into().unwrap_or(0)) };
    usize::try_from(filled).unwrap_or(0)
}

/// The calling allocation's key: each return address on its stack, as an
/// offset into the object that holds it (one binary, at any load
/// address), hashed (FNV-1a). Neither `backtrace` nor `dladdr` allocates
/// through Rust's allocator; the caller's tracing flag passes any
/// allocation they make.
fn key() -> (u64, [*mut libc::c_void; DEPTH], usize) {
    let mut frames = [std::ptr::null_mut(); DEPTH];
    let filled = walk(&mut frames);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &address in &frames[..filled] {
        let mut info = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
        // SAFETY: `info` is writable storage for one `Dl_info`, which
        // `dladdr` fills when it returns nonzero.
        let base = if unsafe { libc::dladdr(address, info.as_mut_ptr()) } == 0 {
            0
        } else {
            // SAFETY: `dladdr` filled it.
            unsafe { info.assume_init() }.dli_fbase as usize
        };
        for byte in (address as usize).wrapping_sub(base).to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    (hash, frames, filled)
}

/// Keys a url scope's child, before its arm, from its environment: aimed
/// at a key's nth allocation (`aim`), past the covered keys (`covered`),
/// or with neither, its census.
pub fn aim(aim: Option<&str>, covered: Option<&str>) {
    // The first walk loads the unwinder, before any armed one; with no
    // stack to walk, every allocation would share one key.
    assert!(
        walk(&mut [std::ptr::null_mut(); DEPTH]) > 1,
        "no stack walk: sites have no keys"
    );
    MET.store(0, SeqCst);
    let mode = match (aim, covered) {
        (Some(aim), None) => {
            let (key, nth) = aim.split_once(':').unwrap();
            TARGET.store(u64::from_str_radix(key, 16).unwrap(), SeqCst);
            NTH.store(nth.parse().unwrap(), SeqCst);
            AIMED
        }
        (None, Some(covered)) => {
            let mut keys: Vec<u64> = covered
                .split(',')
                .filter(|k| !k.is_empty())
                .map(|k| u64::from_str_radix(k, 16).unwrap())
                .collect();
            keys.sort_unstable();
            *COVERED.lock().unwrap() = keys;
            NTH.store(1, SeqCst);
            BEYOND
        }
        (None, None) => CENSUS,
        (Some(_), Some(_)) => panic!("a child is aimed or settling, not both"),
    };
    MODE.store(mode, SeqCst);
}

/// Whether the child's sites are keys: a url scope's.
pub fn keyed() -> bool {
    MODE.load(SeqCst) != OFF
}

/// Called by the hook for each armed allocation, its tracing flag set:
/// whether this is the one the child refuses. The census counts it
/// instead.
pub fn refuses() -> bool {
    let (key, frames, filled) = key();
    match MODE.load(SeqCst) {
        AIMED => key == TARGET.load(SeqCst) && MET.fetch_add(1, SeqCst) + 1 == NTH.load(SeqCst),
        BEYOND => {
            let covered = COVERED.lock().unwrap_or_else(PoisonError::into_inner);
            let refused = covered.binary_search(&key).is_err()
                && MET.compare_exchange(0, 1, SeqCst, SeqCst).is_ok();
            if refused {
                eprintln!("CONTROLLER_BEYOND key={key:016x}");
                diagnostics::record(key, &frames[..filled]);
            }
            refused
        }
        _ => {
            let mut counts = COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, count)) = counts.iter_mut().find(|(k, _)| *k == key) {
                *count += 1;
            } else {
                counts.push((key, 1));
                diagnostics::record(key, &frames[..filled]);
            }
            false
        }
    }
}

/// Whether the child refused what it was aimed at.
pub fn met() -> bool {
    MET.load(SeqCst) >= NTH.load(SeqCst)
}

/// The census child's report, for [`census`].
pub fn report(kind: &str) {
    for (key, count) in COUNTS.lock().unwrap().iter() {
        println!("CONTROLLER_SITE kind={kind} key={key:016x} count={count}");
    }
}

/// The census child's sites, in the order first seen: each key with its
/// count.
pub fn census(kind: &str, stdout: &str) -> Vec<(u64, usize)> {
    let prefix = format!("CONTROLLER_SITE kind={kind} key=");
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .map(|rest| {
            let (key, count) = rest.split_once(" count=").unwrap();
            (
                u64::from_str_radix(key, 16).unwrap(),
                count.parse().unwrap(),
            )
        })
        .collect()
}

/// A child's aim at a key's `nth` allocation, through its environment.
pub fn aimed((key, nth): (u64, usize)) -> (&'static str, String) {
    ("UNITYRS_CONTROLLER_AIM", format!("{key:016x}:{nth}"))
}

/// A settling child's covered keys, through its environment.
pub fn beyond(covered: &std::collections::BTreeSet<u64>) -> (&'static str, String) {
    let keys: Vec<String> = covered.iter().map(|key| format!("{key:016x}")).collect();
    ("UNITYRS_CONTROLLER_COVERED", keys.join(","))
}

/// The key a settling child refused, from its stderr.
pub fn refused(stderr: &str) -> Option<u64> {
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix("CONTROLLER_BEYOND key="))?;
    u64::from_str_radix(line, 16).ok()
}
