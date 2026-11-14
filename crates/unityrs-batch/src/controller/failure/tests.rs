use super::*;
use std::sync::atomic::AtomicBool;
use unityrs_core::budget::Reservation;

#[test]
fn an_absorbed_allocator_refusal_rejects_a_written_failure_record() {
    for refused in [false, true] {
        let scratch = unityrs_testkit::scratch::Scratch::new("failure-audit");
        std::fs::create_dir(scratch.join("fail")).unwrap();
        let raw = serde_json::json!({
            "spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
            "max_inflight_uncompressed_bytes":268_435_456,"max_decoded_bytes":16_777_216,
            "dep_cache_bytes":33_554_432,
            "spool":{"dir":"/spool","size_bytes":1_073_741_824,"reserve_bytes":0},
            "inputs":[{"id":"a","path":"/in/a"}],"dependencies":[],
            "outputs":{"textures":{"key":"{input}/{name}.png"}},
            "sink":{"kind":"directory","root":"/out"}
        });
        let spec = Arc::new(
            crate::spec::parse(serde_json::to_vec(&raw).unwrap())
                .unwrap()
                .spec,
        );
        let memory = Arc::new(crate::job_memory::Plan::new(&spec).unwrap());
        let budget = &memory.failures[0].budget;
        if refused {
            let meter =
                unityrs_core::limits::Meter::new(u64::MAX, budget.clone(), Reservation::Input);
            let charge = meter.charge(32).unwrap();
            let _ = meter.allocator_refused(4096);
            drop(charge);
        }
        let before = crate::alloc::refusals();
        let args = Write {
            input: 0,
            bundle: 0,
            selected: Selected::Peek(FailureCode::NotABundle),
            owner: Shared::new(OnceLock::new()).unwrap(),
            spec,
            spool: PreparedSpool::new(
                scratch.path().to_path_buf(),
                crate::spool::Counter::new(1 << 30, 0, 4096).unwrap(),
                Arc::new(crate::spool::Host),
            )
            .unwrap(),
            control: Arc::new(AtomicBool::new(false)),
            hooks: Arc::new(super::super::NoHooks),
            memory: memory.clone(),
        };
        let result = args.run();
        assert_eq!(
            crate::alloc::refusals(),
            before,
            "independent of the global NULL counter"
        );
        let accounts = budget.snapshot();
        assert!(
            accounts.is_clear(),
            "writer charges dropped before the audit"
        );
        assert_eq!(accounts.refusals(), 0, "no budget refusal");
        assert!(accounts.input.peak > 0, "the record writer ran");
        if refused {
            assert!(matches!(
                result,
                Err(RunFailure {
                    reason: ReasonCode::HostMemory,
                    ..
                })
            ));
        } else {
            assert!(matches!(result, Ok(Some(_))));
        }
    }
}
