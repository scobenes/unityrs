mod artifact_support;
mod peek_support;
mod stage_support;
use peek_support::{Harness, bytes};
use std::{
    cell::{Cell, RefCell},
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Arc, atomic::Ordering},
};
use unityrs_batch::{
    attempt::{DetailCode, PeekEntry, PeekTable, ReasonCode},
    peek::{
        self, Hooks, PeekEnd, Point, State,
        word::{FailureCode, Outcome, Word},
    },
    reason::LimitKind,
};
use unityrs_core::{budget::Reservation, limits::Meter};
use unityrs_export::ngui::{self, PeekBits as _};

#[test]
fn marked_files_have_exact_lsb_layout_padding_mode_claims_and_audited_ends() {
    for (bundle, inputs, name) in [(0, 1, "pi0"), (2, 1, "pd1")] {
        let mut h = Harness::new(&bytes(true));
        let end = h.run(bundle, inputs, 1 << 20, &ngui::NoHooks);
        assert_eq!(end.state, State::Done, "{end:?}");
        assert_eq!(end.outcome, Some(Outcome::Disk(11)));
        assert_eq!(h.word.load(), Ok(Outcome::Disk(11)));
        let file = h.file.as_mut().unwrap();
        assert_eq!(file.path(), h.directory.join(name));
        assert_eq!(fs::read(file.path()).unwrap(), [0x81, 0x05]);
        assert_eq!(fs::metadata(file.path()).unwrap().len(), 11u64.div_ceil(8));
        assert_eq!(
            fs::metadata(file.path()).unwrap().permissions().mode() & 0o777,
            0o400
        );
        assert!(file.is_sealed());
        assert_eq!(end.claimed_bytes, h.counter.used());
        assert_eq!(end.claimed_bytes, 4096);
        assert!(end.input_peak > 0 && end.decode_work > 0);
        assert!(end.accounts.is_clear() && end.slot_releasable);
        assert_eq!(end.accounts.dependency.peak, 0);
        assert_eq!(end.accounts.provider_refusals, 0);
        file.remove().unwrap();
        assert_eq!(h.counter.used(), 0);
    }
}

#[test]
fn all_clear_creates_no_file_and_data_failures_are_done_outcomes() {
    let mut h = Harness::new(&bytes(false));
    let end = h.run(0, 1, 1 << 20, &ngui::NoHooks);
    assert_eq!(end.state, State::Done, "{end:?}");
    assert_eq!(h.word.load(), Ok(Outcome::Clear(11)));
    assert!(h.file.is_none());
    assert!(!h.directory.join("pi0").exists());
    assert_eq!(h.counter.used(), 0);
    for (body, m, expected) in [
        (b"plain data".to_vec(), 1 << 20, FailureCode::NotABundle),
        (bytes(true), 1, FailureCode::Limit(LimitKind::InputBytes)),
    ] {
        let mut h = Harness::new(&body);
        let end = h.run(0, 1, m, &ngui::NoHooks);
        assert_eq!(end.state, State::Done, "{end:?}");
        assert_eq!(end.outcome, Some(Outcome::Failed(expected)));
        assert_eq!(h.word.load(), Ok(Outcome::Failed(expected)));
        assert!(h.file.is_none());
        assert!(end.slot_releasable && end.accounts.is_clear());
    }
}

