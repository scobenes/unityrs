//! Fixed X12 observations. Test fixture construction and local peers are
//! outside this window. Neither allowance is used to calibrate coefficients.
#![allow(dead_code)]
pub mod formats;
use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    },
    time::Instant,
};
use unityrs_batch::{
    alloc::Hook,
    bound,
    progress::Phase,
    run::hooks::memory::{self, Point},
};
use unityrs_core::limits::heap_cost;
use unityrs_testkit::{
    counted::{self, Tally},
    guard::DEADLOCK_GUARD,
    ledger::Ledger,
};

pub static HELD: Ledger = Ledger::new(heap_cost);
pub static WHOLE: Ledger = Ledger::new(heap_cost);
static POST_STAGE: Ledger = Ledger::new(heap_cost);
static POST_BASE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static ROLES: AtomicU64 = AtomicU64::new(0);
static WORKERS: AtomicU64 = AtomicU64::new(0);
static CONTROLLER_POOL: AtomicU64 = AtomicU64::new(0);
static FINISHER_POOL: AtomicU64 = AtomicU64::new(0);
static MARKS: AtomicU64 = AtomicU64::new(0);
static PHASES: AtomicU64 = AtomicU64::new(0);
static CURRENT: AtomicU64 = AtomicU64::new(0);
static MAXIMUM: AtomicU64 = AtomicU64::new(0);
static MAX_PHASE: AtomicU64 = AtomicU64::new(0);
static NETWORK: AtomicU64 = AtomicU64::new(0);
static MERGES: AtomicU64 = AtomicU64::new(0);
static REPORTED: AtomicBool = AtomicBool::new(false);
pub struct Bridge;
impl Hook for Bridge {
    fn shrunk(&self, old: usize, new: usize) {
        if counted::counted() {
            HELD.shrunk(old, new);
            WHOLE.shrunk(old, new);
            POST_STAGE.shrunk(old, new);
        }
    }
    fn growing(&self, size: usize) {
        if counted::counted() {
            HELD.growing(size);
            POST_STAGE.growing(size);
            peak_phase();
            WHOLE.growing(size);
        }
    }
    fn failed(&self, size: usize) {
        if counted::counted() {
            HELD.failed(size);
            POST_STAGE.failed(size);
            WHOLE.failed(size);
        }
    }
    fn shrank(&self, size: usize) {
        if counted::counted() {
            HELD.shrank(size);
            POST_STAGE.shrank(size);
            WHOLE.shrank(size);
        }
    }
}
pub struct Config {
    pub hold: Option<Phase>,
    pub ready: File,
    pub release: PathBuf,
    pub report: File,
    pub inject: bool,
    pub freeze: bool,
    pub freeze_attempt: bool,
}
static CONFIG: OnceLock<Config> = OnceLock::new();
static BOUND: AtomicU64 = AtomicU64::new(u64::MAX);
static ALLOWANCE: AtomicU64 = AtomicU64::new(u64::MAX);
static SPEC: AtomicU64 = AtomicU64::new(0);
static FROZEN: AtomicU64 = AtomicU64::new(0);
static IO_HELD: AtomicBool = AtomicBool::new(false);

pub struct Spool;
impl unityrs_batch::spool::FileSystem for Spool {
    fn available(&self, path: &std::path::Path) -> std::io::Result<u64> {
        unityrs_batch::spool::FileSystem::available(&unityrs_batch::spool::Host, path)
    }
    fn create(
        &self,
        path: &std::path::Path,
    ) -> std::io::Result<Box<dyn unityrs_batch::spool::Writer>> {
        let inner = unityrs_batch::spool::FileSystem::create(&unityrs_batch::spool::Host, path)?;
        Ok(Box::new(AttemptWriter {
            inner,
            records: path.file_name().is_some_and(|n| n == "records.ndjson"),
        }))
    }
}
struct AttemptWriter {
    inner: Box<dyn unityrs_batch::spool::Writer>,
    records: bool,
}
impl Write for AttemptWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let config = CONFIG.get().unwrap();
        let current = std::thread::current();
        let worker = current.name().is_some_and(|name| {
            name.strip_prefix("batch-")
                .is_some_and(|n| n.parse::<u32>().is_ok())
        });
        if config.freeze_attempt && self.records && worker && !IO_HELD.swap(true, SeqCst) {
            FROZEN.store(u64::try_from(HELD.observe().charged).unwrap(), SeqCst);
            (&config.ready).write_all(b"frozen attempt\n")?;
            let until = Instant::now() + DEADLOCK_GUARD;
            while !config.release.exists() {
                assert!(Instant::now() < until, "frozen attempt release");
                std::thread::yield_now();
            }
        }
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
impl unityrs_batch::spool::Writer for AttemptWriter {
    fn seal(&self) -> std::io::Result<()> {
        self.inner.seal()
    }
    fn write_at(&mut self, bytes: &[u8], offset: u64) -> std::io::Result<usize> {
        self.inner.write_at(bytes, offset)
    }
    fn truncate(&mut self) -> std::io::Result<()> {
        self.inner.truncate()
    }
}

