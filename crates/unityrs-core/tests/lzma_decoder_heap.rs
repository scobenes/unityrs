//! What one LZMA decode holds on the C heap, against what it charges
//! (plan section 3.15; review round 12: liblzma's decoder state, about
//! 32 KiB a decode, reached no meter, and no memory test could see it,
//! since a counting global allocator sees only the Rust heap).
//!
//!     scripts/dev cargo test -p unityrs-core --test lzma_decoder_heap -- --nocapture
//!
//! glibc's own statistics see both heaps (`mallinfo2`: `uordblks`, the
//! arenas' chunks in use, and `hblkhd`, the mapped ones), but for the whole
//! process, so the probe runs in a child with one arena and no thread cache
//! (`tcache_count=0`: a chunk freed into it still counts as in use, and a
//! request it serves counts nothing), and with a fixed `mmap` threshold of
//! 128 KiB, the one [`heap_cost`] models (glibc raises its threshold after a
//! mapped chunk is freed, unless it is set). Linux and glibc 2.33 or later
//! only: `mallinfo2` is looked up when the test runs, so the test links on
//! any glibc (review round 13: it linked `mallinfo2`, and the test target
//! failed to link on glibc 2.28-2.32), says it skipped on an older one or
//! elsewhere, and fails, never skips, where glibc says it is 2.33 or later.
//!
//! Owner: package C.

#![allow(unsafe_code)]

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use unityrs_core::Bytes;
use unityrs_core::budget::{
    BudgetExceeded, Counting, DecodeBuf, MemoryBudget, Permit, Reservation,
};
use unityrs_core::codec::{BlockCodec, Lzma};
use unityrs_core::error::LimitKind;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_testkit::bundle::{TestCodec, compress};
use unityrs_testkit::stack::{Probe, classify};

const PROBE: &str = "UNITYRS_CORE_LZMA_DECODER_HEAP_PROBE";

/// The child's malloc settings (module docs).
const TUNABLES: &str = "glibc.malloc.tcache_count=0:glibc.malloc.mmap_threshold=131072";

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod heap {
    use std::ffi::{CStr, c_char, c_void};

    /// `struct mallinfo2` (glibc 2.33 and later).
    #[repr(C)]
    #[allow(dead_code)]
    pub struct Mallinfo2 {
        arena: usize,
        ordblks: usize,
        smblks: usize,
        hblks: usize,
        hblkhd: usize,
        usmblks: usize,
        fsmblks: usize,
        uordblks: usize,
        fordblks: usize,
        keepcost: usize,
    }

    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn gnu_get_libc_version() -> *const c_char;
    }

    /// glibc's `mallinfo2`, looked up in the running process
    /// (`RTLD_DEFAULT`, a null handle in glibc), if this glibc has it.
    fn mallinfo2() -> Option<unsafe extern "C" fn() -> Mallinfo2> {
        // SAFETY: a null handle and a NUL-terminated name; `dlsym` returns
        // the symbol's address or null.
        let f = unsafe { dlsym(std::ptr::null_mut(), c"mallinfo2".as_ptr()) };
        // SAFETY: glibc's `mallinfo2` takes no argument and returns its
        // struct by value (`malloc.h`), as this type says.
        (!f.is_null()).then(|| unsafe {
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> Mallinfo2>(f)
        })
    }

    /// The running glibc's version, `gnu_get_libc_version()`.
    pub fn version() -> String {
        // SAFETY: glibc returns a static NUL-terminated string.
        unsafe { CStr::from_ptr(gnu_get_libc_version()) }
            .to_string_lossy()
            .into_owned()
    }

    /// Whether the heap can be measured here: `Err` with the reason when
    /// this glibc has no `mallinfo2`, which only one before 2.33 may lack (a
    /// later one without it is a failure, not a skip: the test must run
    /// wherever it can).
    pub fn measurable() -> Result<(), String> {
        if mallinfo2().is_some() {
            return Ok(());
        }
        let v = version();
        let mut parts = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let at = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        assert!(
            at < (2, 33),
            "glibc {v} has mallinfo2 (2.33 and later), but dlsym did not find it"
        );
        Err(format!("glibc {v} has no mallinfo2 (it came in 2.33)"))
    }

    /// The bytes the process's heap holds, in glibc chunks: the arenas'
    /// chunks in use and the mapped ones.
    pub fn held() -> u64 {
        let f = mallinfo2().expect("mallinfo2 (checked by measurable())");
        // SAFETY: `mallinfo2` takes no argument, returns its struct by value
        // and allocates nothing.
        let m = unsafe { f() };
        u64::try_from(m.uordblks + m.hblkhd).unwrap()
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
mod heap {
    pub fn measurable() -> Result<(), String> {
        Err("no glibc heap statistics on this target".to_owned())
    }

    pub fn held() -> u64 {
        0
    }
}

/// At each reservation: the heap held and the bytes charged before it.
static SAMPLES: [[AtomicU64; 2]; 16] = [const { [AtomicU64::new(0), AtomicU64::new(0)] }; 16];
static TAKEN: AtomicUsize = AtomicUsize::new(0);

/// A budget that counts what is charged ([`Counting`]) and, at each
/// reservation, samples the heap and that count before it: the bytes to be
/// reserved are not allocated yet (a charge comes first), so the heap may
/// hold no more than what is charged. It allocates nothing itself.
#[derive(Debug)]
struct Sampling(Counting);

impl MemoryBudget for Sampling {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let i = TAKEN.fetch_add(1, Ordering::Relaxed);
        if let Some([heap, charged]) = SAMPLES.get(i) {
            heap.store(heap::held(), Ordering::Relaxed);
            charged.store(self.0.used(), Ordering::Relaxed);
        }
        self.0.try_reserve(bytes, what)
    }
}