#[test]
fn slice_and_run_tables_use_the_same_checked_disk_loader() {
    let mut h = Harness::new(&bytes(true));
    assert_eq!(h.run(0, 1, 1 << 20, &ngui::NoHooks).state, State::Done);
    let v = stage_support::spec(&h.staged, None, None);
    let words = [Word::default()];
    words[0].publish(Outcome::Disk(11)).unwrap();
    let table = peek::PeekBits::new(&v.spec, &v.graph, &words, &h.directory).unwrap();
    let entries = [PeekEntry {
        id: "x",
        rows: 11,
        file: Some(h.file.as_ref().unwrap().path()),
    }];
    let slice = PeekTable::new(&entries).unwrap();
    for loader in [&table as &dyn ngui::PeekBits, &slice as &dyn ngui::PeekBits] {
        let (budget, meter) = artifact_support::meter();
        let bits = loader.load("x", &meter, &ngui::NoHooks).unwrap();
        assert_eq!(bits.rows(), 11);
        assert_eq!(bits.bytes(), [0x81, 0x05]);
        assert!(bits.get(0) && bits.get(7) && bits.get(8) && bits.get(10));
        assert!(!bits.get(1) && !bits.get(9));
        drop(bits);
        assert!(budget.snapshot().is_clear());
        assert!(
            matches!(loader.load("unknown", &meter, &ngui::NoHooks), Err(ngui::Failure::Run(r)) if r.reason == "internal")
        );
    }
    let path = h.file.as_ref().unwrap().path();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    for body in [&[0x81][..], &[0x81, 0x05, 0][..], &[0x81, 0x85][..]] {
        fs::write(path, body).unwrap();
        for loader in [&table as &dyn ngui::PeekBits, &slice as &dyn ngui::PeekBits] {
            let (budget, meter) = artifact_support::meter();
            assert!(
                matches!(loader.load("x", &meter, &ngui::NoHooks), Err(ngui::Failure::Run(r)) if r.reason == "internal")
            );
            assert!(budget.snapshot().is_clear());
        }
    }
    fs::remove_file(path).unwrap();
    let (_, meter) = artifact_support::meter();
    for loader in [&table as &dyn ngui::PeekBits, &slice as &dyn ngui::PeekBits] {
        assert!(
            matches!(loader.load("x", &meter, &ngui::NoHooks), Err(ngui::Failure::Run(r)) if r.reason == "host_io")
        );
    }
    let budget = Arc::new(unityrs_batch::budget::SealedBudget::input_only(0));
    let meter = Meter::new(0, budget.clone(), Reservation::Input);
    assert!(matches!(
        table.load("x", &meter, &ngui::NoHooks),
        Err(ngui::Failure::Input(_))
    ));
    assert!(budget.snapshot().is_clear());
}

struct Observer {
    requested: Cell<bool>,
    seen: RefCell<Vec<usize>>,
}
impl Observer {
    fn new() -> Self {
        Self {
            requested: Cell::new(false),
            seen: RefCell::new(vec![]),
        }
    }
}
impl ngui::Hooks for Observer {
    fn meter(&self, meter: &Meter) {
        self.seen
            .borrow_mut()
            .push(std::ptr::from_ref(meter) as usize);
    }
}
impl Hooks for Observer {
    fn request_stop(&self, _: &PeekEnd) {
        self.requested.set(true);
    }
}
#[test]
fn fresh_single_meter_post_drop_audit_panic_containment_and_duplicate_publication() {
    let mut h = Harness::new(&bytes(false));
    let hooks = Observer::new();
    let end = h.run(0, 1, 1 << 20, &hooks);
    assert_eq!(end.state, State::Done);
    assert!(hooks.seen.borrow().len() >= 2);
    assert!(
        hooks
            .seen
            .borrow()
            .iter()
            .all(|p| *p == hooks.seen.borrow()[0])
    );
    assert!(!hooks.requested.get());
    let end = h.run(0, 1, 1 << 20, &ngui::NoHooks);
    assert_eq!(end.state, State::RunFailure);
    assert_eq!(end.failure.unwrap().reason, ReasonCode::Internal);
}

#[test]
fn peek_spool_faults_retain_owners_claims_and_do_not_publish_words() {
    for operation in [1, 2, 3, 5] {
        for (errno, detail) in [
            (libc::ENOSPC, DetailCode::Enospc),
            (libc::EDQUOT, DetailCode::Edquot),
        ] {
            let mut h = Harness::new(&bytes(true));
            let faults = Arc::new(artifact_support::Faults::default());
            faults.set(operation, errno);
            h.fs = faults;
            let hooks = Observer::new();
            let end = h.run(0, 1, 1 << 20, &hooks);
            assert_eq!(end.state, State::RunFailure);
            let failure = end.failure.unwrap();
            assert_eq!(failure.reason, ReasonCode::SpoolFull);
            assert_eq!(failure.detail, Some(detail));
            assert!(hooks.requested.get());
            assert_eq!(h.word.load(), Ok(Outcome::Pending));
            assert!(end.slot_releasable && end.accounts.is_clear());
            assert_eq!(h.counter.used(), if operation == 1 { 0 } else { 4096 });
        }
    }
}

/// Panics just after the peek word is published.
struct Stop;
impl ngui::Hooks for Stop {}
impl Hooks for Stop {
    fn point(&self, point: Point) {
        assert!(point != Point::AfterWord, "after publication");
    }
}

