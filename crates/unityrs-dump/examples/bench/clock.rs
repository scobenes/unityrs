//! The bench's clocks and probes: each thread's CPU time, the decode time a
//! wrapped codec counts, and the process's peak RSS.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded};
use unityrs_core::{Bytes, Error};

/// The calling thread's CPU time in nanoseconds: the first field of
/// `/proc/thread-self/schedstat` (time on a CPU). `None` where the kernel
/// does not give it, and the bench then times by the wall clock
/// ([`Clock`]).
pub fn thread_cpu_ns() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    text.split_whitespace().next()?.parse().ok()
}

/// The process's peak resident set (`VmHWM` of `/proc/self/status`), in
/// bytes.
pub fn peak_rss() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = text.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kb.checked_mul(1024)
}

/// What the bench times with: each thread's CPU time when the kernel gives
/// it, else the wall clock since the clock was made.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    cpu: bool,
    start: Instant,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            cpu: thread_cpu_ns().is_some(),
            start: Instant::now(),
        }
    }

    /// Whether it reads CPU time (`"thread_cpu"`) or wall time (`"wall"`).
    pub fn kind(self) -> &'static str {
        if self.cpu { "thread_cpu" } else { "wall" }
    }

    /// Now, in nanoseconds on this clock.
    pub fn now(self) -> u64 {
        let wall = || u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        if self.cpu {
            thread_cpu_ns().unwrap_or_else(wall)
        } else {
            wall()
        }
    }
}

/// The time every wrapped codec spent decoding, and its calls.
#[derive(Debug, Default)]
pub struct DecodeTime {
    pub ns: AtomicU64,
    pub calls: AtomicU64,
}

/// A codec timed on the bench's [`Clock`].
struct Timed {
    inner: Arc<dyn BlockCodec>,
    clock: Clock,
    total: Arc<DecodeTime>,
}

impl BlockCodec for Timed {
    fn decode(&self, input: &Bytes, size: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
        let t = self.clock.now();
        let got = self.inner.decode(input, size, out);
        let spent = self.clock.now().saturating_sub(t);
        self.total.ns.fetch_add(spent, Ordering::Relaxed);
        self.total.calls.fetch_add(1, Ordering::Relaxed);
        got
    }
}

/// UnityPy's codecs with LZMA, LZ4 and LZ4HC timed into `total`. Slot 0
/// keeps the built-in stored codec, so stored blocks stay slices of the
/// input (`Codecs::stores_as_is`), as in the dump.
pub fn timed_codecs(clock: Clock, total: &Arc<DecodeTime>) -> Codecs {
    let base = Codecs::unitypy();
    let mut codecs = Codecs::unitypy();
    for which in [Codec::Lzma, Codec::Lz4, Codec::Lz4Hc] {
        if let Some(inner) = base.get(which) {
            let timed = Arc::new(Timed {
                inner: inner.clone(),
                clock,
                total: total.clone(),
            });
            // Canonical slots: never refused.
            let _ = codecs.set(which, timed);
        }
    }
    codecs
}
