use std::{cell::Cell, io::Cursor, process::Command, sync::Arc};
use unityrs_batch::{
    alloc::{Counting, Hook},
    attempt::ReasonCode,
    phase::{Account, Buffer, Error},
    sort::{self, Frame, Hooks, Options, Order, Point, Source},
    spool::{Counter, Host, workspace::Workspace},
};
use unityrs_testkit::{child, scratch::Scratch};

thread_local! {
    static REFUSE: Cell<usize> = const { Cell::new(0) };
    // Refuse an allocation by ordinal, regardless of its size or helper.
    static SITE: Cell<usize> = const { Cell::new(0) };
    static SEEN: Cell<usize> = const { Cell::new(0) };
    static COUNT: Cell<bool> = const { Cell::new(false) };
}
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, size: usize) -> bool {
        if COUNT.try_with(Cell::get).unwrap_or(false) {
            let n = SEEN.with(|seen| {
                let n = seen.get() + 1;
                seen.set(n);
                n
            });
            if SITE.with(Cell::get) == n {
                COUNT.with(|count| count.set(false));
                SITE.with(|site| site.set(0));
                return true;
            }
        }
        REFUSE
            .try_with(|r| {
                if size != 0 && size == r.get() {
                    r.set(0);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);
struct Bytes;
impl Order for Bytes {
    fn tag(&self) -> u32 {
        2
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        a.cmp(b)
    }
}
struct Data {
    left: u8,
    panic: bool,
}
impl Source for Data {
    fn next(&mut self, out: &mut Frame, _: &dyn Fn() -> bool) -> Result<bool, Error> {
        assert!(!self.panic, "source panic");
        if self.left == 0 {
            return Ok(false);
        }
        self.left -= 1;
        out.set(&[self.left])?;
        Ok(true)
    }
}
struct Fault<'a> {
    mode: &'a str,
    meter: &'a Arc<unityrs_core::limits::Meter>,
    armed: Cell<bool>,
    requested: Cell<bool>,
}
impl Hooks for Fault<'_> {
    fn point(&self, point: Point) {
        if ((self.mode.starts_with("sites") && point == Point::Entry)
            || (self.mode == "before_sort" && point == Point::BeforeSort))
            && !self.armed.replace(true)
        {
            SEEN.with(|seen| seen.set(0));
            COUNT.with(|count| count.set(true));
        }
        let request = match (self.mode, point) {
            ("merge_head", Point::MergeGroup) => 65_532,
            ("encoder", Point::AfterSort) => 65_536,
            _ => return,
        };
        if !self.armed.replace(true) {
            REFUSE.with(|r| r.set(request));
        }
    }
    fn run_storage(&self, _: u64) {
        if self.mode == "leak" {
            std::mem::forget(self.meter.charge(7).unwrap());
        }
        if self.mode == "absorbed" {
            REFUSE.with(|r| r.set(117));
            let mut ignored = Vec::<u8>::new();
            assert!(ignored.try_reserve_exact(117).is_err());
        }
    }
    fn request_stop(&self, _: unityrs_batch::job::RunFailure) {
        self.requested.set(true);
    }
}
#[test]
fn audit_child() {
    let Ok(mode) = std::env::var("UNITYRS_SORT_ALLOC") else {
        return;
    };
    let scratch = Scratch::new("sort-alloc");
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let directory = if mode == "sites_long" {
        let directory = scratch.join("x".repeat(200)).join("y".repeat(200));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("w")
    } else {
        scratch.join("w")
    };
    let mut workspace = Workspace::create(directory, counter, Arc::new(Host)).unwrap();
    let account = Account::new(if mode == "budget" { 1024 } else { 64 << 20 });
    let fault = Fault {
        mode: &mode,
        meter: account.meter(),
        armed: Cell::new(false),
        requested: Cell::new(false),
    };
    let request = match mode.as_str() {
        "arena" => (sort::RUN_BYTES - 8192) * 3 / 4,
        "index" => (sort::RUN_BYTES - 8192) / 4,
        "carry" => 65_532,
        "parser" => 32_768,
        "read_buffer" => 65_536,
        "last_key" => 1024,
        _ => 0,
    };
    REFUSE.with(|r| r.set(request));
    let site = std::env::var("UNITYRS_SORT_SITE")
        .ok()
        .map_or(0, |n| n.parse::<usize>().unwrap());
    SITE.with(|r| r.set(if mode == "before_sort" { 1 } else { site }));
    let result = if matches!(mode.as_str(), "parser" | "read_buffer" | "last_key") {
        account.execute(|| {
            let _reader = unityrs_batch::records::read::Reader::new(
                Cursor::new(b""),
                "input",
                false,
                0,
                account.meter(),
            )?;
            Ok(())
        })
    } else {
        sort::sort(
            &mut Data {
                left: 4,
                panic: mode == "panic",
            },
            &Bytes,
            &mut workspace,
            &account,
            Options {
                run_records: 1,
                fan_in: 2,
                ..Options::default()
            },
            &|| false,
            &fault,
        )
        .map(|_| ())
    };
    COUNT.with(|count| count.set(false));
    if mode.starts_with("sites") && site == 0 {
        result.unwrap();
        println!("allocation_sites={}", SEEN.with(Cell::get));
        return;
    }
    SITE.with(|r| assert_eq!(r.get(), 0));
    let Error::Run(failure) = result.unwrap_err() else {
        panic!("run failure required")
    };
    assert_eq!(
        failure.reason,
        if matches!(mode.as_str(), "budget" | "leak" | "panic") {
            ReasonCode::Internal
        } else {
            ReasonCode::HostMemory
        }
    );
    if !matches!(mode.as_str(), "parser" | "read_buffer" | "last_key") {
        assert!(fault.requested.get());
    }
    REFUSE.with(|r| assert_eq!(r.get(), 0));
    assert_eq!(account.snapshot().input.used != 0, mode == "leak");
}
#[test]
fn fallible_allocations_phase_refusals_leaks_and_panics_are_audited() {
    for mode in [
        "arena",
        "index",
        "carry",
        "merge_head",
        "encoder",
        "parser",
        "read_buffer",
        "last_key",
        "before_sort",
        "budget",
        "leak",
        "panic",
        "absorbed",
    ] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "audit_child", "--nocapture", "--test-threads=1"])
                .env("UNITYRS_SORT_ALLOC", mode),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
