//! Counted children: every selection allocation ordinal is refused once.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
use std::{cell::Cell, process::Command, sync::Arc};
use unityrs_batch::{
    alloc::{self, Counting, Hook},
    attempt::ReasonCode,
    phase::{Account, Error},
    sort::{NoHooks, Options},
    spool::{Counter, Host, workspace::Workspace},
    winners::{self, Context, Observer, Pass, Prepared},
};
use unityrs_testkit::{child, scratch::Scratch};
thread_local! {
    static ACTIVE:Cell<bool>=const {Cell::new(false)};
    static SITE:Cell<usize>=const {Cell::new(0)};
    static SEEN:Cell<usize>=const {Cell::new(0)};
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
fn one_case() -> harness::Case {
    use harness::candidate;
    let mut rows = Vec::new();
    for input in ["a", "b"] {
        for key in ["unit", "unit/a", "elsewhere"] {
            rows.push(candidate(input, key, Some("unit"), 1));
        }
        rows.push(candidate(input, "plain", None, 2));
    }
    harness::Case::new(rows)
}
// Shared keys arrive in key order but emit loser markers z, then m.
// Their roots and unique keys must remain reserved after sorting the markers.
fn conflicts_case() -> harness::Case {
    use harness::candidate;
    harness::Case::new(vec![
        candidate("a", "a", Some("a"), 1),
        candidate("b", "a", Some("a"), 2),
        candidate("a", "shared-a", Some("a"), 1),
        candidate("a", "shared-z", Some("a"), 1),
        candidate("b", "z", Some("z"), 3),
        candidate("b", "z-only", Some("z"), 3),
        candidate("b", "shared-a", Some("z"), 3),
        candidate("b", "m", Some("m"), 4),
        candidate("b", "m-only", Some("m"), 4),
        candidate("b", "shared-z", Some("m"), 4),
        candidate("c", "z-only", None, 5),
        candidate("c", "m-only", None, 5),
        candidate("a", "plain", None, 1),
        candidate("b", "plain", None, 2),
    ])
}
struct MarkerSort {
    pass: Cell<Pass>,
    encoded: Cell<u64>,
    runs: Cell<u64>,
}
impl Observer for MarkerSort {
    fn pass(&self, pass: Pass, _: u64) {
        self.pass.set(pass);
    }
    fn sort(&self, _: u64, encoded: u64, runs: u64) {
        if self.pass.get() == Pass::Keepers {
            self.encoded.set(encoded);
            self.runs.set(runs);
        }
    }
}
fn check_scene(
    case: &harness::Case,
    cx: &Context<'_>,
    artifacts: &winners::Artifacts,
    observer: &MarkerSort,
    conflicts: bool,
) {
    if conflicts {
        assert_eq!(
            case.decisions(cx, artifacts),
            unityrs_export::ngui::decide(&case.candidates, &case.prefer, &case.failed, true)
        );
        assert!(
            observer.encoded.get() > 0,
            "conflict markers were not sorted"
        );
        assert!(observer.runs.get() > 0);
        assert_eq!(artifacts.counts.flips, 2);
        let decisions = case.decisions(cx, artifacts);
        for key in ["m", "m-only", "z", "z-only"] {
            let d = decisions.iter().find(|d| d.key == key).unwrap();
            assert!(!d.publish);
            assert_eq!(d.reason, Some("unit_conflict"));
        }
    } else {
        assert_eq!(observer.encoded.get(), 0);
    }
}
#[test]
fn allocation_child() {
    if std::env::var_os("UNITYRS_WINNERS_ALLOC").is_none() {
        return;
    }
    let scratch = Scratch::new("winner-allocation");
    let conflicts = std::env::var("UNITYRS_WINNERS_ALLOC_SCENE").unwrap() == "conflicts";
    let case = if conflicts {
        conflicts_case()
    } else {
        one_case()
    };
    let spec = case.spec().spec;
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let setup = Account::new(64 << 20);
    let (commits, _) = case.commit(&scratch, &counter, &setup, &spec);
    drop(setup);
    let prepared = Prepared::from_spec(&spec).unwrap();
    let mut allocations = 0;
    for site in 0..=10_000 {
        if site > allocations && site > 0 {
            break;
        }
        let account = Account::new(64 << 20);
        let mut workspace = Workspace::create(
            scratch.join(format!("w{site}")),
            counter.clone(),
            Arc::new(Host),
        )
        .unwrap();
        let observer = MarkerSort {
            pass: Cell::new(Pass::Roots),
            encoded: Cell::new(0),
            runs: Cell::new(0),
        };
        let mut cx = Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options {
                run_records: 2,
                fan_in: 2,
                ..Options::default()
            },
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &observer,
        };
        SEEN.with(|s| s.set(0));
        SITE.with(|s| s.set(site));
        ACTIVE.with(|a| a.set(true));
        let (result, measured) = alloc::measured(|| winners::run(&mut cx));
        ACTIVE.with(|a| a.set(false));
        if site == 0 {
            check_scene(&case, &cx, result.as_ref().unwrap(), &observer, conflicts);
            allocations = SEEN.with(Cell::get);
            assert!(allocations < 10_000);
            assert!(measured.peak <= 40 << 20, "{measured:?}");
            println!(
                "WINNERS_ALLOC conflicts={conflicts} allocations={allocations} counted_peak={} account_peak={}",
                measured.peak,
                account.snapshot().input.peak
            );
        } else {
            assert_eq!(SITE.with(Cell::get), 0, "unreached allocation {site}");
            assert!(
                matches!(result,Err(Error::Run(f)) if f.reason==ReasonCode::HostMemory),
                "site {site}: {result:?}"
            );
        }
        assert_eq!(account.snapshot().input.used, 0);
        assert_eq!(measured.kept, 0);
    }
    let account = Account::new(100);
    let mut workspace = Workspace::create(scratch.join("budget"), counter, Arc::new(Host)).unwrap();
    let result = winners::run(&mut Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &commits,
        workspace: &mut workspace,
        account: &account,
        options: Options::default(),
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &winners::NoObserver,
    });
    assert_eq!(result.unwrap_err(), Error::internal());
    assert_eq!(account.snapshot().input.used, 0);
}
fn counted_child(scene: &str) {
    let output = child::output(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "allocation_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("UNITYRS_WINNERS_ALLOC", "1")
            .env("UNITYRS_WINNERS_ALLOC_SCENE", scene),
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
fn every_phase_allocation_is_fallible_and_charges_clear_in_counted_child() {
    counted_child("simple");
}

#[test]
fn conflict_marker_reservation_and_flip_allocations_are_fallible() {
    counted_child("conflicts");
}
