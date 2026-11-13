//! Real B4 -> B6 handoff -> two concurrent archive passes on one spool counter.
mod archive_support;
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
use std::{
    io::Read,
    sync::{Arc, Condvar, Mutex},
};
use unityrs_batch::{
    archive::{self, Context as ArchiveContext, Hooks, Input, Payloads, Point},
    digest::Hashes,
    phase::{Account, Buffer, Error},
    sort::{NoHooks, Options},
    spool::{
        Counter, FileSystem, Host,
        workspace::{Sealed, Workspace},
    },
    winners::{self, Context, Prepared},
    work::rounded,
};
use unityrs_testkit::{guard::DEADLOCK_GUARD, scratch::Scratch};
struct PayloadFile(std::path::PathBuf);
impl Payloads for PayloadFile {
    fn open(&self, source: u32, ordinal: u64) -> Result<Box<dyn Read + Send>, Error> {
        if source != 0 || ordinal != 0 {
            return Err(Error::internal());
        }
        Ok(Host.open(&self.0)?)
    }
}
struct Hold<'a> {
    state: Mutex<(u32, u32)>,
    wake: Condvar,
    counter: &'a Counter,
    expected: u64,
}
impl Hooks for Hold<'_> {
    fn at(&self, point: Point) -> Result<(), Error> {
        if point == Point::CentralSeal {
            let mut state = self.state.lock().unwrap();
            state.0 += 1;
            self.wake.notify_all();
            let (mut state, timeout) = self
                .wake
                .wait_timeout_while(state, DEADLOCK_GUARD, |s| s.0 < 2)
                .unwrap();
            assert!(
                !timeout.timed_out(),
                "deadlock guard: other archive never reached sealing"
            );
            assert_eq!(self.counter.used(), self.expected);
            state.1 += 1;
            self.wake.notify_all();
            let (state, timeout) = self
                .wake
                .wait_timeout_while(state, DEADLOCK_GUARD, |s| s.1 < 2)
                .unwrap();
            assert!(
                !timeout.timed_out(),
                "deadlock guard: other archive never checked its claim"
            );
            drop(state);
        }
        Ok(())
    }
}
struct Dispatch {
    key: String,
    index: u32,
    count: u64,
    size: u64,
    list: Sealed,
}
fn generate(cx: ArchiveContext<'_>, d: &Dispatch, chunk: usize) -> Result<archive::Summary, Error> {
    let input = Input {
        key: &d.key,
        index: d.index,
        members: d.count,
        size: d.size,
        list: &d.list,
    };
    let mut buffer = Buffer::new(cx.meter, chunk)?;
    buffer.fill(0);
    let mut stream = archive::open(cx, input, &|| false)?;
    while stream.read(&mut buffer, &|| false)? > 0 {}
    stream.finish(&|| false)
}
fn case() -> harness::Case {
    let mut candidate = harness::candidate("a", "key", None, 0);
    candidate.md5 = Hashes::of(b"x").md5;
    let mut case = harness::Case::new(vec![candidate]);
    // Original spec order deliberately differs from archive-key order.
    for i in (0..256).rev() {
        case.archives.push(format!("a{i:03}{}", "\\\"".repeat(64)));
        case.expansions.push(harness::Expansion {
            input: "a".into(),
            key: "key".into(),
            archive: case.archives.len() - 1,
            path: "a".into(),
            reason: None,
            cut: false,
        });
    }
    case
}
fn concurrent_pair(
    workspace: &Workspace,
    payload: &PayloadFile,
    account: &Account,
    counter: &Counter,
    batch: &[Dispatch],
    central: u64,
) {
    let retained = counter.used();
    let hold = Hold {
        state: Mutex::new((0, 0)),
        wake: Condvar::new(),
        counter,
        expected: retained + 2 * central,
    };
    let results = std::thread::scope(|scope| {
        let run = |d: &Dispatch| {
            generate(
                ArchiveContext {
                    workspace,
                    payloads: payload,
                    meter: account.meter(),
                    hooks: &hold,
                },
                d,
                65_536,
            )
        };
        let first = scope.spawn(move || run(&batch[0]));
        let second = scope.spawn(move || run(&batch[1]));
        [
            first.join().unwrap().unwrap(),
            second.join().unwrap().unwrap(),
        ]
    });
    assert_eq!(counter.used(), retained);
    // A fresh expected-hash miss/transfer pass uses a new numeric file
    // only after the previous owner deleted its scratch.
    for (d, expected) in batch.iter().zip(results) {
        let actual = generate(
            ArchiveContext {
                workspace,
                payloads: payload,
                meter: account.meter(),
                hooks: &archive::NoHooks,
            },
            d,
            7,
        )
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(counter.used(), retained);
    }
}
#[test]
fn shared_counter_covers_concurrent_passes_and_regeneration_at_all_blocks() {
    for block in [512, 3072, 4096, 5003, 65_536] {
        let scratch = Scratch::new("archive-pipeline-ledger");
        let case = case();
        let spec = case.spec().spec;
        let counter = Counter::new(u64::MAX, 0, block).unwrap();
        let account = Account::new(40 << 20);
        let (commits, spooled) = case.commit(&scratch, &counter, &account, &spec);
        let baseline = counter.used();
        let prepared = Prepared::from_spec(&spec).unwrap();
        let mut workspace =
            Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
        let handoff = winners::complete(&mut Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options::default(),
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &winners::NoObserver,
        })
        .unwrap();
        let mut reader = winners::archives::Reader::open(
            &workspace,
            handoff.metadata.as_ref().unwrap(),
            account.meter(),
            &|| false,
        )
        .unwrap();
        let payload = PayloadFile(commits.paths[0].parent().unwrap().join("o0"));
        let central = rounded(47, block).unwrap();
        let mut processed = 0;
        loop {
            let mut batch = Vec::new();
            for _ in 0..2 {
                if let Some(a) = reader.next(&|| false).unwrap() {
                    batch.push(Dispatch {
                        key: a.key.into(),
                        index: a.index,
                        count: a.members,
                        size: a.size,
                        list: a.list.unwrap(),
                    });
                }
            }
            if batch.is_empty() {
                break;
            }
            assert_eq!(batch.len(), 2);
            concurrent_pair(&workspace, &payload, &account, &counter, &batch, central);
            processed += 2;
        }
        drop(reader);
        assert_eq!(processed, 256);
        let k = if block < 4096 && 4096 % block != 0 {
            2
        } else {
            block.div_ceil(4096).max(1)
        };
        let s = spooled * k;
        let a: u64 = spec
            .archives
            .iter()
            .map(|a| {
                rounded(
                    u64::try_from(serde_json::to_string(&a.key).unwrap().len() - 2).unwrap() + 512,
                    block,
                )
                .unwrap()
            })
            .sum();
        let allowance = 6 * s + 2 * a + 2 * central;
        assert!(counter.peak() - baseline <= allowance);
        assert_eq!(account.snapshot().input.used, 0);
        println!(
            "ARCHIVE_LEDGER b={block} S={s} A={a} central={} peak={} allowance={allowance}",
            2 * central,
            counter.peak() - baseline
        );
    }
}