fn borrowed_charged_source_survives_a_sort_and_drops_at_the_phase_boundary() {
    struct Charged {
        bytes: Buffer<u8>,
    }
    impl Source for Charged {
        fn next(&mut self, _: &mut Frame, _: &dyn Fn() -> bool) -> Result<bool, Error> {
            assert_eq!(self.bytes.len(), 1024);
            Ok(false)
        }
    }
    let scratch = Scratch::new("sort-borrowed-source");
    let mut workspace = Workspace::create(
        scratch.join("w"),
        Counter::new(u64::MAX, 0, 4096).unwrap(),
        Arc::new(Host),
    )
    .unwrap();
    let account = Account::new(64 << 20);
    account
        .execute(|| {
            let mut bytes = Buffer::new(account.meter(), 1024)?;
            bytes.fill(0);
            let mut source = Charged { bytes };
            let report = sort::sort(
                &mut source,
                &Bytes,
                &mut workspace,
                &account,
                Options::default(),
                &|| false,
                &sort::NoHooks,
            )?;
            assert!(report.file.is_none());
            assert!(account.snapshot().input.used > 0);
            Ok(())
        })
        .unwrap();
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn every_reachable_sort_allocation_refuses_without_abort() {
    let run = |mode: &str, site: usize| {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "audit_child", "--nocapture", "--test-threads=1"])
                .env("UNITYRS_SORT_ALLOC", mode)
                .env("UNITYRS_SORT_SITE", site.to_string()),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{mode} site {site}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    for mode in ["sites", "sites_long"] {
        let baseline = run(mode, 0);
        let stdout = String::from_utf8(baseline.stdout).unwrap();
        let count: usize = stdout
            .split("allocation_sites=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(count > 0);
        for site in 1..=count {
            run(mode, site);
        }
    }
}
