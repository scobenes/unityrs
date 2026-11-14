//! B8b: directory archive transfer, resume, outcomes and the central-scratch
//! ledger through the real B6 hand-over and B7 publication.
mod common;
mod controller_support;
#[allow(dead_code)]
mod directory_support;
mod publication_support;
mod stage_support;
use publication_support::Scene;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use unityrs_batch::{
    archive::{self, Eligibility},
    digest::Hashes,
    outcomes::{Code, Outcomes},
    phase::{Account, Error},
    publish::{self, Admission, Context, Gate, Hooks, Point, payload::Committed},
    sink::directory::Directory,
    spool::Host,
    winners::layout::LIMIT,
};

const KEYS: [&str; 2] = ["z.zip", "a.zip"];

fn directory(scene: &Scene, name: &str) -> Directory {
    let root = scene.scratch.join(name);
    std::fs::create_dir(&root).unwrap();
    directory_support::preflight(&root, 1, directory_support::fs(true)).unwrap()
}
fn drive(
    scene: &Scene,
    directory: &Directory,
    outcomes: &Outcomes,
    resume: bool,
    hooks: &dyn Hooks,
    stop: &AtomicBool,
) -> Result<Admission, Error> {
    let account = Account::new(64 << 20);
    let result = publish::drive(&Context {
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        payloads: &Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        },
        directory,
        outcomes,
        gate: &Gate::default(),
        account: &account,
        run_id: "r",
        resume,
        stopped: &|| stop.load(Ordering::SeqCst),
        hooks,
    });
    assert_eq!(account.snapshot().input.used, 0);
    assert!(account.snapshot().input.peak <= publish::ARCHIVE_WORKING_SET);
    result
}
/// The classic end record's central size and offset (no zip64 here).
fn central(bytes: &[u8]) -> (u64, u64) {
    let end = &bytes[bytes.len() - 22..];
    assert_eq!(end[..4], [0x50, 0x4b, 0x05, 0x06]);
    let field = |at: usize| u64::from(u32::from_le_bytes(end[at..at + 4].try_into().unwrap()));
    (field(12), field(16))
}
fn assert_written(scene: &Scene, root: &str, out: &Outcomes, code: Code) -> Vec<Vec<u8>> {
    let mut all = Vec::new();
    for (index, key) in KEYS.iter().enumerate() {
        let bytes = std::fs::read(scene.scratch.join(format!("{root}/{key}"))).unwrap();
        assert_eq!(bytes[..4], [0x50, 0x4b, 0x03, 0x04]);
        let record = out.archive(index as u64).unwrap();
        assert_eq!(record.code, code, "{key}");
        assert_eq!(record.size, bytes.len() as u64);
        assert_eq!(record.md5, Hashes::of(&bytes).md5);
        assert_eq!(record.etag().unwrap(), None, "directory ETag stays null");
        all.push(bytes);
    }
    // Archive temps are `.unityrs-tmp-r-<winners + spec index>`; none
    // survives success.
    for entry in std::fs::read_dir(scene.scratch.join(root)).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix(".unityrs-tmp-r-") {
            assert!(n.parse::<u64>().unwrap() < scene.handoff.counts.winners);
        }
    }
    all
}

#[test]
fn archives_follow_winners_and_resume_hashes_regenerated_bytes() {
    let scene = Scene::new(&["z", "a", "m"], true, 4096);
    let stop = AtomicBool::new(false);
    let dir = directory(&scene, "sink");
    let out = scene.outcomes("out1", &Host);
    let report = drive(&scene, &dir, &out, false, &publish::NoHooks, &stop).unwrap();
    assert_eq!(
        report,
        Admission::Drained(publish::Report {
            written: 3,
            ..publish::Report::default()
        })
    );
    let first = assert_written(&scene, "sink", &out, Code::Written);
    // Matching destinations are `present`, with the expected pass's metadata.
    let out = scene.outcomes("out2", &Host);
    drive(&scene, &dir, &out, true, &publish::NoHooks, &stop).unwrap();
    assert_eq!(assert_written(&scene, "sink", &out, Code::Present), first);
    // Same size, wrong content; and a wrong size: both regenerate identically.
    let mut damaged = first[0].clone();
    damaged[0] ^= 0xff;
    std::fs::write(scene.scratch.join("sink/z.zip"), damaged).unwrap();
    std::fs::write(scene.scratch.join("sink/a.zip"), b"short").unwrap();
    let out = scene.outcomes("out3", &Host);
    drive(&scene, &dir, &out, true, &publish::NoHooks, &stop).unwrap();
    assert_eq!(assert_written(&scene, "sink", &out, Code::Written), first);
}

