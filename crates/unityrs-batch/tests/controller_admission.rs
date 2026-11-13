mod controller_support;
mod stage_support;
use controller_support::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use unityrs_batch::{
    attempt,
    controller::{Boundary, Controller, Event, NoHooks, Point, SetupError},
    peek,
};
use unityrs_testkit::guard;

#[test]
fn low_live_usage_still_reserves_exactly_the_slot_floor_for_peeks_and_attempts() {
    let () = guard::finishes("bounded admission under holds", || {
        for threads in [1, 2, 8] {
            for room in [1, 2, 3] {
                for peeks in [false, true] {
                    let data: Vec<_> = (0..10).map(|i| (format!("i{i}"), clear(i))).collect();
                    let bundles: Vec<_> = data
                        .iter()
                        .map(|(id, bytes)| (id.as_str(), bytes.clone(), vec![]))
                        .collect();
                    let mut h = Harness::new(&bundles, &[], threads, room);
                    let expected = threads.min(room) as usize;
                    let stop = h.control.clone();
                    let (reached, receive) = mpsc::channel();
                    let (release, held) = mpsc::channel();
                    let held = Mutex::new(held);
                    let count = AtomicUsize::new(0);
                    let hooks = Arc::new(Tap {
                        event: Box::new(move |e| {
                            if matches!(
                                (peeks, e),
                                (true, Event::PeekDispatched(_))
                                    | (false, Event::AttemptDispatched(_))
                            ) {
                                assert!(
                                    count.fetch_add(1, Ordering::AcqRel) < expected,
                                    "admitted by usage instead of reservation"
                                );
                            }
                        }),
                        point: Box::new(move |p| {
                            if matches!(
                                (peeks, p),
                                (true, Point::Peek(_, peek::Point::Open))
                                    | (false, Point::Attempt(_, attempt::Point::Opened))
                            ) {
                                reached.send(()).unwrap();
                                guard::hold(&held.lock().unwrap(), "admission release");
                            }
                        }),
                    });
                    let mut c = h.controller(hooks);
                    let wake = h.execution.as_ref().unwrap().wake();
                    let (done, finished) = mpsc::channel();
                    let driver = std::thread::spawn(move || {
                        let b = c.drive_local(h.execution.as_mut().unwrap());
                        done.send((h, c, b)).unwrap();
                    });
                    // Hooks serialize their hold receiver, but send reached
                    // before taking that mutex, so every active worker is seen.
                    for _ in 0..expected {
                        guard::wait(&receive, "admitted job reached");
                    }
                    stop.store(true, Ordering::Release);
                    wake.notify().unwrap();
                    let (mut h, c, b) = guard::recv(&finished, "admission handover");
                    assert_eq!(b, Boundary::HandOver);
                    assert_eq!(c.reserved_slots(), expected);
                    for _ in 0..expected {
                        release.send(()).unwrap();
                    }
                    h.execution.as_mut().unwrap().quiesce().unwrap();
                    driver.join().unwrap();
                }
            }
        }
    });
}

#[test]
fn one_byte_short_and_url_capability_fail_before_any_write() {
    let () = guard::finishes("setup refusals", || {
        let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
        h.configure(|s| s.max_inflight_uncompressed_bytes -= 1);
        assert!(matches!(
            Controller::new(
                h.prepared.take().unwrap(),
                h.spool.clone(),
                h.control.clone(),
                Arc::new(NoHooks)
            ),
            Err(SetupError::Admission)
        ));
        assert_eq!(std::fs::read_dir(h.scratch.join("in")).unwrap().count(), 0);
        let validated = stage_support::from_bundles(
            &[serde_json::json!({"id":"a","path":"/source"})],
            &[serde_json::json!({"id":"unused","url":"https://example.invalid/bundle"})],
        );
        let prepared = unityrs_batch::phase1::prepare(validated).unwrap();
        assert!(matches!(
            Controller::new(
                prepared,
                h.spool.clone(),
                h.control.clone(),
                Arc::new(NoHooks)
            ),
            Err(SetupError::UrlUnsupported)
        ));
        assert_eq!(std::fs::read_dir(h.scratch.join("in")).unwrap().count(), 0);
    });
}
