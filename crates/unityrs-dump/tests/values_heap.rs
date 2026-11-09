//! What `unityrs values` holds for a case is charged to the case's meter
//! before it is allocated, and returned before the next case runs
//! (`unityrs_dump::values`, "Memory"; review round 15: the read ran on an
//! unlimited meter, the value's text, the resave's copy, its hex and the
//! line were plain `String`s and `Vec`s, and every line was held until the
//! last case had run).
//!
//! A counting global allocator watches the test thread's heap while
//! [`run_cases`] runs, in glibc chunks (`heap_cost`, the unit the meter
//! charges), and takes the peak of the heap above what the cases' meters
//! have charged, and the peak of what they have charged. A case of 16
//! structs, about 1 KiB of text each, and one of 16 bytes have the same
//! peak above the charge as a case of 4,096 structs and one of 1 MiB, so
//! nothing a struct, a byte of text or a byte read or resaved costs goes
//! uncharged; and two large cases run one after the other charge at their
//! peak what one does. A case whose resave fails quoting a name of 2^18
//! `é`s has the same peak above the charge as one whose name is 64 (review
//! round 16: that text was built whole, uncharged).
//!
//! It also refuses the one request a test arms: a case's bytes, its rows or
//! a name the allocator refuses is that case's `crash`, never an abort.
//!
//! The allocator counts only the measuring thread
//! (`unityrs_testkit::counted`): libtest's main thread allocates beside it.
//! Three tests measure, and the counts are one set for the file, so each
//! measures on a thread of its own, one at a time ([`counting`]).

use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use unityrs_core::budget::{Counting, MemoryBudget};
use unityrs_core::limits::{Limits, heap_cost};
use unityrs_core::typetree::NodeRow;
use unityrs_dump::record::NOTE_MAX;
use unityrs_dump::values::{CRASH, Case, run_case_on, run_cases};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

/// Live heap on the test thread, in glibc chunks.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The heap above the meters' charge at its peak, while measuring.
static PEAK_ABOVE: AtomicI64 = AtomicI64::new(i64::MIN);
/// What the budget has booked, at its peak, while measuring.
static PEAK_USED: AtomicI64 = AtomicI64::new(i64::MIN);
/// The live heap less the budget's bookings when the measure began.
static BASE: AtomicI64 = AtomicI64::new(0);
/// The budget every case's meter books with.
static BUDGET: OnceLock<Arc<Counting>> = OnceLock::new();
std::thread_local! {
    /// A request size the allocator refuses on this thread, once (0: none).
    static REFUSE: Cell<usize> = const { Cell::new(0) };
}

fn chunk(n: usize) -> usize {
    usize::try_from(heap_cost(n as u64)).unwrap_or(usize::MAX)
}

