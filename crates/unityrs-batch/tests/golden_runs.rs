mod common;
mod publication_support;
mod run_support;
use std::fs;
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    run,
};
use unityrs_testkit::scratch::Scratch;
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
#[test]
fn pinned_synthetic_goldens_across_threads_admission_and_resume() {
    for case in run_support::cases() {
        let scene = run_support::scene(&case);
        let mut baseline: Option<run_support::Checked> = None;
        for (attempt, (threads, concurrency, inflight, resume, corrupt)) in [
            (1, 1, 64 << 20, false, false),
            (2, 2, 256 << 20, false, false),
            (8, 8, 256 << 20, true, false),
            (2, 2, 256 << 20, true, true),
        ]
        .into_iter()
        .enumerate()
        {
            let scratch = Scratch::new("golden-run");
            fs::create_dir(scratch.join("spool")).unwrap();
            fs::create_dir(scratch.join("sink")).unwrap();
            let (mut spec, expected) = run_support::instantiate(&case, &scene, &scratch);
            spec["threads"] = threads.into();
            spec["download_concurrency"] = concurrency.into();
            spec["max_inflight_uncompressed_bytes"] = inflight.into();
            spec["resume"] = resume.into();
            spec["attempt"] = (attempt + 1).into();
            let mut rewritten = None;
            if resume && let Some((_, outputs)) = &baseline {
                for (key, bytes) in outputs {
                    let path = scratch.join(format!("sink/{key}"));
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let mut existing = bytes.clone();
                    if corrupt && rewritten.is_none() {
                        // BTreeMap order chooses exactly one deterministic key.
                        // Keep the size identical so resume must inspect the bytes.
                        existing[0] ^= 0xff;
                        rewritten = Some(key.clone());
                    }
                    fs::write(path, existing).unwrap();
                }
            }
            let prepared = phase1::prepare(
                unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap(),
            )
            .unwrap();
            let service = Service::start(
                fs::File::create(scratch.join("progress")).unwrap(),
                Snapshot::validate(0),
            )
            .unwrap();
            let result = run::drive(
                prepared,
                123,
                &service.sender(),
                &Space,
                std::sync::Arc::new(publication_support::Portable),
            )
            .unwrap();
            assert_eq!(
                u64::from(result.code.value()),
                case["exit_code"].as_u64().unwrap(),
                "{}",
                case["name"]
            );
            assert!(
                service.sender().snapshot().unwrap().outputs.candidates
                    >= u64::try_from(case["outputs"].as_object().unwrap().len()).unwrap(),
                "terminal candidate accounting"
            );
            let final_snapshot = service.sender().snapshot().unwrap();
            assert_eq!(final_snapshot.transfers.stages_active, 0);
            assert_eq!(final_snapshot.inputs.running, 0);
            assert_eq!(final_snapshot.inputs.queued, 0);
            run_support::assert_raw_publication(
                &scratch,
                &spec,
                case["outputs"].as_object().unwrap().len(),
                rewritten.as_deref(),
            );
            let actual = run_support::check(&scratch, &spec, &case, expected, 123);
            if let Some(baseline) = &baseline {
                assert_eq!(&actual, baseline);
            } else {
                baseline = Some(actual);
            }
        }
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn twenty_seeded_input_orders_preserve_unit_decisions_and_row_encodings() {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-losing-units")
        .unwrap();
    let scene = run_support::scene(&case);
    let mut baseline = None;
    for seed in 0..20 {
        let scratch = Scratch::new("seeded-normal");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        let (mut spec, expected) = run_support::instantiate(&case, &scene, &scratch);
        let inputs = spec["inputs"].as_array_mut().unwrap();
        let mut rng = unityrs_testkit::Rng::new(seed);
        for index in (1..inputs.len()).rev() {
            inputs.swap(index, rng.below(index + 1));
        }
        spec["threads"] = if seed % 2 == 0 { 1 } else { 8 }.into();
        spec["download_concurrency"] = if seed % 3 == 0 { 1 } else { 4 }.into();
        let prepared = phase1::prepare(
            unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap(),
        )
        .unwrap();
        let service = Service::start(
            fs::File::create(scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        let result = run::drive(
            prepared,
            0,
            &service.sender(),
            &Space,
            std::sync::Arc::new(publication_support::Portable),
        )
        .unwrap();
        assert_eq!(result.code.value(), 3);
        let checked = run_support::check(&scratch, &spec, &case, expected, 0);
        let selected = ["outputs", "collisions"].map(|name| {
            fs::read(scratch.join(format!("sink/_unityrs/golden/1/{name}.0.ndjson"))).unwrap()
        });
        let current = (checked, selected);
        if let Some(baseline) = &baseline {
            assert_eq!(baseline, &current);
        } else {
            baseline = Some(current);
        }
    }
}
