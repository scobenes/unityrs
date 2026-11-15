mod common;
mod controller_support;
#[cfg(feature = "test-hooks")]
mod http_support;
mod publication_support;
#[path = "controller_support/refusals.rs"]
mod refusals;
#[cfg(feature = "test-hooks")]
mod run_support;
// Without the hooks there is no url scope, so nothing aims a child.
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
#[path = "controller_support/sites.rs"]
mod sites;
mod stage_support;
#[cfg(feature = "test-hooks")]
#[path = "controller_support/url_scopes.rs"]
mod url_scopes;
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
thread_local! {
    static TRACING: Cell<bool> = const { Cell::new(false) };
    /// A test peer's thread (the `url` scope's origin): never refused.
    static EXEMPT: Cell<bool> = const { Cell::new(false) };
}
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, bytes: usize) -> bool {
        if TRACING.with(Cell::get) || EXEMPT.with(Cell::get) || !ACTIVE.load(Ordering::SeqCst) {
            return false;
        }
        let ordinal = SEEN.fetch_add(1, Ordering::SeqCst) + 1;
        let refused = if sites::keyed() {
            TRACING.with(|t| t.set(true));
            let refused = sites::refuses();
            TRACING.with(|t| t.set(false));
            refused
        } else {
            ordinal == SITE.load(Ordering::SeqCst)
        };
        if !refused {
            return false;
        }
        let site = SITE.load(Ordering::SeqCst);
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
#[cfg(feature = "test-hooks")]
const URL_SCOPES: &[&str] = url_scopes::SCOPES;
fn timed(kind: &str) -> bool {
    #[cfg(feature = "test-hooks")]
    return url_scopes::timed(kind);
    #[cfg(not(feature = "test-hooks"))]
    {
        let _ = kind;
        false
    }
}
#[cfg(not(feature = "test-hooks"))]
const URL_SCOPES: &[&str] = &[];
#[test]
#[cfg_attr(debug_assertions, ignore = "scale: run in release")]
fn every_controller_allocation_is_fallible_after_the_running_head() {
    let _serial = sweeping();
    sweep(scopes());
}
/// A networked library run's staging: two URLs and a local copy, armed at
/// the first dispatch through staging's end (or the run's teardown), in
/// each of its scenes (`controller_support/url_scopes.rs`). Its own test.
#[test]
#[cfg_attr(debug_assertions, ignore = "scale: run in release")]
fn every_url_staging_allocation_is_fallible_after_the_first_dispatch() {
    let _serial = sweeping();
    sweep(URL_SCOPES.iter().copied());
}
/// One sweep at a time: a url scope's reads follow its machine's load.
/// The deadlock guard is each child's ([`refusal_child`]), not a sweep's: a
/// sweep runs a child for every site, so its length grows with the sites
/// and with the machine's load, while one child answers at once unless it
/// hangs (a whole sweep under one guard failed a run on a slow machine with
/// no hang, every scope it reached clean).
fn sweeping() -> std::sync::MutexGuard<'static, ()> {
    static SWEEP: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SWEEP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
/// Each scope's sites, one child a site, against the inventory.
fn sweep(kinds: impl Iterator<Item = &'static str>) {
    let mut passed = true;
    // Every scope's reached entries, and the entries a timed scope leaves
    // to them: each must be reached in some scope it names.
    let mut reached_anywhere = std::collections::BTreeSet::new();
    let mut deferred = std::collections::BTreeMap::new();
    for kind in kinds {
        let output = refusal_child(kind, 0, None);
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
            let output = refusal_child(kind, 1, None);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            println!(
                "CONTROLLER_RESULT kind={kind} sites=0 fallible=0 excluded=0 functions=0 unreached=0 failures=0"
            );
            continue;
        }
        let expected = refusals::INVENTORY
            .iter()
            .filter(|e| e.applies(kind))
            .collect::<Vec<_>>();
        let targets = targets(kind, sites, &stdout);
        let mut tally = Tally::default();
        // The keys whose allocation some child refused.
        let mut covered = std::collections::BTreeSet::new();
        for ((site, aim), output) in children(kind, &targets) {
            if tally.add(kind, site, &output, &expected) {
                covered.extend(aim.map(|(key, _)| key));
            }
        }
        let mut sites = targets.len();
        if timed(kind) {
            sites += settle(kind, sites, covered, &expected, &mut tally);
        }
        let Tally {
            reached,
            excluded,
            mut failures,
            unreached,
        } = tally;
        let fallible = sites.saturating_sub(excluded + failures + unreached);
        for entry in &expected {
            // A timed scope's reach follows its peer's timing and the
            // machine's load: a fixed scope holds each entry to staleness,
            // and a timed one its own entries (the failing run's
            // teardown), leaving the rest to the sweep's other scopes.
            if reached.contains(entry.name) {
                continue;
            }
            if !timed(kind) || entry.scopes == [kind] {
                failures += 1;
                eprintln!(
                    "STALE_CR10 kind={kind} functions={:?} name={}",
                    entry.functions, entry.name
                );
            } else {
                deferred.insert(entry.name, *entry);
            }
        }
        reached_anywhere.extend(reached.iter().copied());
        passed &= failures == 0;
        println!(
            "CONTROLLER_RESULT kind={kind} sites={sites} fallible={fallible} excluded={excluded} functions={} unreached={unreached} failures={failures}",
            reached.len()
        );
    }
    for entry in deferred.into_values() {
        if !reached_anywhere.contains(entry.name) {
            passed = false;
            eprintln!(
                "STALE_CR10 kind=any functions={:?} name={}",
                entry.functions, entry.name
            );
        }
    }
    assert!(
        passed,
        "a post-head controller allocation did not report host_memory"
    );
}
/// One scope's children, classified: the entries their aborts reached,
/// and how many were excluded, failed or unreached.
#[derive(Default)]
struct Tally {
    reached: std::collections::BTreeSet<&'static str>,
    excluded: usize,
    failures: usize,
    unreached: usize,
}
impl Tally {
    /// Classifies `site`'s child: whether it met its site.
    fn add(
        &mut self,
        kind: &str,
        site: usize,
        output: &std::process::Output,
        expected: &[&'static refusals::Exclusion],
    ) -> bool {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if unreached_site(kind, site, output) {
            self.unreached += 1;
            return false;
        }
        if output.status.signal() == Some(libc::SIGABRT) {
            // Resolve the first owning function, never an ancestor or a
            // core/export type mentioned in a std generic argument.
            let function = refusals::innermost(&stderr);
            if let Some(entry) = expected
                .iter()
                .find(|e| e.admits(function.as_deref(), &stderr))
            {
                self.reached.insert(entry.name);
                self.excluded += 1;
                println!(
                    "CR10_EXCLUDED kind={kind} site={site} function={} name={}",
                    function.as_deref().unwrap_or_default(),
                    entry.name
                );
                return true;
            }
            self.failures += 1;
            eprintln!(
                "UNLISTED_ABORT kind={kind} site={site} function={function:?} allocating={:?} callee={:?} status={}\n{stderr}",
                refusals::allocating(&stderr),
                refusals::callee(&stderr),
                output.status
            );
        } else if !output.status.success()
            || !stderr.contains(&format!("REFUSED_ALLOCATION site={site} size="))
        {
            self.failures += 1;
            eprintln!(
                "UNHANDLED_REFUSAL kind={kind} site={site} status={}\n{stderr}",
                output.status
            );
        }
        true
    }
}
/// A child's site, numbered from 1, and for a url scope the key and the
/// occurrence it refuses (`controller_support/sites.rs`).
type Target = (usize, Option<(u64, usize)>);
/// A scope's children: each ordinal its census child counted, or, for a
/// url scope, each occurrence of each key its census child listed.
fn targets(kind: &str, sites: usize, stdout: &str) -> Vec<Target> {
    if !timed(kind) {
        return (1..=sites).map(|site| (site, None)).collect();
    }
    sites::census(kind, stdout)
        .into_iter()
        .flat_map(|(key, count)| (1..=count).map(move |nth| (key, nth)))
        .zip(1..)
        .map(|(aim, site)| (site, Some(aim)))
        .collect()
}
/// A url scope's child that met no allocation it was aimed at.
fn unreached_site(kind: &str, site: usize, output: &std::process::Output) -> bool {
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .contains(&format!("CONTROLLER_UNREACHED kind={kind} site={site}\n"))
}
/// Each target in its own child, a few at once: every child has its own
/// scratch (and a url scope's, its own origin), so they share nothing but
/// the machine.
fn children(kind: &str, targets: &[Target]) -> Vec<(Target, std::process::Output)> {
    let at_once = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let mut outputs = Vec::with_capacity(targets.len());
    for batch in targets.chunks(at_once) {
        std::thread::scope(|scope| {
            let running: Vec<_> = batch
                .iter()
                .map(|&(site, aim)| {
                    scope.spawn(move || {
                        (
                            (site, aim),
                            refusal_child(kind, site, aim.map(sites::aimed)),
                        )
                    })
                })
                .collect();
            outputs.extend(running.into_iter().map(|child| child.join().unwrap()));
        });
    }
    outputs
}
/// More new keys than one scope's timing can vary.
const SETTLE: usize = 256;
/// A url scope settles (`controller_support/sites.rs`): one child at a
/// time refuses the first allocation whose key is not `covered`, which
/// that key then joins, until a child meets none. Its refusing children,
/// numbered after `last`.
fn settle(
    kind: &str,
    last: usize,
    mut covered: std::collections::BTreeSet<u64>,
    expected: &[&'static refusals::Exclusion],
    tally: &mut Tally,
) -> usize {
    for refused in 0..SETTLE {
        let site = last + refused + 1;
        let output = refusal_child(kind, site, Some(sites::beyond(&covered)));
        if unreached_site(kind, site, &output) {
            println!(
                "CONTROLLER_SETTLED kind={kind} new={refused} covered={}",
                covered.len()
            );
            return refused;
        }
        tally.add(kind, site, &output, expected);
        // A child that failed before its refusal: counted, and the
        // scope cannot settle.
        let Some(key) = sites::refused(&String::from_utf8_lossy(&output.stderr)) else {
            return refused + 1;
        };
        covered.insert(key);
    }
    tally.failures += 1;
    eprintln!("UNSETTLED kind={kind}: {SETTLE} children each met a key no child had refused");
    SETTLE
}
/// One site's child, under its own deadlock guard (see [`sweeping`]).
fn refusal_child(kind: &str, site: usize, aim: Option<(&str, String)>) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "controller_refusal_child", "--nocapture"])
        .env("UNITYRS_CONTROLLER_REFUSALS", kind)
        .env("UNITYRS_CONTROLLER_SITE", site.to_string());
    if let Some((name, value)) = aim {
        command.env(name, value);
    }
    guard::finishes(
        &format!("the refusal child of kind {kind}, site {site}"),
        move || child::output(&mut command).unwrap(),
    )
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
    #[cfg(feature = "test-hooks")]
    if selected.starts_with("url") {
        let aim = std::env::var("UNITYRS_CONTROLLER_AIM").ok();
        let covered = std::env::var("UNITYRS_CONTROLLER_COVERED").ok();
        assert_eq!(
            aim.is_some() || covered.is_some(),
            site > 0,
            "a url child's aim"
        );
        sites::aim(aim.as_deref(), covered.as_deref());
        url_scopes::child(&selected, site);
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
    // Test-only quiescence: dropping the owned runtime is tokio's teardown,
    // never on a hand-over path, so it is outside the armed span.
    let armed = ACTIVE.swap(false, Ordering::SeqCst);
    execution.quiesce().unwrap();
    ACTIVE.store(armed, Ordering::SeqCst);
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
