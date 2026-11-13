//! Incremental disk generation; no dataset/model beside the measured sort.
use std::{cell::Cell, fs::File, io::Read, process::Command, sync::Arc};
use unityrs_batch::{
    alloc::{self, Counting},
    phase::{Account, Error},
    sort::{self, Frame, Hooks, Options, Order, Source},
    spool::{Counter, Host, OwnedFile, workspace::Workspace},
    work::rounded,
};
use unityrs_testkit::{child, scratch::Scratch};

#[global_allocator]
static ALLOC: Counting = Counting::new();
const WIDTH: usize = 128;
struct Numbers;
fn number(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.get(..8).unwrap().try_into().unwrap())
}
impl Order for Numbers {
    fn tag(&self) -> u32 {
        23
    }
    fn validate(&self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() == WIDTH {
            Ok(())
        } else {
            Err(Error::internal())
        }
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        number(a).cmp(&number(b))
    }
}
struct Disk(File);
impl Source for Disk {
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        unityrs_batch::phase::check(stopped)?;
        let mut bytes = [0; WIDTH];
        if self.0.read(&mut bytes[..1])? == 0 {
            return Ok(false);
        }
        self.0.read_exact(&mut bytes[1..])?;
        out.set(&bytes)?;
        Ok(true)
    }
}
struct Observe {
    charged: Cell<u64>,
}
impl Hooks for Observe {
    fn run_storage(&self, charged: u64) {
        self.charged.set(charged);
    }
}
fn write_source(file: &mut OwnedFile, count: u64) {
    let mut buffer = Vec::with_capacity(64 * 1024);
    for i in (0..count).rev() {
        let mut bytes = [b'x'; WIDTH];
        bytes[..8].copy_from_slice(&i.to_le_bytes());
        buffer.extend_from_slice(&bytes);
        if buffer.len() == 64 * 1024 {
            file.append(&buffer, &|| false).unwrap();
            buffer.clear();
        }
    }
    if !buffer.is_empty() {
        file.append(&buffer, &|| false).unwrap();
    }
    file.seal(&|| false).unwrap();
}
fn case(count: u64, options: Options, block: u64, retained: bool) {
    let scratch = Scratch::new("sort-scale");
    let counter = Counter::new(u64::MAX, 0, block).unwrap();
    let mut committed =
        OwnedFile::create(scratch.join("committed"), counter.clone(), Arc::new(Host)).unwrap();
    write_source(&mut committed, count);
    let source_claim = counter.used();
    let work_bytes = rounded(count * u64::try_from(WIDTH).unwrap(), 4096).unwrap();
    let ratio = if block < 4096 && 4096 % block != 0 {
        2
    } else {
        block.div_ceil(4096).max(1)
    };
    let s = work_bytes * ratio;
    let mut workspace =
        Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
    let mut kept = Vec::new();
    if retained {
        // Two S-sized retained streams exercise the later-pass allowance.
        // A fixed-size write buffer is dropped before measuring the sort.
        let buffer = vec![0; 64 * 1024];
        for _ in 0..2 {
            let mut file = workspace.create_file(&|| false).unwrap();
            let mut left = s;
            while left != 0 {
                let n = usize::try_from(left.min(64 * 1024)).unwrap();
                file.append(&buffer[..n], &|| false).unwrap();
                left -= u64::try_from(n).unwrap();
            }
            kept.push(file.seal(&|| false).unwrap());
        }
    }
    let retained_claim = workspace.claimed();
    let account = Account::new(64 << 20);
    let hooks = Observe {
        charged: Cell::new(0),
    };
    let mut source = Disk(File::open(committed.path()).unwrap());
    let (result, measured) = alloc::measured(|| {
        sort::sort(
            &mut source,
            &Numbers,
            &mut workspace,
            &account,
            options,
            &|| false,
            &hooks,
        )
    });
    let report = result.unwrap();
    assert!(hooks.charged.get() <= 32 << 20);
    assert!(account.snapshot().input.peak <= 40 << 20);
    assert_eq!(account.snapshot().input.used, 0);
    assert_eq!(measured.kept, 0);
    assert!(measured.peak <= 40 << 20, "{measured:?}");
    if count == 1_000_000 {
        assert!(report.runs > 256);
        assert_eq!(report.generations, 3);
    }
    let file = report.file.unwrap();
    let mut reader =
        sort::Reader::open(&workspace, &file, &Numbers, account.meter(), &|| false).unwrap();
    for expected in 0..count {
        let bytes = reader.next(&|| false).unwrap().unwrap();
        assert_eq!(number(bytes), expected);
        assert!(bytes.get(8..).unwrap().iter().all(|&b| b == b'x'));
    }
    assert!(reader.next(&|| false).unwrap().is_none());
    drop(reader);
    // P bounds one rounded generation; two catalogs, each round(16+32n).
    let e = count * (u64::try_from(WIDTH).unwrap() + 4);
    let p = e + report.runs * (16 + block - 1);
    let k = rounded(16 + 32 * report.runs, block).unwrap();
    let ledger = retained_claim + 2 * p + 2 * k;
    assert!(counter.peak() - source_claim <= ledger);
    assert!(
        ledger <= 6 * s,
        "ledger={ledger}, S={s}, peak={}",
        counter.peak()
    );
    println!(
        "LEDGER count={count} block={block} S={s} phase_peak={} ledger={ledger} runs={} generations={} memory={}",
        counter.peak() - source_claim,
        report.runs,
        report.generations,
        measured.peak
    );
    workspace.retire(file, &|| false).unwrap();
    for file in kept {
        workspace.retire(file, &|| false).unwrap();
    }
    assert_eq!(counter.used(), source_claim);
    committed.remove().unwrap();
    assert_eq!(counter.used(), 0);
}

#[test]
fn scale_child() {
    let Ok(mode) = std::env::var("UNITYRS_SORT_SCALE") else {
        return;
    };
    match mode.as_str() {
        "production" => case(850_000, Options::default(), 4096, false),
        "million" => case(
            1_000_000,
            Options {
                run_bytes: 512 * 1024,
                ..Options::default()
            },
            4096,
            false,
        ),
        "ledger" => {
            for block in [3072, 4096, 5003, 65_536] {
                case(
                    20_000,
                    Options {
                        run_bytes: 128 * 1024,
                        ..Options::default()
                    },
                    block,
                    true,
                );
            }
        }
        _ => panic!("unknown scale mode"),
    }
}
#[test]
fn counted_disk_stress_and_lifetime_ledger() {
    for mode in ["production", "million", "ledger"] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "scale_child", "--nocapture", "--test-threads=1"])
                .env("UNITYRS_SORT_SCALE", mode),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
}
