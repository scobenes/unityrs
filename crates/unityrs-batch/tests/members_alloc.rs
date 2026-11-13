//! Counted children: every complete-phase and downstream-reader allocation ordinal.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
#[path = "winners_support/member_scene.rs"]
mod member_scene;
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
    member_scene::scene(true)
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
fn read_handoff(cx: &Context<'_>, handoff: &winners::Handoff) -> Result<(), Error> {
    use winners::rowread;
    for (file, kind) in [
        (handoff.outputs.as_ref(), rowread::File::Outputs),
        (handoff.collisions.as_ref(), rowread::File::Collisions),
        (handoff.archives.as_ref(), rowread::File::Archives),
        (handoff.archive_members.as_ref(), rowread::File::Members),
    ] {
        let mut reader =
            rowread::Reader::open(cx.workspace, file, kind, cx.account.meter(), cx.stopped)?;
        while reader.next(cx.stopped)?.is_some() {}
    }
    if let Some(file) = &handoff.metadata {
        let mut reader =
            winners::archives::Reader::open(cx.workspace, file, cx.account.meter(), cx.stopped)?;
        while let Some(a) = reader.next(cx.stopped)? {
            if let Some(list) = a.list {
                let mut reader = winners::member::Reader::open(
                    cx.workspace,
                    &list,
                    cx.account.meter(),
                    cx.stopped,
                )?;
                while reader.next(cx.stopped)?.is_some() {}
            }
        }
    }
    Ok(())
}
#[test]
#[allow(clippy::too_many_lines)] // One counted child owns setup, refusal sweep and audits.
fn allocation_child() {
    if std::env::var_os("UNITYRS_MEMBERS_ALLOC").is_none() {
        return;
    }
    let scratch = Scratch::new("winner-allocation");
    let conflicts = std::env::var("UNITYRS_MEMBERS_ALLOC_SCENE").unwrap() == "conflicts";
    let case = if conflicts {
        conflicts_case()
    } else {
        one_case()
    };
    let spec = case.spec().spec;
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let setup = Account::new(64 << 20);
    let (commits, _) = case.commit(&scratch, &counter, &setup, &spec);
    let done = scratch.join("done");
    std::fs::create_dir(&done).unwrap();
    for (i, path) in commits.paths.iter().enumerate() {
        std::fs::rename(path.parent().unwrap(), done.join(i.to_string())).unwrap();
    }
    let states: Vec<_> = (0..commits.paths.len())
        .map(|_| unityrs_batch::phase1::InputState {
            phase: unityrs_batch::phase1::InputPhase::Committed,
            failure_bundle: None,
            ..Default::default()
        })
        .collect();
    let committed =
        winners::committed::Committed::new(&done, &states, &commits.failed, &Host, &|| false)
            .unwrap();
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
            inputs: &committed,
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
        let (result, measured) = alloc::measured(|| {
            account.execute(|| {
                let handoff = winners::complete(&mut cx)?;
                read_handoff(&cx, &handoff)?;
                Ok(handoff)
            })
        });
        ACTIVE.with(|a| a.set(false));
        if site == 0 {
            assert!(result.as_ref().unwrap().outputs.is_some());
            allocations = SEEN.with(Cell::get);
            assert!(allocations < 10_000);
            assert!(measured.peak <= 40 << 20, "{measured:?}");
            println!(
                "MEMBERS_ALLOC conflicts={conflicts} allocations={allocations} counted_peak={} account_peak={}",
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
    let result = winners::complete(&mut Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &committed,
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
            .env("UNITYRS_MEMBERS_ALLOC", "1")
            .env("UNITYRS_MEMBERS_ALLOC_SCENE", scene),
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