/// A real publication write held with its charged buffer on the controller
/// pool while the independent finisher writes rows on its own runtime.
pub struct Sink;
impl unityrs_batch::sink::fs::FileSystem for Sink {
    fn write(&self, file: &File, bytes: &[u8]) -> std::io::Result<usize> {
        let config = CONFIG.get().unwrap();
        if config.freeze
            && PHASES.load(SeqCst) & (1 << Phase::Publish as u32) != 0
            && std::thread::current().name() == Some("batch-controller-pool")
            && !IO_HELD.swap(true, SeqCst)
        {
            FROZEN.store(u64::try_from(HELD.observe().charged).unwrap(), SeqCst);
            (&config.ready).write_all(b"frozen\n")?;
            let until = Instant::now() + DEADLOCK_GUARD;
            while !config.release.exists() {
                assert!(Instant::now() < until, "frozen publication release");
                std::thread::yield_now();
            }
        }
        unityrs_batch::sink::fs::FileSystem::write(&unityrs_batch::sink::fs::Host, file, bytes)
    }
}
pub fn register() {
    if counted::counted() {
        return;
    }
    counted::count_this_thread();
    let current = std::thread::current();
    let name = current.name().unwrap_or("");
    let bit = match name {
        "memory_child" => 1,
        "batch-controller" => 2,
        "batch-progress" => 4,
        "batch-finisher" => 8,
        "batch-lifecycle" => 16,
        "batch-stdin" => 32,
        "exit-writer" => 64,
        "batch-controller-pool" => {
            CONTROLLER_POOL.fetch_add(1, SeqCst);
            128
        }
        "batch-finisher-pool" => {
            FINISHER_POOL.fetch_add(1, SeqCst);
            256
        }
        n if n.starts_with("batch-") => {
            WORKERS.fetch_add(1, SeqCst);
            512
        }
        _ => 1024,
    };
    ROLES.fetch_or(bit, SeqCst);
}
pub fn mark() {
    MARKS.fetch_add(1, SeqCst);
}

pub fn start(config: Config) {
    CONFIG.set(config).ok().unwrap();
    memory::install(memory::Observer {
        charge: |bytes, held| {
            if held {
                HELD.charge(bytes);
                POST_STAGE.charge(bytes);
            } else {
                HELD.release(bytes);
                POST_STAGE.release(bytes);
                peak_phase();
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
    })
    .unwrap();
    // No testkit span tables or fixture bytes enter either measured window.
    HELD.arm(u64::MAX);
    WHOLE.arm(u64::MAX);
    POST_STAGE.arm(u64::MAX);
    register();
}
pub fn selected(b: bound::Bound, workers: u32, phase1_peak: u64) {
    SPEC.store(b.c_spec, SeqCst);
    BOUND.store(b.total(), SeqCst);
    // Stacks are not Rust heap. Input/phase/finisher charges are observed;
    // network storage remains in C_net. Do not add unused I or C_fin.
    let uncharged = b.c_env
        + b.c_spec
        + b.c_parse
        + b.c_hdr
        + b.c_net
        + b.c_base
        + u64::from(workers) * (bound::WORKER_STATE + bound::PANIC_SLOT);
    ALLOWANCE.store(uncharged, SeqCst);
    assert!(phase1_peak <= b.total());
}
fn point(point: Point) {
    let config = CONFIG.get().unwrap();
    match point {
        Point::SortMerge => {
            MERGES.fetch_add(1, SeqCst);
        }
        Point::Phase(phase) => {
            if phase == Phase::Extract {
                let (heap, charged) = POST_STAGE.arm(u64::MAX);
                POST_BASE.store(heap - charged, SeqCst);
            }
            CURRENT.store(phase as u64, SeqCst);
            PHASES.fetch_or(1 << phase as u32, SeqCst);
            if config.hold == Some(phase) {
                (&config.ready).write_all(b"ready\n").unwrap();
                let until = Instant::now() + DEADLOCK_GUARD;
                while !config.release.exists() {
                    assert!(Instant::now() < until, "phase hold release");
                    std::thread::yield_now();
                }
                if phase == Phase::Validate {
                    // libtest's runner is outside the engine and predates
                    // Startup's mask. Queue the early signal on the blocked
                    // driver itself, as a single-threaded CLI receives it.
                    unityrs_batch::sys::testing::raise(libc::SIGTERM).unwrap();
                }
            }
        }
        Point::Exit => {
            if REPORTED.swap(true, SeqCst) {
                return;
            }
            if config.inject {
                // A deliberately uncharged holder, kept through exit. It
                // exceeds the residual allowance even though I is much larger.
                let mut leak = Vec::new();
                leak.try_reserve_exact(64 << 20).unwrap();
                leak.resize(64 << 20, 0_u8);
                std::mem::forget(std::hint::black_box(leak));
            }
            let o = HELD.observe();
            let whole = WHOLE.observe();
            writeln!(
                &config.report,
                "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
                o.max_residual,
                ALLOWANCE.load(SeqCst),
                whole.peak_above_arm,
                BOUND.load(SeqCst),
                o.charged,
                o.events,
                ROLES.load(SeqCst),
                WORKERS.load(SeqCst),
                CONTROLLER_POOL.load(SeqCst),
                FINISHER_POOL.load(SeqCst),
                MARKS.load(SeqCst),
                PHASES.load(SeqCst),
                o.heap,
                FROZEN.load(SeqCst),
                SPEC.load(SeqCst),
                NETWORK.load(SeqCst),
                MAX_PHASE.load(SeqCst),
                POST_STAGE.observe().max_residual + POST_BASE.load(SeqCst),
                MERGES.load(SeqCst),
            )
            .unwrap();
        }
    }
}
fn peak_phase() {
    let maximum = u64::try_from(HELD.observe().max_residual).unwrap_or(0);
    if maximum > MAXIMUM.fetch_max(maximum, SeqCst) {
        MAX_PHASE.store(CURRENT.load(SeqCst), SeqCst);
    }
}
