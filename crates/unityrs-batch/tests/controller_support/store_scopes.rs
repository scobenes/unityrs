//! CR10 whole store runs: publication, resume, verification and the finisher.
//! The fake has its own exempt current-thread runtime; engine threads alone
//! participate in the keyed census and one-refusal-per-child sweep.
use super::{ACTIVE, EXEMPT, SEEN, SITE, Space, publication_support, sites, store_support};
use serde_json::{Value, json};
use std::{
    fs,
    sync::{Arc, atomic::Ordering::SeqCst},
};
use store_support::fake::{Fake, Operation};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    publish,
    reason::Code,
    run,
};
use unityrs_testkit::scratch::Scratch;

pub const SCOPES: &[&str] = &["store", "store-resume", "store-multipart"];

static FIXTURE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
pub fn fixture() -> Scratch {
    let scratch = Scratch::new("store-refusal-fixture");
    let mut rng = unityrs_testkit::Rng::new(42);
    let mut pixels = vec![0; 2048 * 2048 * 4];
    for part in pixels.as_chunks_mut::<8>().0 {
        part.copy_from_slice(&rng.next_u64().to_le_bytes());
    }
    let file = unityrs_testkit::ngui::file(
        &[],
        &[unityrs_testkit::ngui::texture(
            1,
            b"pixel",
            4,
            (2048, 2048),
            pixels,
        )],
        true,
    )
    .unwrap();
    fs::write(
        scratch.join("input"),
        unityrs_testkit::env::bundle(&[("CAB-synthetic", file)]),
    )
    .unwrap();
    FIXTURE.set(scratch.join("input")).unwrap();
    scratch
}
pub fn command(command: &mut std::process::Command) {
    if let Some(fixture) = FIXTURE.get() {
        command.env("UNITYRS_STORE_FIXTURE", fixture);
    }
}

