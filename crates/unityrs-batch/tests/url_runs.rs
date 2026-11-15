//! Whole library runs with URL bundles over a loopback origin, through the
//! engine's own HTTP service, against the same runs from local paths.
mod common;
mod publication_support;
mod run_support;
mod url_run_support;
use std::{fs, sync::Arc};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    run::{self, Network, Options},
};
use unityrs_testkit::scratch::Scratch;
use url_run_support::{Close, Origin};
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
/// Which bundles move onto the origin.
#[derive(Clone, Copy, Debug)]
enum Moved {
    None,
    All,
    Alternate,
}
fn run_case(
    case: &serde_json::Value,
    moved: Moved,
    (threads, concurrency): (u64, u64),
) -> (run_support::Checked, usize) {
    let scene = run_support::scene(case);
    let scratch = Scratch::new("url-run");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, mut expected) = run_support::instantiate(case, &scene, &scratch);
    spec["threads"] = threads.into();
    spec["download_concurrency"] = concurrency.into();
    let origin = Origin::start();
    let ids = match moved {
        Moved::None => vec![],
        Moved::All => url_run_support::onto(&origin, &mut spec, |_| true),
        Moved::Alternate => url_run_support::onto(&origin, &mut spec, |n| n % 2 == 0),
    };
    let urls = ids.len();
    // A source the origin lacks is its 404: the same views fail, for the
    // URL's own reason and detail.
    missing_is_404(&mut expected, &ids);
    let mut prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    run::unitycn_key(&mut prepared, run_support::env(case)).unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let options = Options {
        network: (urls > 0).then(Network::system),
        ..Options::default()
    };
    let result = run::drive_with(
        prepared,
        123,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &options,
    )
    .unwrap();
    assert_eq!(
        u64::from(result.code.value()),
        case["exit_code"].as_u64().unwrap(),
        "{} {moved:?}",
        case["name"]
    );
    let snapshot = service.sender().snapshot().unwrap();
    assert_eq!(snapshot.transfers.stages_active, 0);
    assert_eq!(
        snapshot.transfers.stages_done + snapshot.transfers.stages_failed,
        u64::try_from(
            spec["inputs"].as_array().unwrap().len()
                + spec["dependencies"].as_array().map_or(0, Vec::len)
        )
        .unwrap()
    );
    // One try a URL, each on a connection of its own, each ended by RST:
    // no FIN precedes the reset of a zero-linger close.
    assert_eq!(origin.total(), urls, "{}", case["name"]);
    assert!(origin.wait_closed().iter().all(|c| *c == Close::Reset));
    let checked = run_support::check(&scratch, &spec, case, expected, 123);
    let progress = fs::read(scratch.join("progress")).unwrap();
    assert!(!String::from_utf8_lossy(&progress).contains("127.0.0.1"));
    (checked, urls)
}
#[test]
fn url_runs_equal_local_runs_across_threads_and_concurrency() {
    for case in run_support::cases() {
        if case["missing_sources"]
            .as_array()
            .is_some_and(|m| !m.is_empty())
        {
            continue;
        }
        let (local, _) = run_case(&case, Moved::None, (1, 1));
        for (moved, shape) in [
            (Moved::All, (1, 1)),
            (Moved::Alternate, (2, 2)),
            (Moved::All, (8, 256)),
        ] {
            let (checked, urls) = run_case(&case, moved, shape);
            assert!(urls > 0);
            assert_eq!(checked, local, "{} {moved:?} {shape:?}", case["name"]);
        }
    }
}
fn missing_is_404(value: &mut serde_json::Value, moved: &[String]) {
    match value {
        serde_json::Value::Object(fields) => {
            if fields.get("reason").and_then(|r| r.as_str()) == Some("source_missing")
                && fields
                    .get("id")
                    .and_then(|i| i.as_str())
                    .is_some_and(|id| moved.iter().any(|m| m == id))
            {
                fields.insert("reason".into(), "download_failed".into());
                fields.insert("detail".into(), "status 404".into());
            }
            for v in fields.values_mut() {
                missing_is_404(v, moved);
            }
        }
        serde_json::Value::Array(values) => {
            for v in values {
                missing_is_404(v, moved);
            }
        }
        _ => {}
    }
}
/// Every failure view the goldens pin (a failed own bundle against failed
/// dependencies, a failed unused dependency's exit 4, ...) fails alike when
/// its missing source is a URL the origin answers 404, once, unretried.
#[test]
fn missing_sources_as_404_urls_fail_the_same_views() {
    for case in run_support::cases() {
        if case["missing_sources"].as_array().is_none_or(Vec::is_empty) {
            continue;
        }
        for (moved, shape) in [(Moved::All, (1, 1)), (Moved::Alternate, (2, 4))] {
            let (checked, urls) = run_case(&case, moved, shape);
            assert!(urls > 0);
            if matches!(moved, Moved::All) {
                assert!(!checked.0.to_string().contains("source_missing"));
            }
        }
    }
}