/// The dictionary buffer liblzma allocates for a dictionary of `dict`
/// bytes, as `lz/lz_decoder.c:253-289` of xz 5.8.4 sizes it: at least
/// 4 KiB, rounded up to 16, and `2 * LZ_DICT_REPEAT_MAX` (576) more.
fn dictionary_buffer(dict: u32) -> u64 {
    u64::from(dict.max(4096)).next_multiple_of(16) + 576
}

/// What the `liblzma` crate's filter chain takes while it is built: a
/// one-entry array (16 bytes) grown to four (64), both held while it
/// grows, and one options node (128 bytes), in glibc chunks.
fn filter_chain() -> u64 {
    heap_cost(16) + heap_cost(64) + heap_cost(128)
}

/// Run by `liblzmas_decoder_is_charged`, in a child (module docs): for
/// dictionaries named from 0 (liblzma takes 4 KiB) to 8 MiB, one at a size
/// that is no multiple of 16 and one whose buffer crosses a page its size
/// alone does not, a block of 23 bytes declared that size (so the first
/// attempt's dictionary is the named one, `Lzma`'s docs) is decoded on a
/// meter whose budget samples the heap at each reservation. At none does
/// the heap hold more than is charged; at the first window's (the second
/// reservation: the attempt's charge is the first), when the decoder and
/// its dictionary are alive, the charge exceeds the heap by at most the 32
/// bytes of the filter array's first chunk, which its growth freed. Then
/// each part is measured through liblzma's API, as `lzma_attempt` calls it:
/// what liblzma allocates for the decoder (in this child each of its
/// requests takes exactly the chunk [`heap_cost`] gives it, nothing else
/// allocating between) and the filter chain's peak are exactly
/// [`Lzma::decoder_charge`], and liblzma's own state, beside its dictionary
/// buffer, the same at every size.
#[test]
#[ignore = "run by liblzmas_decoder_is_charged"]
fn lzma_decoder_heap_probe() {
    if std::env::var(PROBE).is_err() {
        return;
    }
    heap::measurable().unwrap();
    // The settings took: a freed small chunk is no longer counted in use
    // (in a thread cache it would be).
    let before = heap::held();
    let chunk = std::hint::black_box(Box::new([7u8; 48]));
    let during = heap::held();
    drop(chunk);
    assert!(
        during > before && heap::held() == before,
        "the child's heap is not measurable: {before} {during} {} (GLIBC_TUNABLES {TUNABLES})",
        heap::held()
    );
    let text = b"hello hello hello hello";
    let stream = compress(TestCodec::Lzma, text);
    for named in [0_u32, 4096, 5000, 65_536, 1_052_564, 8 << 20] {
        let mut block = stream.clone();
        block[1..5].copy_from_slice(&named.to_le_bytes());
        let input = Bytes::from(block);
        let budget = Arc::new(Sampling(Counting::new(u64::MAX)));
        let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::BlockOutput).unwrap();
        let declared = named.max(23);
        TAKEN.store(0, Ordering::Relaxed);
        let (heap0, charged0) = (heap::held(), budget.0.used());
        let got = Lzma.decode(&input, declared, &mut out);
        let taken = TAKEN.load(Ordering::Relaxed);
        assert!(got.is_ok(), "{named}: {got:?}");
        assert_eq!(out.as_slice(), text, "{named}");
        assert!(
            (2..=SAMPLES.len()).contains(&taken),
            "{named}: {taken} reservations"
        );
        let at = |i: usize| {
            let [heap, charged] = &SAMPLES[i];
            (
                heap.load(Ordering::Relaxed).saturating_sub(heap0),
                charged.load(Ordering::Relaxed).saturating_sub(charged0),
            )
        };
        for i in 0..taken {
            let (heap, charged) = at(i);
            assert!(
                heap <= charged,
                "{named}: reservation {i}: the heap holds {heap} bytes, {} more than the \
                 {charged} charged",
                heap - charged
            );
        }
        let (heap, charged) = at(1);
        assert!(
            charged - heap <= 32,
            "{named}: the decoder holds {heap} bytes, {} less than its charge, {charged}",
            charged - heap
        );
        println!(
            "dictionary {named}: one decoder holds {heap} bytes and is charged {charged} \
             ({} its dictionary buffer)",
            heap_cost(dictionary_buffer(named))
        );
    }
    // liblzma's own part, through the API `lzma_attempt` calls.
    for named in [0_u32, 4096, 5000, 65_536, 1_052_564, 8 << 20] {
        use liblzma::stream::{Filters, LzmaOptions, Stream};
        let start = heap::held();
        let mut opts = LzmaOptions::new_preset(6).unwrap();
        opts.dict_size(named)
            .literal_context_bits(3)
            .literal_position_bits(0)
            .position_bits(2);
        let mut filters = Filters::new();
        filters.lzma1(&opts);
        let built = heap::held();
        let decoder = Stream::new_raw_decoder(&filters).unwrap();
        let liblzma = heap::held() - built;
        drop(decoder);
        drop(filters);
        let chain = built - start;
        assert!(
            chain <= filter_chain(),
            "{named}: the filter chain holds {chain} bytes"
        );
        assert_eq!(
            liblzma + filter_chain(),
            Lzma::decoder_charge(named),
            "{named}: liblzma allocates {liblzma} bytes for the decoder"
        );
        let state = liblzma - heap_cost(dictionary_buffer(named));
        assert_eq!(
            state + filter_chain(),
            Lzma::DECODER_STATE,
            "{named}: liblzma's decoder holds {state} bytes beside its dictionary buffer"
        );
        println!(
            "dictionary {named}: liblzma's state {state} bytes; the filter chain {chain} held, \
             {} while it grows",
            filter_chain()
        );
    }
}

/// Review round 12: the C heap one LZMA decode holds, liblzma's state and
/// its dictionary buffer as liblzma sizes it, is charged before it is
/// allocated, with at most 32 bytes to spare (`lzma_decoder_heap_probe`).
#[test]
fn liblzmas_decoder_is_charged() {
    if let Err(why) = heap::measurable() {
        eprintln!("skipped: {why}");
        return;
    }
    let exe = std::env::current_exe().expect("the test binary's path");
    let out = unityrs_testkit::child::output(
        Command::new(exe)
            .args([
                "--exact",
                "lzma_decoder_heap_probe",
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .env("MALLOC_ARENA_MAX", "1")
            .env("GLIBC_TUNABLES", TUNABLES),
    )
    .expect("the probe starts");
    match classify(out.status, &out.stdout, &out.stderr) {
        Ok(Probe::Fits) => {}
        Ok(Probe::Overflows) => panic!("the probe overflowed a stack"),
        Err(e) => panic!("{e}"),
    }
    // libtest prints the probe's first line after the test's name.
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(at) = line.find("dictionary ") {
            println!("{}", &line[at..]);
        }
    }
}