#[test]
fn archive_destination_conflict_fails_only_that_archive() {
    let scene = Scene::new(&["z", "a", "m"], true, 4096);
    let stop = AtomicBool::new(false);
    let dir = directory(&scene, "sink");
    std::fs::create_dir(scene.scratch.join("sink/a.zip")).unwrap();
    let out = scene.outcomes("out", &Host);
    drive(&scene, &dir, &out, false, &publish::NoHooks, &stop).unwrap();
    for index in 0..scene.handoff.counts.winners {
        assert_eq!(out.loose(index).unwrap(), Code::Written);
    }
    assert_eq!(out.archive(0).unwrap().code, Code::Written);
    let failed = out.archive(1).unwrap();
    assert_eq!(failed.code, Code::PublishFailed);
    assert_eq!((failed.size, failed.md5), (0, [0; 16]));
    assert!(scene.scratch.join("sink/a.zip").is_dir());
}

#[test]
fn a_loose_conflict_leaves_archive_membership_and_bytes_intact() {
    let scene = Scene::new(&["z", "a", "m"], true, 4096);
    let stop = AtomicBool::new(false);
    let clean = directory(&scene, "clean");
    let out = scene.outcomes("clean-out", &Host);
    drive(&scene, &clean, &out, false, &publish::NoHooks, &stop).unwrap();
    let expected = assert_written(&scene, "clean", &out, Code::Written);
    let dir = directory(&scene, "sink");
    std::fs::create_dir(scene.scratch.join("sink/m")).unwrap();
    let out = scene.outcomes("out", &Host);
    drive(&scene, &dir, &out, false, &publish::NoHooks, &stop).unwrap();
    assert_eq!(
        assert_written(&scene, "sink", &out, Code::Written),
        expected
    );
}

/// Counts every payload opened through the single numeric seam.
struct Counted<'a> {
    inner: Committed<'a>,
    opened: AtomicUsize,
}
impl publish::payload::Payloads for Counted<'_> {
    fn open(&self, input: u32, plan: u64) -> Result<Box<dyn std::io::Read + Send>, Error> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        self.inner.open(input, plan)
    }
}
#[test]
fn oversized_is_refused_before_any_byte_and_exactly_limit_is_eligible() {
    assert_eq!(archive::eligibility(1, false, LIMIT), Eligibility::Eligible);
    assert_eq!(
        archive::eligibility(1, false, LIMIT + 1),
        Eligibility::Oversized
    );
    // Precedence: empty, then global withholding, then oversized.
    assert_eq!(archive::eligibility(0, true, LIMIT + 1), Eligibility::Empty);
    assert_eq!(
        archive::eligibility(1, true, LIMIT + 1),
        Eligibility::Withheld
    );
    let scene = Scene::new(&["z"], true, 4096);
    let dir = directory(&scene, "sink");
    let out = scene.outcomes("out", &Host);
    let account = Account::new(64 << 20);
    let payloads = Counted {
        inner: Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        },
        opened: AtomicUsize::new(0),
    };
    let cx = Context {
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        payloads: &payloads,
        directory: &dir,
        outcomes: &out,
        gate: &Gate::default(),
        account: &account,
        run_id: "r",
        resume: true,
        stopped: &|| false,
        hooks: &publish::NoHooks,
    };
    let claimed = scene.counter.used();
    let mut slot = publish::archive::Descriptor::new(account.meter()).unwrap();
    slot.set("big.zip", 1, 3, LIMIT + 1, None).unwrap();
    let mut buffer = vec![0; 64 * 1024];
    assert_eq!(
        publish::archive::one(&cx, &slot, &mut buffer).unwrap(),
        Some(Code::ArchiveTooLarge)
    );
    let record = out.archive(1).unwrap();
    assert_eq!(record.code, Code::ArchiveTooLarge);
    assert_eq!((record.size, record.md5), (0, [0; 16]));
    assert_eq!(payloads.opened.load(Ordering::SeqCst), 0);
    assert_eq!(scene.counter.used(), claimed, "no central scratch");
    assert_eq!(
        std::fs::read_dir(scene.scratch.join("sink"))
            .unwrap()
            .count(),
        1
    );
    // Empty and withheld archives write no outcome of their own.
    slot.set("e.zip", 0, 0, 22, None).unwrap();
    assert_eq!(
        publish::archive::one(&cx, &slot, &mut buffer).unwrap(),
        None
    );
    assert_eq!(out.archive(0).unwrap().code, Code::NotWritten);
    drop(slot);
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn a_payload_that_disagrees_with_sealed_metadata_is_internal_and_unwritten() {
    let scene = Scene::new(&["z"], true, 4096);
    let dir = directory(&scene, "sink");
    // Resume finds the loose output by its recorded MD5 without reading the
    // payload, so only the archive's expected-hash pass reads it.
    std::fs::write(scene.scratch.join("sink/z"), b"payload").unwrap();
    // Same size, other bytes: only the recomputed CRC-32/MD5 can notice.
    for input in 0..2 {
        let payload = scene.done.join(format!("{input}/o0"));
        let mut bytes = std::fs::read(&payload).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&payload, bytes).unwrap();
    }
    let out = scene.outcomes("out", &Host);
    let result = drive(
        &scene,
        &dir,
        &out,
        true,
        &publish::NoHooks,
        &AtomicBool::new(false),
    );
    assert_eq!(result, Err(Error::internal()));
    assert_eq!(out.loose(0).unwrap(), Code::Present);
    for index in 0..2 {
        assert_eq!(out.archive(index).unwrap().code, Code::NotWritten);
    }
    for key in KEYS {
        assert!(!scene.scratch.join(format!("sink/{key}")).exists());
    }
}

