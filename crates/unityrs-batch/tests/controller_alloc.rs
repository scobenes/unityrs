mod common;
mod controller_support;
mod publication_support;
#[path = "controller_support/refusals.rs"]
mod refusals;
#[cfg(feature = "test-hooks")]
mod run_support;
mod stage_support;
use std::os::unix::process::ExitStatusExt;
use std::{
    cell::Cell,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    alloc::{Counting, Hook},
    control::{Control, Culprit},
    controller::{Boundary, Controller, Event, Hooks},
    execution::Execution,
    finish,
    job::RunFailure,
    phase::Account,
    progress::{Exit, Snapshot, live::Service},
    reason::Code,
    sink::directory::Directory,
    sys::Lock,
};
use unityrs_testkit::{child, guard};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SITE: AtomicUsize = AtomicUsize::new(0);
static SEEN: AtomicUsize = AtomicUsize::new(0);
thread_local! { static TRACING: Cell<bool> = const { Cell::new(false) }; }
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, bytes: usize) -> bool {
        if TRACING.with(Cell::get) || !ACTIVE.load(Ordering::SeqCst) {
            return false;
        }
        let site = SEEN.fetch_add(1, Ordering::SeqCst) + 1;
        if site != SITE.load(Ordering::SeqCst) {
            return false;
        }
        TRACING.with(|t| t.set(true));
        eprintln!(
            "REFUSED_ALLOCATION site={site} size={bytes}\n{}END_REFUSED_ALLOCATION",
            std::backtrace::Backtrace::force_capture()
        );
        TRACING.with(|t| t.set(false));
        true
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);
#[derive(Default)]
struct Stop(Lock<Option<RunFailure>>);
impl Control for Stop {
    fn stopped(&self) -> bool {
        self.0.lock().is_some()
    }
    fn request_stop(&self, failure: RunFailure, _: Culprit) {
        RunFailure::reduce(&mut self.0.lock(), failure);
    }
}
struct ArmAt(&'static str);
impl unityrs_export::ngui::Hooks for ArmAt {}
impl Hooks for ArmAt {
    fn event(&self, event: Event) {
        if matches!(
            (self.0, event),
            ("attempt", Event::PeekReceived(_)) | ("failure", Event::AttemptReceived(_))
        ) {
            ACTIVE.store(true, Ordering::SeqCst);
        }
    }
}
fn scopes() -> impl Iterator<Item = &'static str> {
    [
        "stage", "peek", "attempt", "failure", "complete", "progress",
    ]
    .into_iter()
    .chain(cfg!(feature = "test-hooks").then_some("progress-delayed"))
    // A whole `unityrs batch` run with archives, armed at publication's
    // admission: winners and archive transfers, then the normal finisher.
    .chain(cfg!(feature = "test-hooks").then_some("archive"))
}
#[test]
#[cfg_attr(debug_assertions, ignore = "scale: run in release")]
fn every_controller_allocation_is_fallible_after_the_running_head() {
    let () = guard::finishes("controller refusal sweep", || {
        let mut passed = true;
        for kind in scopes() {
            let output = refusal_child(kind, 0);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let sites: usize = stdout
                .lines()
                .find_map(|s| s.strip_prefix(&format!("CONTROLLER_SITES kind={kind} sites=")))
                .unwrap()
                .parse()
                .unwrap();
            if kind.starts_with("progress") {
                assert_eq!(
                    sites, 0,
                    "progress writer must never allocate after startup"
                );
                let output = refusal_child(kind, 1);
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                println!(
                    "CONTROLLER_RESULT kind={kind} sites=0 fallible=0 excluded=0 functions=0 failures=0"
                );
                continue;
            }
            let expected = refusals::INVENTORY
                .iter()
                .filter(|e| e.applies(kind))
                .collect::<Vec<_>>();
            let mut reached = std::collections::BTreeSet::new();
            let mut excluded = 0;
            let mut failures = 0;
            for site in 1..=sites {
                let output = refusal_child(kind, site);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if output.status.signal() == Some(libc::SIGABRT) {
                    // Resolve the first owning function, never an ancestor or a
                    // core/export type mentioned in a std generic argument.
                    let function = refusals::innermost(&stderr);
                    let core_or_export = function.as_deref().filter(|function| {
                        function.starts_with("unityrs_core::")
                            || function.starts_with("unityrs_export::")
                    });
                    if let Some(entry) =
                        expected.iter().find(|e| core_or_export == Some(e.function))
                    {
                        reached.insert(entry.function);
                        excluded += 1;
                        println!(
                            "CR10_EXCLUDED kind={kind} site={site} function={} name={}",
                            entry.function, entry.name
                        );
                        continue;
                    }
                    failures += 1;
                    eprintln!(
                        "UNLISTED_ABORT kind={kind} site={site} function={function:?} status={}\n{stderr}",
                        output.status
                    );
                } else if !output.status.success()
                    || !stderr.contains(&format!("REFUSED_ALLOCATION site={site} size="))
                {
                    failures += 1;
                    eprintln!(
                        "UNHANDLED_REFUSAL kind={kind} site={site} status={}\n{stderr}",
                        output.status
                    );
                }
            }
            let fallible = sites - excluded - failures;
            for entry in &expected {
                if !reached.contains(entry.function) {
                    failures += 1;
                    eprintln!("STALE_CR10 kind={kind} function={}", entry.function);
                }
            }
            passed &= failures == 0;
            println!(
                "CONTROLLER_RESULT kind={kind} sites={sites} fallible={fallible} excluded={excluded} functions={} failures={failures}",
                reached.len()
            );
        }
        assert!(
            passed,
            "a post-head controller allocation did not report host_memory"
        );
    });
}
fn refusal_child(kind: &str, site: usize) -> std::process::Output {
    child::output(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "controller_refusal_child", "--nocapture"])
            .env("UNITYRS_CONTROLLER_REFUSALS", kind)
            .env("UNITYRS_CONTROLLER_SITE", site.to_string()),
    )
    .unwrap()
}
#[test]
fn controller_refusal_child() {
    if std::env::var_os("UNITYRS_CONTROLLER_REFUSALS").is_none() {
        return;
    }
    let selected = std::env::var("UNITYRS_CONTROLLER_REFUSALS").unwrap();
    let site: usize = std::env::var("UNITYRS_CONTROLLER_SITE")
        .unwrap()
        .parse()
        .unwrap();
    assert!(site < 4096, "controller refusal sweep exceeded cap");
    if selected.starts_with("progress") {
        progress_child(&selected, site);
        return;
    }
    #[cfg(feature = "test-hooks")]
    if selected == "archive" {
        archive_child(site);
        return;
    }
    for kind in scopes() {
        if selected == kind {
            controller_child(kind, site);
            return;
        }
    }
    panic!("unknown controller scope: {selected}");
}
fn controller_child(kind: &'static str, site: usize) {
    let mut harness =
        controller_support::Harness::new(&[("a", controller_support::clear(1), vec![])], &[], 1, 1);
    match kind {
        "stage" => std::fs::remove_file(harness.source(0)).unwrap(),
        "peek" => std::fs::write(harness.source(0), b"invalid").unwrap(),
        "attempt" | "failure" => harness.configure(|s| s.max_export_work = 0),
        _ => {}
    }
    let stop = Arc::new(Stop::default());
    let mut controller = Controller::new(
        harness.prepared.take().unwrap(),
        harness.spool.clone(),
        stop.clone(),
        Arc::new(ArmAt(kind)),
    )
    .unwrap();
    let mut execution = Execution::new(&controller).unwrap();
    let progress_path = harness.scratch.join("progress");
    let progress = Service::start(
        std::fs::File::create(&progress_path).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    controller.progress(progress.sender());
    let sink = harness.scratch.join("sink");
    std::fs::create_dir(&sink).unwrap();
    let directory = Directory::preflight(
        &sink,
        "r",
        1,
        Arc::new(publication_support::Portable),
        &mut vec![0; unityrs_batch::manifest::head::HEAD_LIMIT + 1],
    )
    .unwrap();
    finish::running(
        controller.state(),
        &directory,
        &Account::new(unityrs_batch::bound::C_FIN),
        &finish::Identity {
            started_at: "2026-01-01T00:00:00Z",
            memory_bound: 0,
            spool_peak: 0,
            vm_hwm: 0,
        },
    )
    .unwrap();
    println!("CONTROLLER_REFUSAL kind={kind} site={site}");
    SEEN.store(0, Ordering::SeqCst);
    SITE.store(site, Ordering::SeqCst);
    ACTIVE.store(!matches!(kind, "attempt" | "failure"), Ordering::SeqCst);
    let result = controller.drive_local(&mut execution);
    execution.quiesce().unwrap();
    let acknowledged = progress.exit(
        progress.sender().snapshot().unwrap(),
        Exit {
            status: Some("running"),
            code: Code::Host,
            reason: Some("host_memory"),
            detail: None,
        },
        std::time::Duration::from_secs(1),
    );
    ACTIVE.store(false, Ordering::SeqCst);
    assert!(acknowledged, "progress terminal line was not acknowledged");
    if site == 0 {
        let sites = SEEN.load(Ordering::SeqCst);
        assert_eq!(result, Boundary::ReadyForWinners);
        assert!(sites > 0);
        println!("CONTROLLER_SITES kind={kind} sites={sites}");
    } else {
        assert!(
            SEEN.load(Ordering::SeqCst) >= site,
            "unreached {kind} {site}"
        );
        assert_eq!(result, Boundary::HandOver, "{kind} {site}");
        let failure = stop.0.lock().unwrap();
        assert_eq!(
            failure.reason,
            unityrs_batch::attempt::ReasonCode::HostMemory,
            "{kind} {site}"
        );
        assert_eq!(failure.code().value(), 74, "{kind} {site}");
    }
    let head: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sink.join("_unityrs/r/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(head["status"], "running");
    assert_eq!(head["write_seq"], 0);
}

/// Refuse the first post-start allocation, even when the writer was held
/// before its clock and mailbox initialization. There must be no such site.
fn progress_child(kind: &str, site: usize) {
    use std::{io::Read, time::Duration};
    let (mut reader, writer) = std::io::pipe().unwrap();
    let file = std::fs::File::from(std::os::fd::OwnedFd::from(writer));
    let initial = Snapshot::validate(0);
    let service = match kind {
        #[cfg(feature = "test-hooks")]
        "progress-delayed" => Service::test_start(file, initial, || {
            std::thread::sleep(Duration::from_millis(50));
        })
        .unwrap(),
        "progress" => Service::start(file, initial).unwrap(),
        _ => panic!("unknown progress scope"),
    };
    SEEN.store(0, Ordering::SeqCst);
    SITE.store(site, Ordering::SeqCst);
    ACTIVE.store(true, Ordering::SeqCst);
    let mut read_line = || {
        let mut buffer = [0; 4096];
        loop {
            let n = reader.read(&mut buffer).unwrap();
            assert!(n > 0, "progress writer closed without a line");
            if buffer[..n].contains(&b'\n') {
                break;
            }
        }
    };
    read_line();
    service.post(initial);
    read_line();
    let acknowledged = service.exit(
        initial,
        Exit {
            status: Some("running"),
            code: Code::Host,
            reason: Some("host_memory"),
            detail: None,
        },
        Duration::from_secs(1),
    );
    ACTIVE.store(false, Ordering::SeqCst);
    assert!(acknowledged);
    let sites = SEEN.load(Ordering::SeqCst);
    assert_eq!(sites, 0, "progress must use only its startup storage");
    let mut terminal = String::new();
    reader.read_to_string(&mut terminal).unwrap();
    let exit: serde_json::Value = serde_json::from_str(&terminal).unwrap();
    assert_eq!(exit["status"], "running");
    assert_eq!(exit["reason"], "host_memory");
    assert_eq!(exit["code"], 74);
    println!("CONTROLLER_SITES kind={kind} sites={sites}");
}

#[cfg(feature = "test-hooks")]
struct ArmAtAdmission;
#[cfg(feature = "test-hooks")]
impl unityrs_batch::publish::Hooks for ArmAtAdmission {
    fn point(&self, point: unityrs_batch::publish::Point) {
        if point == unityrs_batch::publish::Point::Admission {
            ACTIVE.store(true, Ordering::SeqCst);
        }
    }
}
#[cfg(feature = "test-hooks")]
struct Space;
#[cfg(feature = "test-hooks")]
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &std::fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &std::fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
/// Every allocation from publication's admission through the final head of
/// an archive run either succeeds or reports `host_memory`/74.
#[cfg(feature = "test-hooks")]
fn archive_child(site: usize) {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "archives-textures")
        .unwrap();
    let scene = run_support::scene(&case);
    let scratch = unityrs_testkit::scratch::Scratch::new("archive-refusals");
    std::fs::create_dir(scratch.join("spool")).unwrap();
    std::fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
    // One thread and one drive keep allocation ordinals deterministic.
    spec["threads"] = 1.into();
    spec["download_concurrency"] = 1.into();
    let prepared = unityrs_batch::phase1::prepare(
        unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap(),
    )
    .unwrap();
    let service = Service::start(
        std::fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    println!("CONTROLLER_REFUSAL kind=archive site={site}");
    SEEN.store(0, Ordering::SeqCst);
    SITE.store(site, Ordering::SeqCst);
    let result = unityrs_batch::run::drive_hooked(
        prepared,
        0,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        Arc::new(ArmAtAdmission),
    );
    ACTIVE.store(false, Ordering::SeqCst);
    if site == 0 {
        let sites = SEEN.load(Ordering::SeqCst);
        assert_eq!(result.unwrap().code, Code::Complete);
        assert!(sites > 0);
        println!("CONTROLLER_SITES kind=archive sites={sites}");
        return;
    }
    assert!(
        SEEN.load(Ordering::SeqCst) >= site,
        "unreached archive {site}"
    );
    match result {
        Err(e) => assert_eq!((e.code, e.reason), (Code::Host, "host_memory"), "{site}"),
        Ok(end) => assert_eq!(
            (end.code, end.reason),
            (Code::Host, Some("host_memory")),
            "{site}"
        ),
    }
}
