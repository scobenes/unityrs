//! The owner join's late names under repeated refusal (B3's review round 5,
//! Q1): every texture's name read refused while the cutter reads them all
//! (`Attempt::names`), so each is late; then, as each texture's owner is
//! asked, the late names are read again, all but the last granted, and the
//! last refused at every join until its own texture reads it. The join
//! reads on from where it stopped, so the late names it looks at (this
//! crate's `testing` count, under its `test-counters` feature) grow with
//! the names and the asks, not with their product. Built under the feature
//! only; the count is the process's, so this binary holds one test.

#![cfg(feature = "test-counters")]

use std::backtrace::Backtrace;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::FileContext;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation, Unbounded};
use unityrs_core::limits::Meter;
use unityrs_export::testing;
use unityrs_testkit::ngui::{AtlasScene, CutOptions, SceneBundle, file, format, sheet, texture};

mod common;
use common::{Engine, Ran, run_in};

/// The textures.
const N: u64 = 40;

fn scene() -> AtlasScene {
    let objects: Vec<_> = (0..N)
        .map(|i| {
            let name = format!("t{i:02}");
            let path_id = 1 + i64::try_from(i).unwrap();
            texture(
                path_id,
                name.as_bytes(),
                format::RGBA32,
                (2, 2),
                sheet(0x10, 2, 2, false),
            )
        })
        .collect();
    let cab = file(&[], &objects, true).unwrap();
    AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".to_owned(),
            data: unityrs_testkit::env::bundle(&[("CAB-l1", cab)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions {
            texture_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    }
}

/// Refuses every reservation of a name's read inside `Attempt::names`, and
/// of one inside `Attempt::join_late` once `granted` of those are granted;
/// counts the latter granted, and every refusal (the engine's count).
#[derive(Debug)]
struct Late {
    granted: u64,
    joined: AtomicU64,
    refusals: Arc<AtomicU64>,
}

impl MemoryBudget for Late {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let stack = Backtrace::force_capture().to_string();
        let stack: String = stack.chars().filter(|c| !matches!(c, '<' | '>')).collect();
        let peek = stack.contains("ngui::read::Reader::peek_name");
        let refuse = peek
            && (stack.contains("ngui::attempt::Attempt::names")
                || stack.contains("ngui::attempt::Attempt::join_late")
                    && self.joined.fetch_add(1, Ordering::AcqRel) >= self.granted);
        if refuse {
            self.refusals.fetch_add(1, Ordering::AcqRel);
            return Err(BudgetExceeded {
                requested: bytes,
                what,
            });
        }
        Unbounded.try_reserve(bytes, what)
    }
}

/// The cut under [`Late`] granting `granted`: the join's reservations
/// counted, and the cut.
fn cut(s: &AtlasScene, granted: u64) -> (u64, Ran) {
    let late = Arc::new(Late {
        granted,
        joined: AtomicU64::new(0),
        refusals: Arc::new(AtomicU64::new(0)),
    });
    let engine = Engine {
        refusals: Some(late.refusals.clone()),
        ..Engine::default()
    };
    let meter = Meter::new(u64::MAX, late.clone(), Reservation::Input);
    let ran = run_in(s, "x.bundle", FileContext::default(), &meter, &engine).unwrap();
    (late.joined.load(Ordering::Acquire), ran)
}

#[test]
fn a_late_name_refused_at_every_join_is_looked_at_once_a_join() {
    let s = scene();
    // Every join granted: the first texture's own read takes its name, the
    // first join reads the others', in `all` reservations.
    let (all, ran) = cut(&s, u64::MAX);
    assert_eq!(
        ran.handed.len(),
        usize::try_from(N).unwrap(),
        "{:?}",
        ran.skips
    );
    assert!(all >= N - 1, "{all} reservations for {} names", N - 1);
    // All but the join's last reservation granted: the first join reads
    // N - 2 names and is refused in the last, which every later join meets
    // first, refused at once, until the last texture's own read takes it.
    let before = testing::late_scans();
    let (_, ran) = cut(&s, all - 1);
    let scans = testing::late_scans() - before;
    let budget: Vec<_> = ran
        .skips
        .iter()
        .filter(|k| k.reason.reason == "budget(input)")
        .collect();
    assert_eq!(
        budget.len(),
        usize::try_from(N - 1).unwrap(),
        "{:?}",
        ran.skips
    );
    assert_eq!(ran.handed.len(), 1);
    // The first join looks at N names, each later one at one: at most 2N,
    // where a join that looked from the first again would look at about
    // N x N / 2.
    assert!(scans <= 2 * N, "{scans} late names looked at");
}
