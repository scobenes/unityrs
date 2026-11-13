//! Counted blocking-thread children: every reachable stream allocation ordinal.
mod archive_support;
use archive_support::Scene;
use std::{cell::Cell, process::Command};
use unityrs_batch::{
    alloc::{self, Counting, Hook},
    archive,
    attempt::ReasonCode,
    phase::{Account, Buffer, Error},
};
use unityrs_testkit::child;
thread_local! {
    static ACTIVE: Cell<bool> = const {Cell::new(false)};
    static SITE: Cell<usize> = const {Cell::new(0)};
    static SEEN: Cell<usize> = const {Cell::new(0)};
}
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, _: usize) -> bool {
        if !ACTIVE.try_with(Cell::get).unwrap_or(false) {
            return false;
        }
        let n = SEEN.with(|s| {
            let n = s.get() + 1;
            s.set(n);
            n
        });
        if SITE.with(Cell::get) == n {
            SITE.with(|s| s.set(0));
            true
        } else {
            false
        }
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);
fn drain(scene: &Scene, account: &Account) -> Result<archive::Summary, Error> {
    let mut stream = archive::open(scene.context(account), scene.input(), &|| false)?;
    let mut buffer = Buffer::new(account.meter(), 65_536)?;
    buffer.fill(0);
    while stream.read(&mut buffer, &|| false)? > 0 {}
    stream.finish(&|| false)
}
#[test]
fn allocation_child() {
    if std::env::var_os("UNITYRS_ARCHIVE_ALLOC").is_none() {
        return;
    }
    let scene = Scene::new(&[("a", b"x"), ("b", b"")]);
    let mut allocations = 0;
    for site in 0..=256 {
        if site > allocations && site > 0 {
            break;
        }
        let account = Account::new(1 << 20);
        let (result, measured, seen, reached) = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    SEEN.with(|s| s.set(0));
                    SITE.with(|s| s.set(site));
                    ACTIVE.with(|s| s.set(true));
                    let (result, measured) =
                        alloc::measured(|| account.execute(|| drain(&scene, &account)));
                    ACTIVE.with(|s| s.set(false));
                    (result, measured, SEEN.with(Cell::get), SITE.with(Cell::get))
                })
                .join()
                .unwrap()
        });
        if site == 0 {
            result.unwrap();
            allocations = seen;
            assert!((1..256).contains(&allocations));
            assert!(measured.peak < archive::ENVELOPE, "{measured:?}");
            println!(
                "ARCHIVE_ALLOC sites={allocations} peak={} charge={}",
                measured.peak,
                account.snapshot().input.peak
            );
        } else {
            assert_eq!(reached, 0, "site {site}");
            assert!(
                matches!(result,Err(Error::Run(f)) if f.reason==ReasonCode::HostMemory),
                "site {site}: {result:?}"
            );
        }
        assert_eq!(measured.kept, 0);
        assert_eq!(account.snapshot().input.used, 0);
    }
}
#[test]
fn scale_child() {
    if std::env::var_os("UNITYRS_ARCHIVE_SCALE").is_none() {
        return;
    }
    for (count, size) in [(1, 1), (65_536, 0), (1, 33_554_433)] {
        let scene = Scene::repeated(count, &vec![b'x'; size]);
        let baseline = scene.counter.used();
        let account = Account::new(1 << 20);
        let (result, measured) = alloc::measured(|| account.execute(|| drain(&scene, &account)));
        result.unwrap();
        assert!(measured.peak < archive::ENVELOPE, "{measured:?}");
        assert!(account.snapshot().input.peak < archive::ENVELOPE);
        assert_eq!(measured.kept, 0);
        assert_eq!(account.snapshot().input.used, 0);
        assert_eq!(scene.counter.used(), baseline);
        println!(
            "ARCHIVE_SCALE count={count} size={size} counted={} charged={}",
            measured.peak,
            account.snapshot().input.peak
        );
    }
}
fn run(test: &str, env: &str) {
    let output = child::output(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env(env, "1"),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
#[test]
fn every_stream_allocation_is_fallible_on_the_blocking_thread() {
    run("allocation_child", "UNITYRS_ARCHIVE_ALLOC");
}
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn memory_does_not_grow_with_members_or_payloads() {
    run("scale_child", "UNITYRS_ARCHIVE_SCALE");
}