/// Records the spool's live claims at every archive buffer, and can stop at
/// the first buffer after `arm` transfers have begun.
struct Ledger<'a> {
    counter: &'a unityrs_batch::spool::Counter,
    base: u64,
    transfers: AtomicUsize,
    arm: usize,
    stop: &'a AtomicBool,
    peak: AtomicU64,
}
impl Hooks for Ledger<'_> {
    fn point(&self, point: Point) {
        match point {
            Point::Transfer => {
                self.transfers.fetch_add(1, Ordering::SeqCst);
            }
            Point::Buffer if self.transfers.load(Ordering::SeqCst) > self.arm => {
                let used = self.counter.used();
                self.peak.fetch_max(used, Ordering::SeqCst);
                // Stop once the sealed central file holds a claim.
                if self.arm != 0 && used > self.base {
                    self.stop.store(true, Ordering::SeqCst);
                }
            }
            _ => {}
        }
    }
}
#[test]
fn a_stop_mid_archive_keeps_its_scratch_claim_and_publishes_nothing() {
    let scene = Scene::new(&["z", "a", "m"], true, 4096);
    let stop = AtomicBool::new(false);
    let dir = directory(&scene, "sink");
    let out = scene.outcomes("out", &Host);
    let claimed = scene.counter.used();
    // Three loose transfers, then the first archive (`a.zip`, spec index 1)
    // once its central file is sealed and claimed.
    let hooks = Ledger {
        counter: &scene.counter,
        base: claimed,
        transfers: AtomicUsize::new(0),
        arm: 3,
        stop: &stop,
        peak: AtomicU64::new(0),
    };
    assert_eq!(
        drive(&scene, &dir, &out, false, &hooks, &stop),
        Err(Error::Stopped)
    );
    for index in 0..2 {
        assert_eq!(out.archive(index).unwrap().code, Code::NotWritten);
    }
    assert!(!scene.scratch.join("sink/a.zip").exists());
    // The partial temp may remain; its ordinal is winners + spec index.
    assert!(scene.scratch.join("sink/.unityrs-tmp-r-4").exists());
    // No Drop cleanup: the stopped owner's central claim stays until the
    // next start's owned cleanup.
    assert!(scene.counter.used() > claimed);
}

/// 2.9's ledger: at most one central file per active pass, retired before a
/// pass completes and before a resume miss transfers, so live archive scratch
/// stays within the largest `C_i` (serial) and every claim returns.
#[test]
fn central_scratch_is_retired_before_completion_and_before_a_miss_transfers() {
    for block in [512_u64, 3_072, 4_096, 5_003, 65_536] {
        let scene = Scene::new(&["z", "a", "m"], true, block);
        let stop = AtomicBool::new(false);
        let dir = directory(&scene, "sink");
        let out = scene.outcomes("out1", &Host);
        drive(&scene, &dir, &out, false, &publish::NoHooks, &stop).unwrap();
        let bytes = assert_written(&scene, "sink", &out, Code::Written);
        let largest = bytes
            .iter()
            .map(|b| central(b).0.div_ceil(block) * block)
            .max()
            .unwrap();
        // Every destination misses (wrong size), so each archive runs an
        // expected-hash pass and then a transfer.
        for key in KEYS {
            std::fs::write(scene.scratch.join(format!("sink/{key}")), b"x").unwrap();
        }
        let out = scene.outcomes("out2", &Host);
        let claimed = scene.counter.used();
        let hooks = Ledger {
            counter: &scene.counter,
            base: claimed,
            transfers: AtomicUsize::new(0),
            arm: 0,
            stop: &stop,
            peak: AtomicU64::new(0),
        };
        drive(&scene, &dir, &out, true, &hooks, &stop).unwrap();
        assert_eq!(assert_written(&scene, "sink", &out, Code::Written), bytes);
        assert_eq!(scene.counter.used(), claimed, "block {block}");
        let peak = hooks.peak.load(Ordering::SeqCst);
        assert!(peak > claimed, "central scratch was claimed");
        assert!(peak - claimed <= largest, "block {block}: {peak} {claimed}");
    }
}