#[test]
fn stop_after_the_word_keeps_it_but_returns_no_success_end() {
    let mut h = Harness::new(&bytes(true));
    let end = h.run(0, 1, 1 << 20, &Stop);
    assert_eq!(end.state, State::RunFailure);
    assert_eq!(end.failure.unwrap().reason, ReasonCode::Internal);
    assert_eq!(h.word.load(), Ok(Outcome::Disk(11)));
    assert!(end.outcome.is_none());
    assert!(h.file.as_ref().unwrap().is_sealed());
    assert_eq!(fs::read(h.directory.join("pi0")).unwrap(), [0x81, 0x05]);
    let mut h = Harness::new(&bytes(true));
    h.stop.store(true, Ordering::Release);
    let end = h.run(0, 1, 1 << 20, &ngui::NoHooks);
    assert_eq!(end.state, State::Stopped);
    assert_eq!(h.word.load(), Ok(Outcome::Pending));
    assert!(h.file.is_none());
}

#[test]
fn cooperative_stops_before_seal_and_after_publication_keep_owned_artifacts() {
    struct Stop {
        at: Point,
        stopped: Cell<bool>,
    }
    impl ngui::Hooks for Stop {
        fn stopped(&self) -> bool {
            self.stopped.get()
        }
    }
    impl Hooks for Stop {
        fn point(&self, point: Point) {
            if point == self.at {
                self.stopped.set(true);
            }
        }
    }
    for at in [Point::BeforeSeal, Point::AfterWord] {
        let mut h = Harness::new(&bytes(true));
        let stop = Stop {
            at,
            stopped: Cell::new(false),
        };
        let end = h.run(0, 1, 1 << 20, &stop);
        assert_eq!(end.state, State::Stopped);
        assert_eq!(end.outcome, None);
        assert_eq!(
            h.word.load(),
            Ok(if at == Point::AfterWord {
                Outcome::Disk(11)
            } else {
                Outcome::Pending
            })
        );
        let file = h.file.as_ref().unwrap();
        assert_eq!(file.is_sealed(), at == Point::AfterWord);
        assert_eq!(fs::read(file.path()).unwrap(), [0x81, 0x05]);
        assert_eq!(end.claimed_bytes, 4096);
        assert!(end.accounts.is_clear() && end.slot_releasable);
    }
}

#[test]
fn compressed_row_limit_after_open_fails_the_peek_without_success_word() {
    use unityrs_core::{
        Error, FileContext, LimitKind as CoreLimit, RawStr,
        detect::FileType,
        files::{File, parse_file_with_reader},
        stream::ReaderKind,
    };
    use unityrs_testkit::bundle::{self, BundleSpec, EntrySpec};

    const M: u64 = 1 << 20;
    let body = bundle::build(&BundleSpec {
        block_size: 2 << 20,
        entries: vec![EntrySpec {
            name: b"large-row".to_vec(),
            data: vec![0; 2 << 20],
            flags: 0,
        }],
        ..BundleSpec::default()
    });
    assert!(u64::try_from(body.len()).unwrap() < M);
    let mut h = Harness::new(&body);
    // Prove this same context admits the header and directory, and only the
    // compressed row's read meets the limit, before driving the sealed peek.
    let budget = Arc::new(unityrs_batch::budget::SealedBudget::input_only(M));
    let mut ctx = FileContext::unitypy();
    ctx.budget = budget.clone();
    ctx.limits.max_input_memory = M;
    ctx.limits.max_input_bytes = M;
    ctx.limits.max_decode_work = 4 * M;
    let meter = Meter::for_input(&ctx.limits, budget.clone(), Reservation::Input);
    {
        let bytes = unityrs_core::env::read_path(&h.staged, &ctx.limits, &meter).unwrap();
        let File::Bundle(opened) = parse_file_with_reader(
            RawStr::from_static(b"i0"),
            bytes,
            FileType::BundleFile,
            None,
            &meter,
            &Arc::new(ctx),
            ReaderKind::Stream,
        )
        .unwrap() else {
            panic!("bundle fixture");
        };
        assert_eq!(opened.directory().len(), 1);
        let error = opened.entry_range(0, 0..21).unwrap_err();
        assert!(
            matches!(error, Error::Limit(ref l) if l.kind() == CoreLimit::InputMemory && l.max() == M),
            "{error:?}"
        );
    }
    assert!(budget.snapshot().is_clear());
    let end = h.run(0, 1, M, &ngui::NoHooks);
    let failed = Outcome::Failed(FailureCode::Limit(LimitKind::InputMemory));
    assert_eq!(end.state, State::Done, "{end:?}");
    assert_eq!(end.outcome, Some(failed));
    assert_eq!(h.word.load(), Ok(failed));
    assert!(h.file.is_none());
    assert_eq!(h.counter.used(), 0);
    assert!(end.accounts.is_clear() && end.slot_releasable);
}
