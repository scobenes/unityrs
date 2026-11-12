mod attempt_support;
use attempt_support::*;
use std::cell::Cell;
use unityrs_batch::{
    attempt::{Hooks, Point, ReasonCode, State},
    reason::{Limit, LimitKind},
};
use unityrs_export::ngui;
use unityrs_testkit::ngui::{SceneBundle, scenes};
#[derive(Default)]
struct Observe {
    latest: Cell<u64>,
    registered: Cell<u64>,
}
impl ngui::Hooks for Observe {
    fn environment(&self, env: &unityrs_core::env::Environment) {
        self.latest.set(env.stats().dependencies_evicted);
    }
}
impl Hooks for Observe {
    fn point(&self, p: Point) {
        if p == Point::Registered {
            self.registered.set(self.latest.get());
        }
    }
}
#[test]
fn registration_evictions_are_the_inputs_bare_limit() {
    let mut scene = simple(&[]);
    scene.bundles[0].deps = vec!["a".into(), "b".into()];
    for id in ["a", "b"] {
        scene.bundles.push(SceneBundle {
            id: id.into(),
            data: unityrs_testkit::env::bundle(&[("resource", vec![0; 10000])]),
            deps: vec![],
        });
    }
    let mut found = false;
    for d in (8192..65536).step_by(2048) {
        let h = Harness::new(&scene, "x");
        let mut cap = limits();
        cap.d = d;
        cap.max_attempt_evictions = 0;
        let end = h.run(cap, &ngui::NoHooks);
        clear(&end);
        if matches!(
            end.reason,
            Some(ReasonCode::Limit(Limit {
                kind: LimitKind::AttemptEvictions,
                value: 0
            }))
        ) {
            assert_eq!(end.state, State::Failed);
            assert_eq!(end.dependency, None);
            assert_eq!(end.figures.evictions, 1);
            found = true;
            break;
        }
    }
    assert!(found);
}
#[test]
fn crossed_reads_enforce_the_same_eviction_ceiling_after_registration() {
    let mut found = false;
    'scenes: for (_, scene) in scenes().unwrap() {
        for input in &scene.inputs {
            if !scene.bundles.iter().any(|b| !b.deps.is_empty()) {
                continue;
            }
            for d in (4096..65536).step_by(2048) {
                let h = Harness::new(&scene, input);
                let observer = Observe::default();
                let mut cap = limits();
                cap.d = d;
                let end = h.run(cap, &observer);
                clear(&end);
                if end.state != State::Done || end.figures.evictions <= observer.registered.get() {
                    continue;
                }
                let h = Harness::new(&scene, input);
                cap.max_attempt_evictions = observer.registered.get();
                let end = h.run(cap, &ngui::NoHooks);
                clear(&end);
                assert_eq!(
                    end.reason,
                    Some(ReasonCode::Limit(Limit {
                        kind: LimitKind::AttemptEvictions,
                        value: cap.max_attempt_evictions
                    }))
                );
                assert_eq!(end.dependency, None);
                assert_eq!(end.figures.evictions, cap.max_attempt_evictions + 1);
                found = true;
                break 'scenes;
            }
        }
    }
    assert!(found, "a crossed read must evict in the tested range");
}
#[cfg(feature = "test-hooks")]
#[test]
fn a_run_failure_during_an_environment_call_precedes_its_eviction_limit() {
    // The real load supplies the eviction; the existing test-hooks seam
    // substitutes host_io for its result so both precedence checks are reached.
    struct HostIo;
    impl ngui::Hooks for HostIo {
        fn environment_fault(
            &self,
            env: &unityrs_core::env::Environment,
        ) -> Option<unityrs_core::Error> {
            (env.stats().dependencies_evicted > 0)
                .then(|| unityrs_core::Error::from(std::io::Error::from_raw_os_error(libc::EIO)))
        }
    }
    impl Hooks for HostIo {}
    let mut scene = simple(&[]);
    scene.bundles[0].deps = vec!["a".into(), "b".into()];
    for id in ["a", "b"] {
        scene.bundles.push(SceneBundle {
            id: id.into(),
            data: unityrs_testkit::env::bundle(&[("resource", vec![0; 10000])]),
            deps: vec![],
        });
    }
    let mut found = false;
    for d in (8192..65536).step_by(2048) {
        let h = Harness::new(&scene, "x");
        let mut cap = limits();
        cap.d = d;
        cap.max_attempt_evictions = 0;
        let end = h.run(cap, &HostIo);
        clear(&end);
        if end.figures.evictions > 0 {
            assert_eq!(end.state, State::RunFailure);
            assert_eq!(end.reason, Some(ReasonCode::HostIo));
            found = true;
            break;
        }
    }
    assert!(found);
}