#[test]
fn pool_drives_publish_archives_within_the_archive_envelope() {
    let mut harness =
        controller_support::Harness::new(&[("a", controller_support::clear(1), vec![])], &[], 2, 2);
    let controller = harness.controller(Arc::new(controller_support::Observe::default()));
    let mut execution = unityrs_batch::execution::Execution::new(&controller).unwrap();
    let mut expected: Option<Vec<Vec<u8>>> = None;
    for concurrency in [1, 2, 8] {
        let scene = Scene::new(&["z", "a", "m"], true, 4096);
        std::fs::create_dir(scene.scratch.join("sink")).unwrap();
        let sink = scene.scratch.join("sink");
        let directory =
            directory_support::preflight(&sink, 1, directory_support::fs(true)).unwrap();
        let outcomes = scene.outcomes("out", &Host);
        let owned = Arc::new(publish::Owned {
            prepared: unityrs_batch::sys::Lock::new(scene.prepared),
            done: scene.done,
            workspace: scene.workspace,
            handoff: scene.handoff,
            directory: Arc::new(directory),
            outcomes,
            account: Arc::new(Account::new(if concurrency == 1 {
                publish::ARCHIVE_WORKING_SET
            } else {
                64 << 20
            })),
            gate: Gate::default(),
            fs: Arc::new(Host),
            control: Arc::new(AtomicBool::new(false)),
            hooks: Arc::new(publish::NoHooks),
            progress: None,
            spool_lease: None,
        });
        let report = publish::parallel(&owned, &execution, concurrency).unwrap();
        assert_eq!(report.written, 3);
        let mut all = Vec::new();
        for (index, key) in KEYS.iter().enumerate() {
            let bytes = std::fs::read(sink.join(key)).unwrap();
            let record = owned.outcomes.archive(index as u64).unwrap();
            assert_eq!(record.code, Code::Written);
            assert_eq!(record.md5, Hashes::of(&bytes).md5);
            all.push(bytes);
        }
        if let Some(expected) = &expected {
            assert_eq!(&all, expected, "concurrency {concurrency}");
        } else {
            expected = Some(all);
        }
        assert_eq!(owned.account.snapshot().input.used, 0);
        assert!(owned.account.snapshot().input.peak <= owned.account.limit());
    }
    execution.quiesce().unwrap();
}

/// An archive over 64 MiB starts writeback periodically and once more after
/// its stream completes, always before its rename.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's scale job does"
)]
fn large_archives_start_writeback_every_64_mib_and_before_rename() {
    let payload = vec![0x5a; 40 << 20];
    let scene = Scene::with_payload(&["a", "b"], 4096, &payload);
    std::fs::create_dir(scene.scratch.join("sink")).unwrap();
    let adapter = directory_support::fs(true);
    let dir =
        directory_support::preflight(&scene.scratch.join("sink"), 1, adapter.clone()).unwrap();
    let out = scene.outcomes("out", &Host);
    adapter.calls.lock().unwrap().clear();
    drive(
        &scene,
        &dir,
        &out,
        false,
        &publish::NoHooks,
        &AtomicBool::new(false),
    )
    .unwrap();
    let calls = adapter.calls.lock().unwrap().clone();
    // Loose winners publish first, then each archive, each ending in rename.
    let renames: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == "rename")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(renames.len(), 4);
    for pair in renames.windows(2).skip(1) {
        let archive = &calls[pair[0] + 1..pair[1]];
        let writes = archive.iter().filter(|c| **c == "write").count();
        let writebacks = archive.iter().filter(|c| **c == "writeback").count();
        assert!(writes * (64 << 10) > 64 << 20, "{writes}");
        assert!(writebacks >= 2, "{writebacks}");
        let last = |name: &str| archive.iter().rposition(|c| *c == name).unwrap();
        assert!(
            last("write") < last("writeback"),
            "final writeback precedes rename"
        );
    }
    for index in 0..2 {
        let record = out.archive(index).unwrap();
        assert_eq!(record.code, Code::Written);
        assert!(record.size > 80 << 20);
    }
}
