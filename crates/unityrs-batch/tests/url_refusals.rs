//! A refused allocation in a networked run's staging, aimed at one site:
//! a body's DATA buffer (`http::request::PIECE` bytes), refused while its
//! peer holds the rest of the body. The run reports `host_memory`/74 and
//! keeps the connection open until after its finisher (the refused stage's
//! residual), and a stop requested inside the refused poll does not hide
//! the refusal.
mod common;
mod publication_support;
mod run_support;
mod url_run_support;
use serde_json::json;
use std::{
    fs,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
};
use unityrs_batch::{
    alloc::{Counting, Hook},
    controller::{self, Event},
    finish::Finished,
    lifetime::Stop,
    phase1,
    progress::{Snapshot, live::Service},
    reason::{Code, StopReason},
    run::{self, Lifetime, Network, Observer, Options},
};
use unityrs_testkit::scratch::Scratch;
use url_run_support::{Close, Origin, Reply};

/// Armed from the first dispatch: the first DATA buffer is refused once.
static ARMED: AtomicBool = AtomicBool::new(false);
static REFUSED: AtomicBool = AtomicBool::new(false);
/// Set for the stop test: the hook requests a SIGTERM before refusing, so
/// the stop and the refusal fall in one poll of the stage.
static STOP: OnceLock<Arc<Lifetime>> = OnceLock::new();
static SERIAL: Mutex<()> = Mutex::new(());
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, bytes: usize) -> bool {
        if bytes != unityrs_batch::http::request::PIECE
            || !ARMED.load(SeqCst)
            || REFUSED.swap(true, SeqCst)
        {
            return false;
        }
        // Atomics and a wake's write: nothing here allocates.
        if let Some(lifetime) = STOP.get() {
            lifetime.request(Stop::external(StopReason::Sigterm));
        }
        true
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
struct Probe(Arc<Origin>, Mutex<Option<Vec<Option<Close>>>>);
impl unityrs_export::ngui::Hooks for Probe {}
impl controller::Hooks for Probe {
    fn event(&self, event: Event) {
        if matches!(event, Event::StageDispatched(_)) {
            ARMED.store(true, SeqCst);
        }
    }
}
impl Observer for Probe {
    fn finished(&self, _: &Finished) {
        // The parked owners are alive here.
        *self.1.lock().unwrap() = Some(self.0.closes());
    }
}
/// The input alone moved onto the origin, which sends its head and 64
/// body bytes and then holds the rest.
fn refused_run(lifetime: Arc<Lifetime>) -> (unityrs_batch::finish::ResultEnd, Arc<Probe>) {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ARMED.store(false, SeqCst);
    REFUSED.store(false, SeqCst);
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-material_in_dependency")
        .unwrap();
    let scene = run_support::scene(&case);
    let scratch = Scratch::new("url-refusals");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
    let origin = Arc::new(Origin::start());
    url_run_support::onto(&origin, &mut spec, |n| n == 0);
    origin.script("/b0", [Reply::Hold(64)]);
    spec["download_concurrency"] = json!(1);
    spec["http"] = json!({"attempts": 1});
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let probe = Arc::new(Probe(origin.clone(), Mutex::new(None)));
    let end = run::drive_with(
        prepared,
        0,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &Options {
            lifetime,
            controller_hooks: probe.clone(),
            observer: probe.clone(),
            network: Some(Network::system()),
            ..Options::default()
        },
    )
    .unwrap();
    ARMED.store(false, SeqCst);
    assert!(REFUSED.load(SeqCst), "the DATA buffer was refused");
    (end, probe)
}
#[test]
fn a_refused_body_buffer_keeps_its_connection_until_after_the_finisher() {
    let (end, probe) = refused_run(Arc::new(Lifetime::default()));
    assert_eq!((end.code, end.reason), (Code::Host, Some("host_memory")));
    let at_finish = probe.1.lock().unwrap().clone().unwrap();
    assert_eq!(at_finish, [None], "closed before the finisher");
    assert_eq!(probe.0.wait_closed(), [Close::Reset]);
}
#[test]
fn a_stop_in_the_refused_poll_still_reports_host_memory() {
    let lifetime = Arc::new(Lifetime::default());
    STOP.set(lifetime.clone()).ok().unwrap();
    let (end, probe) = refused_run(lifetime);
    // The SIGTERM came first; the refusal outranks it.
    assert_eq!((end.code, end.reason), (Code::Host, Some("host_memory")));
    let at_finish = probe.1.lock().unwrap().clone().unwrap();
    assert_eq!(at_finish, [None], "closed before the finisher");
    assert_eq!(probe.0.wait_closed(), [Close::Reset]);
}
