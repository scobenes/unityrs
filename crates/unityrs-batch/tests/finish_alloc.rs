mod common;
mod publication_support;
use publication_support::{Portable, Scene};
use std::{cell::Cell, process::Command, sync::Arc};
use unityrs_batch::{
    alloc::{self, Counting, Hook},
    finish, manifest,
    phase::Account,
    publish::Gate,
    reason::Code,
    sink::directory::Directory,
    spool::Host,
    winners,
};
use unityrs_testkit::{child, guard};
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static SITE: Cell<usize> = const { Cell::new(0) };
    static SEEN: Cell<usize> = const { Cell::new(0) };
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

fn completed_outcomes(scene: &Scene, site: usize) -> unityrs_batch::outcomes::Outcomes {
    let outcomes = scene.outcomes(&format!("out-{site}"), &Host);
    for index in 0..scene.handoff.counts.winners {
        outcomes
            .set(index, unityrs_batch::outcomes::Code::Written)
            .unwrap();
    }
    for index in 0..u64::try_from(scene.prepared.spec.archives.len()).unwrap() {
        outcomes
            .set_archive(
                index,
                &unityrs_batch::outcomes::Archive::success(
                    unityrs_batch::outcomes::Code::Written,
                    10,
                    [0; 16],
                    None,
                )
                .unwrap(),
            )
            .unwrap();
    }
    outcomes
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn finisher_ceiling_and_every_reachable_refusal_in_counted_children() {
    let output = guard::finishes("finisher allocations", || {
        child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "finisher_child", "--nocapture"])
                .env("UNITYRS_FINISH_ALLOC", "1"),
        )
        .unwrap()
    });
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
#[test]
fn finisher_child() {
    if std::env::var_os("UNITYRS_FINISH_ALLOC").is_none() {
        return;
    }
    for (count, archives) in [(1, false), (128, false), (1, true), (128, true)] {
        let keys: Vec<String> = (0..count).map(|i| format!("key-{i:04}")).collect();
        let refs: Vec<_> = keys.iter().map(String::as_str).collect();
        let scene = Scene::new(&refs, archives, 4096);
        let mut allocations = 0;
        let mut site = 0;
        loop {
            let sink = scene.scratch.join("sink");
            if sink.exists() {
                std::fs::remove_dir_all(&sink).unwrap();
            }
            std::fs::create_dir(&sink).unwrap();
            let directory = Directory::preflight(
                &sink,
                "r",
                1,
                Arc::new(Portable),
                &mut vec![0; manifest::head::HEAD_LIMIT + 1],
            )
            .unwrap();
            let outcomes = completed_outcomes(&scene, site);
            let gate = Gate::default();
            let frozen = gate.freeze(&outcomes).unwrap();
            let account = Account::new(unityrs_batch::bound::C_FIN);
            let inputs = winners::committed::Committed::received(
                &scene.done,
                &scene.prepared.inputs,
                &Host,
                &|| false,
            )
            .unwrap();
            let cx = manifest::rows::Context {
                facts: &scene.prepared,
                inputs: &inputs,
                selected: Some(manifest::rows::Selected {
                    handoff: &scene.handoff,
                    workspace: &scene.workspace,
                    outcomes: frozen.outcomes,
                }),
                stop: None,
                found: None,
                account: &account,
                stopped: &|| false,
                taken_at: std::time::Instant::now(),
            };
            let identity = finish::Identity {
                started_at: "2026-01-01T00:00:00Z",
                memory_bound: 0,
                spool_peak: 0,
                vm_hwm: 0,
            };
            finish::running(&scene.prepared, &directory, &account, &identity).unwrap();
            SEEN.with(|s| s.set(0));
            SITE.with(|s| s.set(site));
            ACTIVE.with(|s| s.set(true));
            let (result, measured) = alloc::measured(|| {
                finish::normal(&cx, &directory, &identity, "2026-01-01T00:00:01Z")
            });
            ACTIVE.with(|s| s.set(false));
            if site == 0 {
                assert_eq!(result.unwrap().code, Code::Complete);
                allocations = SEEN.with(Cell::get);
                assert!(measured.peak <= unityrs_batch::bound::C_FIN, "{measured:?}");
                println!(
                    "FINISH_ALLOC count={count} archives={archives} sites={allocations} peak={} charged={}",
                    measured.peak,
                    account.snapshot().input.peak
                );
            } else {
                assert_eq!(SITE.with(Cell::get), 0, "unreached {site}");
                assert_eq!(
                    result.unwrap().code,
                    Code::Host,
                    "count={count} site={site}"
                );
            }
            let head: serde_json::Value = serde_json::from_slice(
                &std::fs::read(sink.join("_unityrs/r/manifest.json")).unwrap(),
            )
            .unwrap();
            assert!(
                head["stop_reason"].is_null(),
                "a normal snapshot gained a stop at site {site}"
            );
            assert_eq!(measured.kept, 0);
            assert_eq!(account.snapshot().input.used, 0);
            if site == allocations {
                break;
            }
            site += 1;
        }
        stopped_scope(&refs, archives);
    }
}
/// A phase-7 hand-over by a stop whose outcome files were left for a view.
fn stopped_handover(
    keys: &[&str],
    archives: bool,
) -> (unityrs_testkit::scratch::Scratch, finish::HandOver) {
    use unityrs_batch::{
        finish::{FinishCause, HandOver, Selected, Spool},
        progress::Phase,
    };
    let scene = Scene::new(keys, archives, 4096);
    let run = scene.scratch.join("run");
    std::fs::create_dir(&run).unwrap();
    drop(completed_outcomes(&scene, 0));
    for name in ["outcomes", "archive_outcomes"] {
        std::fs::rename(scene.scratch.join("out-0").join(name), run.join(name)).unwrap();
    }
    let Scene {
        scratch,
        prepared,
        done,
        workspace,
        handoff,
        ..
    } = scene;
    let mut h = HandOver::new(FinishCause::Stop, Phase::Publish);
    h.running = true;
    // A pending commit whose rename landed: the one stat finds it.
    h.pending = Some(0);
    h.facts = Some(unityrs_batch::sys::Shared::new(prepared).unwrap());
    h.selected = Some(Selected::Parts { workspace, handoff });
    h.spool = Some(Spool {
        run,
        done,
        fs: Arc::new(Host),
        lease: None,
    });
    (scratch, h)
}
/// The stopped finisher over a phase-7 hand-over whose outcome files are a
/// view (the controller's owner held by a task): every row family, the
/// reconciling stat, `syncfs` and the head, within `C_fin`, and every
/// refusal a 74 with the stop's head shape kept.
fn stopped_scope(keys: &[&str], archives: bool) {
    use unityrs_batch::{lifetime::Stop, reason::StopReason};
    let (scratch, mut h) = stopped_handover(keys, archives);
    let taken = Some(Stop::external(StopReason::Sigterm));
    let identity = finish::Identity {
        started_at: "2026-01-01T00:00:00Z",
        memory_bound: 0,
        spool_peak: 0,
        vm_hwm: 0,
    };
    let mut allocations = 0;
    let mut site = 0;
    loop {
        let sink = scratch.join("sink");
        if sink.exists() {
            std::fs::remove_dir_all(&sink).unwrap();
        }
        std::fs::create_dir(&sink).unwrap();
        let directory = Arc::new(
            Directory::preflight(
                &sink,
                "r",
                1,
                Arc::new(Portable),
                &mut vec![0; manifest::head::HEAD_LIMIT + 1],
            )
            .unwrap(),
        );
        let account = Account::new(unityrs_batch::bound::C_FIN);
        let facts = h.facts.as_deref().unwrap();
        finish::running(facts, &directory, &account, &identity).unwrap();
        h.directory = Some(directory);
        SEEN.with(|s| s.set(0));
        SITE.with(|s| s.set(site));
        ACTIVE.with(|s| s.set(true));
        let (finished, measured) = alloc::measured(|| {
            finish::run(
                &h,
                taken,
                &mut finish::Finisher {
                    account: &account,
                    identity: &identity,
                    budget: finish::budget::Budget::UNARMED,
                    clock: &finish::budget::BootClock,
                    backend: &mut finish::budget::NoJournal,
                },
            )
        });
        ACTIVE.with(|s| s.set(false));
        let head: serde_json::Value =
            serde_json::from_slice(&std::fs::read(sink.join("_unityrs/r/manifest.json")).unwrap())
                .unwrap();
        if site == 0 {
            assert_eq!(finished.end.code, Code::Stopped);
            assert_eq!(finished.reconcile, Some(finish::Reconcile::Found(0)));
            allocations = SEEN.with(Cell::get);
            assert!(measured.peak <= unityrs_batch::bound::C_FIN, "{measured:?}");
            assert_eq!(head["status"], "incomplete");
            println!(
                "FINISH_ALLOC_STOPPED count={} archives={archives} sites={allocations} peak={} charged={}",
                keys.len(),
                measured.peak,
                account.snapshot().input.peak
            );
        } else {
            assert_eq!(SITE.with(Cell::get), 0, "unreached {site}");
            assert_eq!(finished.end.code, Code::Host, "stopped site={site}");
        }
        // A refused head write leaves `running`; any other keeps the stop.
        if head["status"] != "running" {
            assert_eq!(head["stop_reason"], "sigterm", "stopped site {site}");
            assert_eq!(head["phase"], "publish", "stopped site {site}");
        }
        assert_eq!(measured.kept, 0);
        assert_eq!(account.snapshot().input.used, 0);
        if site == allocations {
            break;
        }
        site += 1;
    }
}
