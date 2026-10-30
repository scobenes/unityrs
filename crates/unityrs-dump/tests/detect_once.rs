//! A bundle entry is detected once, by the core, whose detection of a gzip
//! entry inflates it as the open's counted work
//! (`unityrs_core::detect::check_entry_type`): the dump takes the entry's
//! type from the core (`File::file_type`) and never inflates it again
//! uncounted. A counting global allocator sees each inflation as the 64 KiB
//! zeroed scratch gzip detection inflates into, on the test's own thread
//! (`unityrs_testkit::counted`).

use std::io::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_dump::{DumpOptions, Levels};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

/// The size of gzip detection's scratch (`detect.rs`'s `SCRATCH`).
const SCRATCH: usize = 64 << 10;

struct Watching;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static INFLATIONS: AtomicUsize = AtomicUsize::new(0);

impl Tally for Watching {
    fn grew(&self, size: usize, zeroed: bool) {
        LIVE.fetch_add(size, Ordering::Relaxed);
        if zeroed && size == SCRATCH {
            INFLATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);

/// A gzip stream of `n` zero bytes, at least 128 bytes long (detection's
/// least length for a gzip entry).
fn gzip_of_zeros(n: usize) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&vec![0; n]).unwrap();
    let mut out = gz.finish().unwrap();
    out.resize(out.len().max(128), 0);
    out
}

#[test]
fn a_gzip_entry_is_inflated_once() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let gz = gzip_of_zeros(5000);
    let entries = [("a.gz", gz.clone()), ("b.gz", gz.clone()), ("c.gz", gz)];
    let data = unityrs_testkit::env::bundle(&entries);
    let opts = DumpOptions {
        levels: Levels::ALL,
        ..DumpOptions::default()
    };
    INFLATIONS.store(0, Ordering::Relaxed);
    let d = unityrs_dump::dump_input(b"gz-rows", data.into(), &opts);
    let inflations = INFLATIONS.load(Ordering::Relaxed);
    // The bundle and its three entries, each a ResourceFile.
    assert_eq!(d.files, 4, "{}", d.text());
    assert_eq!(d.text().matches("\"ResourceFile\"").count(), 3);
    assert_eq!(inflations, entries.len(), "one inflation per entry");
}