struct Environment;
fn check_rows(peer: &Fake, head: &Value) {
    for (family, chunks) in head["rows"].as_object().unwrap() {
        let Some(chunks) = chunks.as_u64() else {
            continue;
        };
        for chunk in 0..chunks {
            let key = format!("p/_unityrs/r/{}/{family}.{chunk}.ndjson", head["attempt"]);
            let object = peer
                .object("bucket", &key)
                .expect("head references a missing row chunk");
            assert!(object.bytes.ends_with(b"\n"));
            let text = std::str::from_utf8(&object.bytes).unwrap();
            for line in text.lines() {
                let row: Value = serde_json::from_str(line).unwrap();
                assert!(row.is_object());
                if family == "inputs" {
                    assert_eq!(row["id"], "a");
                }
            }
        }
    }
}
impl unityrs_batch::stage::env::Environment for Environment {
    fn var(&self, name: &str) -> Option<std::ffi::OsString> {
        match name {
            "AWS_ACCESS_KEY_ID" => Some("AKIDEXAMPLE".into()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret".into()),
            _ => None,
        }
    }
}
struct Arm {
    seed: Option<(Arc<Fake>, std::path::PathBuf)>,
}
impl publish::Hooks for Arm {
    fn point(&self, point: publish::Point) {
        if point == publish::Point::Admission {
            if let Some((fake, root)) = &self.seed {
                EXEMPT.with(|e| e.set(true));
                fake.seed_upload("p/abandoned", "journaled-upload");
                unityrs_batch::spool::journal::Journal::new(root)
                    .unwrap()
                    .append(unityrs_batch::spool::journal::Entry {
                        endpoint: Some(fake.endpoint.as_str()),
                        bucket: "bucket",
                        key: "p/abandoned",
                        upload_id: "journaled-upload",
                    })
                    .unwrap();
                EXEMPT.with(|e| e.set(false));
            }
            ACTIVE.store(true, SeqCst);
            // Make the next real operation exhaust its cooperative IO budget.
            // Tokio creates the deferred-wake queue per block_on, so startup
            // cannot prime it. This schedule holds that rare allocation to
            // the same census and staleness rule as every other store site.
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            for _ in 0..1024 {
                if !tokio::task::coop::has_budget_remaining() {
                    break;
                }
                let mut budget = std::pin::pin!(tokio::task::consume_budget());
                assert!(std::future::Future::poll(budget.as_mut(), &mut cx).is_ready());
            }
            assert!(!tokio::task::coop::has_budget_remaining());
        }
    }
}
struct Observe(
    unityrs_batch::sys::Lock<Vec<Arc<unityrs_core::limits::Meter>>>,
    std::sync::atomic::AtomicBool,
    unityrs_batch::sys::Lock<Vec<unityrs_batch::http::permit::ChargeProbe>>,
);
impl run::Observer for Observe {
    fn taken(&self, stop: Option<unityrs_batch::lifetime::Stop>) {
        self.1.store(stop.is_some(), SeqCst);
    }
    fn handed_over(&self, h: &unityrs_batch::finish::HandOver) {
        // The observer's bookkeeping is test infrastructure.
        EXEMPT.with(|e| e.set(true));
        let mut meters = self.0.lock();
        if let Some(unityrs_batch::finish::Selected::Owned(owned)) = &h.selected {
            meters.push(owned.account.meter().clone());
        }
        if let Some(unityrs_batch::sink::Sink::Store(store)) = &h.sink {
            store.visit_meters(|meter| meters.push(meter.clone()));
            store.visit_permits(|probe| self.2.lock().push(probe));
        }
        EXEMPT.with(|e| e.set(false));
    }
    fn finished(&self, finished: &unityrs_batch::finish::Finished) {
        assert!(finished.settled.closed);
    }
}
fn drive(
    spec: &Value,
    scratch: &Scratch,
    armed: bool,
    peer: &Arc<Fake>,
) -> Result<unityrs_batch::finish::ResultEnd, run::Error> {
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(spec).unwrap()).unwrap())
            .unwrap();
    let progress = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let mut network = run::Network::system();
    network.environment = Arc::new(Environment);
    let hooks: Arc<dyn publish::Hooks + Send> = if armed {
        Arc::new(Arm {
            seed: armed.then(|| (peer.clone(), scratch.join("spool/runs/r"))),
        })
    } else {
        Arc::new(publish::NoHooks)
    };
    let observe = Arc::new(Observe(
        unityrs_batch::sys::Lock::new(Vec::with_capacity(8)),
        std::sync::atomic::AtomicBool::new(false),
        unityrs_batch::sys::Lock::new(Vec::with_capacity(8)),
    ));
    let result = run::drive_with(
        prepared,
        0,
        &progress.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &run::Options {
            network: Some(network),
            publish_hooks: hooks,
            observer: observe.clone(),
            ..Default::default()
        },
    );
    ACTIVE.store(false, SeqCst);
    let deadline = std::time::Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
    while observe.0.lock().iter().any(|m| m.used() != 0)
        || observe.2.lock().iter().any(|p| p.used() != 0)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "store run leaked a held charge"
        );
        std::thread::yield_now();
    }
    if let Ok(end) = &result {
        if armed && SITE.load(SeqCst) > 0 && sites::met() {
            let head: Value = serde_json::from_slice(
                &peer
                    .object("bucket", "p/_unityrs/r/manifest.json")
                    .unwrap()
                    .bytes,
            )
            .unwrap();
            if head["write_seq"] == 1 {
                assert_eq!(head["exit_code"], end.code.value());
                assert_eq!(head["status"], "incomplete");
                assert_eq!(
                    head["stop_reason"],
                    if observe.1.load(SeqCst) {
                        json!("host_memory")
                    } else {
                        Value::Null
                    }
                );
            }
        }
        assert!(progress.exit(
            progress.sender().snapshot().unwrap(),
            unityrs_batch::progress::Exit {
                status: end.status,
                code: end.code,
                reason: end.reason,
                detail: end.detail.map(Into::into),
            },
            unityrs_testkit::guard::DEADLOCK_GUARD
        ));
        let text = fs::read_to_string(scratch.join("progress")).unwrap();
        let exit: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(exit["code"], end.code.value());
        assert_eq!(exit["reason"], end.reason.map_or(Value::Null, |r| json!(r)));
    }
    result
}
pub fn child(kind: &str, site: usize) {
    store_support::run(async {
        EXEMPT.with(|e| e.set(true));
        let fake = Arc::new(Fake::start().await);
        let peer = fake.clone();
        let kind = kind.to_owned();
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let scratch = Scratch::new("store-refusals");
            let spec = make_spec(&kind, &scratch, &peer);
            println!("CONTROLLER_REFUSAL kind={kind} site={site}");
            SEEN.store(0, SeqCst);
            SITE.store(site, SeqCst);
            let puts_before = peer
                .logs()
                .iter()
                .filter(|l| {
                    l.operation == Operation::Put
                        && ["/bucket/p/a/pixel.png", "/bucket/p/all.zip"]
                            .contains(&l.raw_path.as_str())
                })
                .count();
            let result = drive(&spec, &scratch, true, &peer);
            let seen = SEEN.load(SeqCst);
            let end = result.unwrap();
            assert_eq!(peer.open_uploads(), 0);
            let journal =
                unityrs_batch::spool::journal::Journal::new(&scratch.join("spool/runs/r")).unwrap();
            assert!(journal.reader().unwrap().next().unwrap().is_none());
            if site == 0 || !sites::met() {
                check_success(&kind, &spec, &peer, puts_before, &end);
            } else {
                check_refused(site, &peer, &end);
            }
            if site == 0 {
                assert!(seen > 0);
                println!("CONTROLLER_SITES kind={kind} sites={seen}");
                sites::report(&kind);
            } else if !sites::met() {
                println!("CONTROLLER_UNREACHED kind={kind} site={site}");
            }
            send.send(()).unwrap();
        });
        receive.await.unwrap();
    });
}