fn signed(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The heap above the charge now, and the charge. It allocates nothing.
fn note_peak() {
    if let Some(budget) = BUDGET.get() {
        let live = signed(LIVE.load(Ordering::Relaxed) as u64);
        let used = signed(budget.used());
        PEAK_ABOVE.fetch_max(
            live - used - BASE.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        PEAK_USED.fetch_max(used, Ordering::Relaxed);
    }
}

/// The test thread's live heap in chunks, its peaks, and the one fresh
/// allocation it armed refused.
struct Watching;

impl Tally for Watching {
    fn refuses(&self, size: usize) -> bool {
        size != 0
            && REFUSE
                .try_with(|r| r.get() == size && r.replace(0) == size)
                .unwrap_or(false)
    }

    /// A `realloc` is never refused: only the fresh allocations a test arms.
    fn refuses_realloc(&self, _new_size: usize) -> bool {
        false
    }

    fn grew(&self, size: usize, _zeroed: bool) {
        LIVE.fetch_add(chunk(size), Ordering::Relaxed);
        note_peak();
    }

    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(chunk(size), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);

/// `count` structs of one field, an empty struct with a 1,000-byte name, in
/// 4 bytes: about 1 KiB of text a struct.
fn structs(count: i32) -> Case {
    Case {
        name: format!("structs{count}"),
        big_endian: false,
        nodes: vec![
            (0, "Base".into(), "Base".into(), 0),
            (1, "vector".into(), "f".into(), 0),
            (2, "Array".into(), "Array".into(), 0),
            (3, "int".into(), "size".into(), 0),
            (3, "S".into(), "data".into(), 0),
            (4, "E".into(), "n".repeat(1000), 0),
        ],
        data: count.to_le_bytes().to_vec(),
    }
}

/// A `vector<UInt8>` of `len` bytes: re-saved, its hex twice that.
fn bytes(len: usize) -> Case {
    let mut data = i32::try_from(len).unwrap().to_le_bytes().to_vec();
    data.resize(4 + len, 0x5A);
    Case {
        name: format!("bytes{len}"),
        big_endian: false,
        nodes: vec![
            (0, "Base".into(), "Base".into(), 0),
            (1, "vector".into(), "v".into(), 0),
            (2, "Array".into(), "Array".into(), 0),
            (3, "int".into(), "size".into(), 0),
            (3, "UInt8".into(), "data".into(), 0),
        ],
        data,
    }
}

/// [`run_cases`] over `cases`, every meter booking with [`BUDGET`]: the
/// lines, and the heap above the charge and the charge, each at its peak.
fn measured(cases: &[Case]) -> (Vec<u8>, i64, i64) {
    let budget = BUDGET.get_or_init(|| Arc::new(Counting::new(u64::MAX)));
    let dynamic: Arc<dyn MemoryBudget> = budget.clone();
    let limits = Limits::default();
    // Room for every line, made before the measure begins: what the
    // lines are written to is the caller's (stdout, in the binary).
    let mut out = Vec::with_capacity(16 << 20);
    BASE.store(
        signed(LIVE.load(Ordering::Relaxed) as u64) - signed(budget.used()),
        Ordering::Relaxed,
    );
    PEAK_ABOVE.store(0, Ordering::Relaxed);
    PEAK_USED.store(0, Ordering::Relaxed);
    run_cases(cases, &limits, &dynamic, &mut out).unwrap();
    let above = PEAK_ABOVE.swap(i64::MIN, Ordering::Relaxed);
    let used = PEAK_USED.swap(i64::MIN, Ordering::Relaxed);
    assert!(out.len() < 16 << 20, "the lines outgrew their room");
    (out, above, used)
}

/// Each line read back: every case read, encoded and re-saved whole.
fn whole(out: &[u8], cases: usize) {
    let lines: Vec<serde_json::Value> = out
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    assert_eq!(lines.len(), cases);
    for l in lines {
        assert_eq!(
            (&l["kind"], &l["resave"]),
            (&serde_json::Value::Null, &serde_json::json!("equal")),
            "{}",
            l["name"]
        );
        assert!(l["value"].is_string() && l["resave_hex"].is_string());
    }
}

/// One measuring test at a time ([`counting`]).
static COUNTING: Mutex<()> = Mutex::new(());

/// `f` on a thread of its own whose allocations are counted, while no
/// other test measures (`staging.rs`'s rule).
fn counting<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    let _one = COUNTING.lock().unwrap_or_else(PoisonError::into_inner);
    std::thread::scope(|s| {
        s.spawn(|| {
            count_this_thread();
            others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
            f()
        })
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[test]
fn a_case_holds_nothing_uncharged_however_big_its_value() {
    counting(|| {
        let (small, at_small, _) = measured(&[structs(16), bytes(16)]);
        whole(&small, 2);
        let (large, at_large, _) = measured(&[structs(4096), bytes(1 << 20)]);
        whole(&large, 2);
        assert!(large.len() > 6 << 20, "{}", large.len());
        eprintln!("heap above the charge at most {at_small} bytes small, {at_large} large");
        // The same constant, give or take one allocation's rounding.
        assert!(
            at_large - at_small <= 64,
            "{at_small} bytes above the charge small, {at_large} large"
        );
    });
}

#[test]
fn what_a_case_held_is_returned_before_the_next_runs() {
    counting(|| {
        let (_, _, one) = measured(&[structs(4096)]);
        let (out, _, two) = measured(&[structs(4096), structs(4096)]);
        whole(&out, 2);
        eprintln!("charged at the peak: {one} bytes for one case, {two} for two");
        assert!(one > 4 << 20, "{one}");
        assert!(
            two - one <= 64,
            "{one} bytes charged for one case, {two} for two"
        );
    });
}

/// A case read whole whose resave fails, the error quoting a name of `n`
/// `é`s. UnityPyBoost compares a node's type as a C string
/// (`TypeTreeHelper.cpp:1112`, `:1121`), so a `string\0` reads its 4 zero
/// bytes as an empty string; UnityPy's writer looks the type up whole
/// (`helpers/TypeTreeHelper.py:416`), takes the node for a class and asks
/// the string for its child's name (`:454-455`). `value_cases.py`'s
/// `oracle` gives kind null, the value `{"a": ""}`, no resave bytes and
/// `error: AttributeError`; unityrs's write error quotes the child's name,
/// four characters for each byte of `é`.
fn refused_resave(n: usize) -> Case {
    Case {
        name: format!("refused{n}"),
        big_endian: false,
        nodes: vec![
            (0, "Base".into(), "Base".into(), 0),
            (1, "string\0".into(), "a".into(), 0),
            (2, "int".into(), "é".repeat(n), 0),
        ],
        data: vec![0; 4],
    }
}

/// Review round 16: a resave's error text was built whole, uncharged, in a
/// plain `String`, so a case of one long name aborted the run when the
/// allocator refused it. Its text is the note (`record::note_of`): a name
/// of 2^18 `é`s holds no more above the charge than one of 64 (whose text
/// is cut as well), and the case after it is answered.
#[test]
fn a_refused_resave_holds_a_note_whatever_its_names_length() {
    counting(|| {
        let mut peaks = Vec::new();
        for n in [64, 1 << 18] {
            let (out, above, _) = measured(&[refused_resave(n), bytes(16)]);
            let lines: Vec<serde_json::Value> = out
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .map(|l| serde_json::from_slice(l).unwrap())
                .collect();
            assert_eq!(lines.len(), 2, "{n}: every case is answered");
            let refused = &lines[0];
            assert_eq!(
                (&refused["kind"], &refused["value"], &refused["resave_hex"]),
                (
                    &serde_json::Value::Null,
                    &serde_json::json!(r#"{"t":"dict","v":[["a",""]]}"#),
                    &serde_json::Value::Null
                ),
                "{n}"
            );
            let resave = refused["resave"].as_str().unwrap();
            let chars = resave.chars().count();
            assert!(
                resave.starts_with("error: ") && chars <= "error: ".len() + NOTE_MAX,
                "{n}: a resave of {chars} characters: {}",
                resave.get(..80).unwrap_or(resave)
            );
            assert_eq!(lines[1]["resave"], "equal", "{n}: the next case");
            peaks.push(above);
        }
        eprintln!("heap above the charge at most {peaks:?} bytes");
        assert!(
            peaks[1] - peaks[0] <= 64,
            "{} bytes above the charge for a name of 64, {} for 2^18",
            peaks[0],
            peaks[1]
        );
    });
}

/// Each allocation of a case's own that asks for its whole size at once,
/// refused by the allocator: the case's `crash`, the process alive to say so
/// and to run the next. An infallible allocation here (the bytes copied with
/// `clone`, the rows collected into a `Vec`) aborts this test's process.
#[test]
fn what_the_allocator_refuses_a_case_is_its_crash() {
    counting(|| {
        let budget: Arc<dyn MemoryBudget> = Arc::new(Counting::new(u64::MAX));
        let limits = Limits::default();
        let (structs, bytes) = (structs(1), bytes(1 << 20));
        for (what, case, size) in [
            ("its bytes", &bytes, 4 + (1 << 20)),
            ("its rows", &structs, 6 * size_of::<NodeRow>()),
            ("a name", &structs, 1000),
        ] {
            REFUSE.with(|r| r.set(size));
            let o = run_case_on(case, &limits, &budget);
            assert_eq!(REFUSE.with(Cell::get), 0, "nothing asked for {what}");
            assert_eq!(o.kind, Some(CRASH), "{what}: {o:?}");
            let again = run_case_on(case, &limits, &budget);
            assert_eq!(
                (again.kind, again.resave.as_str()),
                (None, "equal"),
                "{what}"
            );
        }
    });
}
