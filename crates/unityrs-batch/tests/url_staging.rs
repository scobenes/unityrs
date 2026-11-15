//! URL staging inside whole library runs: retries, redirects, integrity,
//! the stop freeze, the cutoff, header variables and secrets, over a
//! loopback origin and the engine's own HTTP service.
mod common;
mod publication_support;
mod run_support;
mod url_run_support;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use unityrs_batch::{
    controller::{self, Event, Point},
    finish::{Finished, HandOver},
    lifetime::Stop,
    phase1,
    progress::{Snapshot, live::Service},
    reason::StopReason,
    run::{self, Lifetime, Network, Observer, Options},
    stage::env::Environment,
};
use unityrs_testkit::scratch::Scratch;
use url_run_support::{Close, Origin, Reply};

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
/// One input and one dependency, both moved onto the origin (`/b0`, `/b1`).
struct Scene {
    scratch: Scratch,
    spec: Value,
    origin: Arc<Origin>,
    /// The input's and the dependency's bytes.
    bytes: [Vec<u8>; 2],
}
impl Scene {
    fn new() -> Self {
        let case = run_support::cases()
            .into_iter()
            .find(|c| c["name"] == "ngui-material_in_dependency")
            .unwrap();
        let scene = run_support::scene(&case);
        let scratch = Scratch::new("url-staging");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
        let bytes = [&spec["inputs"][0], &spec["dependencies"][0]]
            .map(|b| fs::read(b["path"].as_str().unwrap()).unwrap());
        let origin = Arc::new(Origin::start());
        url_run_support::onto(&origin, &mut spec, |_| true);
        spec["http"] = json!({"backoff_initial_ms": 0, "backoff_max_ms": 0});
        Self {
            scratch,
            spec,
            origin,
            bytes,
        }
    }
    /// The row of `family` for the input (0) or the dependency (1); rows
    /// run in graph order.
    fn row(&self, family: &str, n: usize) -> Value {
        let id = if n == 0 {
            &self.spec["inputs"][0]["id"]
        } else {
            &self.spec["dependencies"][0]["id"]
        };
        self.rows(family)
            .into_iter()
            .find(|r| r["id"] == *id)
            .unwrap()
    }
    fn bundle(&mut self, n: usize) -> &mut Value {
        if n == 0 {
            &mut self.spec["inputs"][0]
        } else {
            &mut self.spec["dependencies"][0]
        }
    }
    fn drive(&self, options: &Options) -> Result<unityrs_batch::finish::ResultEnd, run::Error> {
        let prepared = phase1::prepare(
            unityrs_batch::spec::parse(serde_json::to_vec(&self.spec).unwrap()).unwrap(),
        )
        .unwrap();
        let service = Service::start(
            fs::File::create(self.scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        run::drive_with(
            prepared,
            0,
            &service.sender(),
            (&Space, Arc::new(publication_support::Portable)),
            options,
        )
    }
    fn run(&self) -> unityrs_batch::finish::ResultEnd {
        self.drive(&Options {
            network: Some(Network::system()),
            ..Options::default()
        })
        .unwrap()
    }
    fn rows(&self, family: &str) -> Vec<Value> {
        let path = self
            .scratch
            .join(format!("sink/_unityrs/golden/1/{family}.0.ndjson"));
        fs::read(path)
            .map(|b| {
                b.split(|c| *c == b'\n')
                    .filter(|l| !l.is_empty())
                    .map(|l| serde_json::from_slice(l).unwrap())
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Every byte the run left in the sink and the progress file.
    fn published_text(&self) -> String {
        let mut text =
            String::from_utf8_lossy(&fs::read(self.scratch.join("progress")).unwrap()).into_owned();
        let mut stack = vec![self.scratch.join("sink")];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    text.push_str(&String::from_utf8_lossy(&fs::read(path).unwrap()));
                }
            }
        }
        text
    }
}
#[test]
fn a_retried_status_counts_its_tries_and_a_redirect_is_followed() {
    let scene = Scene::new();
    scene.origin.script(
        "/b0",
        [
            Reply::Status(503, "Retry-After: 0\r\n".into()),
            Reply::Status(500, String::new()),
        ],
    );
    scene
        .origin
        .script("/b1", [Reply::Redirect("/moved?received=elsewhere".into())]);
    scene.origin.body("/moved", scene.bytes[1].clone());
    assert_eq!(scene.run().code.value(), 0);
    // Each launch boxed one owning future, within its storage bound.
    let size = unityrs_batch::stage::slot::run_size();
    eprintln!("owning future: {size} bytes");
    assert!(size > 0 && size <= unityrs_batch::stage::slot::RUN_STORAGE);
    assert_eq!(scene.origin.requests("/b0"), 3);
    assert_eq!(scene.origin.requests("/b1"), 1);
    assert_eq!(scene.origin.requests("/moved"), 1);
    let bundles = scene.rows("bundles");
    assert!(
        bundles.iter().all(|r| r["status"] == "staged"),
        "{bundles:?}"
    );
    // Every connection ended in RST, a retried try's and a redirect hop's too.
    assert!(
        scene
            .origin
            .wait_closed()
            .iter()
            .all(|c| *c == Close::Reset)
    );
}
#[test]
fn without_a_network_a_url_is_refused_before_any_request() {
    let scene = Scene::new();
    let error = scene.drive(&Options::default()).unwrap_err();
    assert_eq!(
        (error.reason, error.detail.as_deref()),
        ("bad_spec", Some("usage"))
    );
    assert_eq!(scene.origin.total(), 0);
    assert!(
        unityrs_batch::start::supported(
            &unityrs_batch::spec::parse(serde_json::to_vec(&scene.spec).unwrap())
                .unwrap()
                .spec
        )
        .is_ok()
    );
}
#[test]
fn retry_on_none_fails_the_first_try_and_the_view() {
    let mut scene = Scene::new();
    scene.spec["http"]["retry_on"] = json!([]);
    scene
        .origin
        .script("/b1", [Reply::Status(503, String::new())]);
    assert_eq!(scene.run().code.value(), 4);
    assert_eq!(scene.origin.requests("/b1"), 1);
    let dependency = scene.row("bundles", 1);
    assert_eq!(dependency["reason"], "download_failed");
    assert_eq!(dependency["detail"], "status 503");
    let input = scene.row("inputs", 0);
    assert_eq!(input["status"], "failed");
    assert_eq!(
        input["reason"],
        "dependency_failed(d.bundle, download_failed)"
    );
    assert_eq!(input["detail"], "status 503");
}
#[test]
fn integrity_rows_carry_facts_only_for_a_received_body() {
    for (field, value, facts) in [
        ("sha256", json!("0".repeat(64)), true),
        ("size", json!(3), false),
    ] {
        let mut scene = Scene::new();
        scene.bundle(1)[field] = value;
        assert_eq!(scene.run().code.value(), 4);
        let row = scene.row("bundles", 1);
        assert_eq!(row["reason"], "integrity_mismatch");
        assert_eq!(row["detail"], field);
        // A Content-Length that differs from `size` fails at the head, with
        // nothing received; a digest mismatch records what arrived.
        assert_eq!(row["size"].is_u64(), facts, "{row}");
        assert_eq!(row["sha256"].is_string(), facts, "{row}");
    }
}
#[test]
fn a_resumed_run_stages_every_url_again() {
    let mut scene = Scene::new();
    assert_eq!(scene.run().code.value(), 0);
    scene.spec["resume"] = json!(true);
    scene.spec["attempt"] = json!(2);
    assert_eq!(scene.run().code.value(), 0);
    assert_eq!(scene.origin.requests("/b0"), 2);
    assert_eq!(scene.origin.requests("/b1"), 2);
}

/// Counts what happens after the hand-over: slot polls and receipts.
#[derive(Default)]
struct Probe {
    handed: AtomicBool,
    polls_after: AtomicUsize,
    received_after: AtomicUsize,
    origin: Mutex<Option<Arc<Origin>>>,
    closes_at_finish: Mutex<Option<Vec<Option<Close>>>>,
    claimed_at_handover: AtomicUsize,
}
impl unityrs_export::ngui::Hooks for Probe {}
impl controller::Hooks for Probe {
    fn event(&self, event: Event) {
        match event {
            Event::HandOver => self.handed.store(true, Ordering::SeqCst),
            Event::StageReceived(_) if self.handed.load(Ordering::SeqCst) => {
                self.received_after.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    }
    fn point(&self, point: Point) {
        if matches!(point, Point::StagePoll(_)) && self.handed.load(Ordering::SeqCst) {
            self.polls_after.fetch_add(1, Ordering::SeqCst);
        }
    }
}
impl Observer for Probe {
    fn handed_over(&self, handover: &HandOver) {
        let used = handover
            .spool
            .as_ref()
            .and_then(|s| s.lease.as_ref())
            .map_or(0, |l| l.counter.used());
        self.claimed_at_handover
            .store(usize::try_from(used).unwrap(), Ordering::SeqCst);
    }
    fn finished(&self, _: &Finished) {
        // The parked owners are alive here: no frozen connection has closed.
        let origin = self.origin.lock().unwrap().clone().unwrap();
        *self.closes_at_finish.lock().unwrap() = Some(origin.closes());
    }
}
/// A spool whose free space runs out on demand, under a reserve.
struct Filling(AtomicBool);
impl unityrs_batch::spool::FileSystem for Filling {
    fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
        Ok(if self.0.load(Ordering::SeqCst) {
            0
        } else {
            u64::MAX
        })
    }
}
/// A stage that requests the run's stop (here the spool's reserve, after
/// body bytes arrived) keeps its response open: its stage and permit are
/// parked with the owners, so the peer sees the reset only after the
/// finisher, as for a frozen request.
#[test]
fn a_run_failure_keeps_its_live_connection_until_after_the_finisher() {
    let mut scene = Scene::new();
    scene.spec["download_concurrency"] = json!(1);
    scene.spec["spool"]["reserve_bytes"] = json!(1);
    let flag = Arc::new(AtomicBool::new(false));
    scene
        .origin
        .script("/b0", [Reply::HoldAfter(64, flag.clone())]);
    let full = Arc::new(Filling(AtomicBool::new(false)));
    let probe = Arc::new(Probe::default());
    *probe.origin.lock().unwrap() = Some(scene.origin.clone());
    let filler = {
        let (origin, full) = (scene.origin.clone(), full.clone());
        std::thread::spawn(move || {
            origin.wait_requests(1);
            full.0.store(true, Ordering::SeqCst);
            flag.store(true, Ordering::SeqCst);
        })
    };
    let end = scene
        .drive(&Options {
            controller_hooks: probe.clone(),
            observer: probe.clone(),
            spool: Some(full),
            network: Some(Network::system()),
            ..Options::default()
        })
        .unwrap();
    filler.join().unwrap();
    assert_eq!(end.reason, Some("spool_full"), "{end:?}");
    assert_eq!(probe.polls_after.load(Ordering::SeqCst), 0);
    let at_finish = probe.closes_at_finish.lock().unwrap().clone().unwrap();
    assert_eq!(at_finish, [None], "closed before the finisher");
    assert_eq!(scene.origin.wait_closed(), [Close::Reset]);
}
/// Stop the run once the origin has seen `requests` requests (and `after`
/// has passed), and check the freeze: nothing polled or received after the
/// hand-over, no connection closed until the owners drop, then RST.
fn frozen(scene: &Scene, requests: usize, after: Duration) -> Arc<Probe> {
    let probe = Arc::new(Probe::default());
    *probe.origin.lock().unwrap() = Some(scene.origin.clone());
    let lifetime = Arc::new(Lifetime::default());
    let options = Options {
        lifetime: lifetime.clone(),
        controller_hooks: probe.clone(),
        observer: probe.clone(),
        network: Some(Network::system()),
        ..Options::default()
    };
    let origin = scene.origin.clone();
    let stopper = std::thread::spawn(move || {
        origin.wait_requests(requests);
        std::thread::sleep(after);
        lifetime.request(Stop::external(StopReason::Sigterm));
    });
    let started = Instant::now();
    let end = scene.drive(&options).unwrap();
    stopper.join().unwrap();
    // A stop never awaits a frozen request: the run returns promptly.
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(end.code.value(), 6);
    assert!(probe.handed.load(Ordering::SeqCst));
    assert_eq!(probe.polls_after.load(Ordering::SeqCst), 0);
    assert_eq!(probe.received_after.load(Ordering::SeqCst), 0);
    let at_finish = probe.closes_at_finish.lock().unwrap().clone().unwrap();
    assert!(
        at_finish.last().is_some_and(Option::is_none),
        "{at_finish:?}"
    );
    // Dropped after the finisher: the frozen connection's reset.
    assert_eq!(scene.origin.wait_closed().last(), Some(&Close::Reset));
    probe
}
#[test]
fn a_stop_mid_body_freezes_the_request_and_retains_its_claim() {
    let mut scene = Scene::new();
    scene.spec["download_concurrency"] = json!(1);
    let size = scene.bytes[0].len() as u64;
    scene.origin.script("/b0", [Reply::Hold(64)]);
    // Supplied, so the whole size is claimed at the try's start.
    scene.bundle(0)["size"] = json!(size);
    let probe = frozen(&scene, 1, Duration::from_millis(200));
    assert!(probe.claimed_at_handover.load(Ordering::SeqCst) as u64 >= size);
    let bundles = scene.rows("bundles");
    assert!(
        bundles.iter().all(|r| r["status"] == "not_staged"),
        "{bundles:?}"
    );
    assert_eq!(scene.origin.total(), 1);
}
#[test]
fn a_stop_before_the_head_freezes_the_request() {
    let scene = Scene::new();
    scene.origin.script("/b1", [Reply::Silent]);
    frozen(&scene, 2, Duration::from_millis(200));
}
#[test]
fn a_stop_mid_backoff_starts_no_retry() {
    let mut scene = Scene::new();
    scene.spec["download_concurrency"] = json!(1);
    scene.spec["http"] = json!({"backoff_initial_ms": 60_000, "backoff_max_ms": 60_000});
    scene
        .origin
        .script("/b0", [Reply::Status(503, String::new())]);
    let probe = Arc::new(Probe::default());
    let lifetime = Arc::new(Lifetime::default());
    let origin = scene.origin.clone();
    let stopper = {
        let lifetime = lifetime.clone();
        std::thread::spawn(move || {
            origin.wait_requests(1);
            std::thread::sleep(Duration::from_millis(200));
            lifetime.request(Stop::external(StopReason::Sigint));
        })
    };
    let started = Instant::now();
    let end = scene
        .drive(&Options {
            lifetime,
            controller_hooks: probe.clone(),
            network: Some(Network::system()),
            ..Options::default()
        })
        .unwrap();
    stopper.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(end.code.value(), 6);
    assert_eq!(scene.origin.total(), 1);
    assert_eq!(probe.polls_after.load(Ordering::SeqCst), 0);
}
/// The cutoff is on the boot clock: Linux only (elsewhere no policy exists).
#[cfg(target_os = "linux")]
#[test]
fn no_retry_starts_past_the_cutoff_and_the_deadline_stop_ends_the_run() {
    let mut scene = Scene::new();
    scene.spec["download_concurrency"] = json!(1);
    scene.spec["http"] = json!({"backoff_initial_ms": 10_000, "backoff_max_ms": 10_000});
    scene
        .origin
        .script("/b0", [Reply::Status(503, String::new())]);
    let probe = Arc::new(Probe::default());
    let lifetime = Arc::new(Lifetime::default());
    let now = unityrs_batch::lifetime::Boot::now().unwrap();
    // The retry's wait would end past the cutoff: no retry starts, and the
    // stage's end is held unreceived until the deadline's stop.
    let cutoff = unityrs_batch::lifetime::Boot(now.0 + 5_000);
    let origin = scene.origin.clone();
    let stopper = {
        let lifetime = lifetime.clone();
        std::thread::spawn(move || {
            origin.wait_requests(1);
            std::thread::sleep(Duration::from_millis(300));
            lifetime.request(Stop::external(StopReason::Deadline));
        })
    };
    let end = scene
        .drive(&Options {
            lifetime,
            controller_hooks: probe.clone(),
            network: Some(Network {
                cutoff: Some(cutoff),
                ..Network::system()
            }),
            ..Options::default()
        })
        .unwrap();
    stopper.join().unwrap();
    assert_eq!(end.code.value(), 6);
    assert_eq!(end.reason, Some("deadline"));
    assert_eq!(scene.origin.total(), 1);
    assert_eq!(probe.received_after.load(Ordering::SeqCst), 0);
    assert_eq!(scene.row("bundles", 0)["status"], "not_staged");
}

struct Variables(Vec<(&'static str, OsString)>);
impl Environment for Variables {
    fn var(&self, name: &str) -> Option<OsString> {
        self.0
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.clone())
    }
}
#[test]
fn header_variables_are_read_once_and_refused_with_their_names() {
    use std::os::unix::ffi::OsStringExt;
    for (value, reason) in [
        (None, "header_env_missing"),
        (Some(OsString::new()), "header_env_missing"),
        (
            Some(OsString::from("x".repeat(16 * 1024 + 1))),
            "header_env_too_long",
        ),
        (Some(OsString::from("a\nb")), "header_env_invalid"),
        (
            Some(OsString::from_vec(vec![b'a', 0xff])),
            "header_env_invalid",
        ),
    ] {
        let mut scene = Scene::new();
        scene.bundle(0)["headers"] = json!({"X-Token": {"env": "TOKEN_VAR"}});
        let variables: Vec<_> = value.into_iter().map(|v| ("TOKEN_VAR", v)).collect();
        let error = scene
            .drive(&Options {
                network: Some(Network {
                    environment: Arc::new(Variables(variables)),
                    ..Network::system()
                }),
                ..Options::default()
            })
            .unwrap_err();
        assert_eq!(error.code.value(), 69);
        assert_eq!(error.reason, reason);
        assert_eq!(error.detail.as_deref(), Some("TOKEN_VAR"));
        assert_eq!(scene.origin.total(), 0);
    }
}
#[test]
fn secrets_reach_the_origin_and_nothing_the_run_writes() {
    let mut scene = Scene::new();
    let query = "signature=query-secret-1";
    let input = scene.bundle(0)["url"].as_str().unwrap().to_owned();
    scene.bundle(0)["url"] = json!(format!("{input}?{query}"));
    scene.bundle(0)["headers"] = json!({
        "Authorization": "Bearer literal-secret-2",
        "X-Token": {"env": "TOKEN_VAR"},
    });
    scene.bundle(1)["headers"] = json!({"X-Token": {"env": "TOKEN_VAR"}});
    scene
        .origin
        .script("/b1", [Reply::Redirect("/moved?redirect-secret-4".into())]);
    scene.origin.body("/moved", scene.bytes[1].clone());
    let end = scene
        .drive(&Options {
            network: Some(Network {
                environment: Arc::new(Variables(vec![(
                    "TOKEN_VAR",
                    OsString::from("env-secret-3"),
                )])),
                ..Network::system()
            }),
            ..Options::default()
        })
        .unwrap();
    assert_eq!(end.code.value(), 0);
    let heads = scene.origin.heads().join("\n");
    for sent in [
        query,
        "literal-secret-2",
        "env-secret-3",
        "redirect-secret-4",
    ] {
        assert!(heads.contains(sent), "{sent} not sent");
    }
    // Read once, shared: both bundles sent the one value.
    assert_eq!(heads.matches("env-secret-3").count(), 3);
    let written = scene.published_text();
    for secret in [
        "query-secret-1",
        "literal-secret-2",
        "env-secret-3",
        "redirect-secret-4",
        "127.0.0.1",
    ] {
        assert!(!written.contains(secret), "{secret} written");
    }
}
#[test]
fn the_maskers_list_holds_each_query_and_every_header_value() {
    use unityrs_batch::{diagnostic::Secrets, stage::env::Values};
    let mut scene = Scene::new();
    let input = scene.bundle(0)["url"].as_str().unwrap().to_owned();
    scene.bundle(0)["url"] = json!(format!("{input}?signature=query-secret-1#frag"));
    scene.bundle(0)["headers"] = json!({
        "Authorization": "Bearer literal-secret-2",
        "X-Token": {"env": "TOKEN_VAR"},
    });
    scene.bundle(1)["headers"] = json!({"X-Token": {"env": "TOKEN_VAR"}});
    let validated = unityrs_batch::spec::parse(serde_json::to_vec(&scene.spec).unwrap()).unwrap();
    let variables = Variables(vec![("TOKEN_VAR", OsString::from("env-secret-3"))]);
    let values = Values::resolve(&validated.spec, &variables).unwrap();
    assert_eq!(values.len(), 1);
    let mut list = Vec::new();
    values.secrets(&validated.spec, &mut list).unwrap();
    assert_eq!(
        list,
        [
            "signature=query-secret-1",
            "Bearer literal-secret-2",
            "env-secret-3",
            "env-secret-3"
        ]
    );
    let credentials = std::sync::OnceLock::new();
    let secrets = Secrets {
        values: &list,
        credentials: &credentials,
    };
    let mut out = [0; 256];
    let n = secrets.mask(
        "a ?signature=query-secret-1 b env-secret-3 Bearer literal-secret-2",
        &mut out,
    );
    assert_eq!(&out[..n], b"a ?*** b *** ***");
}