fn make_spec(kind: &str, scratch: &Scratch, peer: &Arc<Fake>) -> Value {
    fs::create_dir(scratch.join("spool")).unwrap();
    let file = unityrs_testkit::ngui::file(
        &[],
        &[unityrs_testkit::ngui::texture(
            1,
            b"pixel",
            4,
            (1, 1),
            vec![1, 2, 3, 255],
        )],
        true,
    )
    .unwrap();
    fs::write(
        scratch.join("input"),
        unityrs_testkit::env::bundle(&[("CAB-synthetic", file)]),
    )
    .unwrap();
    let mut spec = super::common::base();
    spec["run_id"] = "r".into();
    spec["threads"] = 1.into();
    spec["download_concurrency"] = 1.into();
    spec["deadline"] = "2099-01-01T00:00:00Z".into();
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"] = json!({"kind":"object_store", "bucket":"bucket", "region":"us-east-1", "prefix":"p/", "endpoint":peer.endpoint.as_str(), "path_style":true});
    spec["inputs"] = json!([{"id":"a", "path":scratch.join("input")}]);
    spec["dependencies"] = json!([]);
    spec["archives"] =
        json!([{"key":"all.zip", "members":[{"kind":"texture", "path":"{name}.png"}]}]);
    if kind == "store-resume" {
        assert_eq!(
            drive(&spec, scratch, false, peer).unwrap().code,
            Code::Complete
        );
        // A successful library run detaches its retiring workers. They can
        // still own the spool lease after their last charge is released.
        // Wait for that exact teardown boundary before the next run starts.
        let deadline = std::time::Instant::now() + unityrs_testkit::guard::DEADLOCK_GUARD;
        loop {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(scratch.join("spool/lock"))
                .unwrap();
            match unityrs_batch::sys::lock_exclusive(file) {
                Ok(lease) => {
                    drop(lease);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "seed run retained its spool lock"
                    );
                    std::thread::yield_now();
                }
                Err(error) => panic!("seed spool lock: {error}"),
            }
        }
        spec["attempt"] = 2.into();
        spec["resume"] = true.into();
    }
    if kind == "store-multipart" {
        spec["spec_version"] = 2.into();
        spec["outputs"]["image"] = "png_fast".into();
        fs::copy(
            std::env::var_os("UNITYRS_STORE_FIXTURE").unwrap(),
            scratch.join("input"),
        )
        .unwrap();
        spec["max_inflight_uncompressed_bytes"] = (512 * 1024 * 1024).into();
        spec["max_decoded_bytes"] = (128 * 1024 * 1024).into();
        spec["dep_cache_bytes"] = (128 * 1024 * 1024).into();
    }
    spec
}

fn check_success(
    kind: &str,
    spec: &Value,
    peer: &Fake,
    puts_before: usize,
    end: &unityrs_batch::finish::ResultEnd,
) {
    assert_eq!(end.code, Code::Complete);
    let head: Value = serde_json::from_slice(
        &peer
            .object("bucket", "p/_unityrs/r/manifest.json")
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert_eq!(head["write_seq"], 1);
    assert_eq!(head["verified"], true);
    if kind == "store-resume" {
        assert_eq!(head["counts"]["outputs_present"], 1);
        assert_eq!(head["counts"]["archives_present"], 1);
    }
    check_rows(peer, &head);
    assert!(peer.logs().iter().any(|l| l.operation == Operation::List));
    assert!(peer.logs().iter().all(|l| l.signature_valid));
    assert!(
        peer.object(
            "bucket",
            &format!("p/_unityrs/r/{}/inputs.0.ndjson", spec["attempt"])
        )
        .is_some()
    );
    for key in ["p/a/pixel.png", "p/all.zip"] {
        let object = peer.object("bucket", key).unwrap();
        assert_eq!(
            object.bytes.len() > 16 * 1024 * 1024,
            kind == "store-multipart"
        );
        if kind == "store-multipart" {
            for operation in [Operation::Create, Operation::Part, Operation::Complete] {
                assert!(peer.logs().iter().any(|l| l.operation == operation
                    && l.raw_path == format!("/bucket/{key}")));
            }
        }
    }
    let puts_after = peer
        .logs()
        .iter()
        .filter(|l| {
            l.operation == Operation::Put
                && ["/bucket/p/a/pixel.png", "/bucket/p/all.zip"].contains(&l.raw_path.as_str())
        })
        .count();
    assert_eq!(
        puts_after - puts_before,
        if kind == "store" { 2 } else { 0 }
    );
}

fn check_refused(site: usize, peer: &Fake, end: &unityrs_batch::finish::ResultEnd) {
    assert_eq!(
        (end.code, end.reason),
        (Code::Host, Some("host_memory")),
        "{site}"
    );
    let head: Value = serde_json::from_slice(
        &peer
            .object("bucket", "p/_unityrs/r/manifest.json")
            .unwrap()
            .bytes,
    )
    .unwrap();
    if head["write_seq"] == 1 {
        assert_eq!(head["exit_code"], 74);
        check_rows(peer, &head);
    } else {
        assert_eq!(head["status"], "running");
        assert_eq!(end.status, Some("running"));
    }
}
