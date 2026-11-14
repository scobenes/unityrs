use std::io::Read;
use unityrs_batch::{
    progress::{self, Snapshot, live},
    reason::Code,
};
use unityrs_testkit::guard::DEADLOCK_GUARD;

#[test]
fn writer_emits_gapless_lines_and_exit_has_priority() {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let service = live::Service::start(
        std::fs::File::from(std::os::fd::OwnedFd::from(writer)),
        Snapshot::validate(0),
    )
    .unwrap();
    for ms in 1..100 {
        service.post(Snapshot::validate(ms));
    }
    assert!(service.exit(
        Snapshot::validate(100),
        progress::Exit {
            status: Some("complete"),
            code: Code::Complete,
            reason: None,
            detail: None
        },
        DEADLOCK_GUARD.min(std::time::Duration::from_secs(1))
    ));
    service.post(Snapshot::validate(200));
    drop(service);
    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();
    let lines: Vec<serde_json::Value> = output
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_ne!(lines.len(), 0);
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line["seq"], index);
    }
    assert_eq!(lines.last().unwrap()["type"], "exit");
    assert_eq!(lines.last().unwrap()["ms"], 100);
    assert_eq!(lines.iter().filter(|l| l["type"] == "exit").count(), 1);
}

#[test]
fn a_blocked_writer_never_blocks_producers_or_extends_the_ack_budget() {
    use std::{
        io::{ErrorKind, Write},
        os::unix::net::UnixStream,
    };
    let (mut reader, mut writer) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    let mut filled = 0;
    loop {
        match writer.write(&[b'x'; 1024]) {
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => panic!("{e}"),
        }
    }
    writer.set_nonblocking(false).unwrap();
    let service = live::Service::start(
        std::fs::File::from(std::os::fd::OwnedFd::from(writer)),
        Snapshot::validate(0),
    )
    .unwrap();
    let sender = service.sender();
    let posted = unityrs_testkit::guard::finishes("nonblocking progress producers", move || {
        for ms in 1..10_000 {
            sender.post(Snapshot::validate(ms));
        }
        9_999
    });
    assert_eq!(posted, 9_999);
    assert!(!service.exit(
        Snapshot::validate(10_000),
        progress::Exit {
            status: Some("complete"),
            code: Code::Complete,
            reason: None,
            detail: None
        },
        std::time::Duration::ZERO
    ));
    drop(service);
    let bytes = unityrs_testkit::guard::finishes("progress drain", move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let text = std::str::from_utf8(bytes.get(filled..).unwrap()).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line["seq"], index);
    }
    assert_eq!(lines.last().unwrap()["type"], "exit");
}
#[test]
fn live_budget_counters_include_both_accounts_and_release_every_charge() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use unityrs_core::budget::{MemoryBudget, Reservation};
    let used = Arc::new(AtomicU64::new(0));
    let budget = unityrs_batch::budget::SealedBudget::observed(100, 200, false, Some(used.clone()));
    let input = budget.try_reserve(50, Reservation::Input).unwrap();
    let dependency = budget.try_reserve(150, Reservation::Dependency).unwrap();
    assert_eq!(used.load(Ordering::Acquire), 200);
    assert!(budget.try_reserve(101, Reservation::Input).is_err());
    assert_eq!(used.load(Ordering::Acquire), 200);
    drop(input);
    assert_eq!(used.load(Ordering::Acquire), 150);
    drop(dependency);
    assert_eq!(used.load(Ordering::Acquire), 0);
}

mod common;
mod controller_support;
mod stage_support;

fn check_live_usage(reader: std::io::PipeReader, observed: &std::sync::mpsc::SyncSender<()>) {
    use std::io::BufRead;
    let mut positive = false;
    for line in std::io::BufReader::new(reader).lines() {
        let line: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let used = line["memory"]["attempts_used"].as_u64().unwrap();
        assert!(
            used <= line["memory"]["slots_reserved"].as_u64().unwrap(),
            "live budget usage exceeds its published reservation: {line}",
        );
        if used != 0 && !positive {
            positive = true;
            observed.send(()).unwrap();
        }
        if line["type"] == "exit" {
            assert!(positive, "no live budget usage was sampled");
            return;
        }
    }
    panic!("progress ended without an exit line");
}

#[test]
fn dispatch_publishes_the_reservation_before_workers_can_charge_it() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    };
    use unityrs_batch::{
        controller::{Boundary, Controller, Event, Point},
        execution::Execution,
    };
    let () = unityrs_testkit::guard::finishes("dispatch reservation progress", || {
        let mut harness = controller_support::Harness::new(
            &[("a", controller_support::clear(4_096), vec![])],
            &[],
            1,
            1,
        );
        let (reader, writer) = std::io::pipe().unwrap();
        let service = live::Service::start(
            std::fs::File::from(std::os::fd::OwnedFd::from(writer)),
            Snapshot::validate(0),
        )
        .unwrap();
        let (observed, observation) = mpsc::sync_channel(1);
        let observation = Mutex::new(observation);
        let reader = std::thread::spawn(move || check_live_usage(reader, &observed));
        let sender = service.sender();
        let sampler = sender.clone();
        let dispatched = Arc::new(AtomicU64::new(0));
        let seen = dispatched.clone();
        let hooks = controller_support::Tap {
            event: Box::new(move |event| {
                let reserved = match event {
                    Event::PeekDispatched(_) | Event::AttemptDispatched(_) => 64 << 20,
                    _ => return,
                };
                assert_eq!(sender.snapshot().unwrap().memory.slots_reserved, reserved);
                seen.fetch_add(1, Ordering::AcqRel);
            }),
            point: Box::new(move |point| {
                if matches!(
                    point,
                    Point::Attempt(_, unityrs_batch::attempt::Point::Opened)
                ) {
                    assert_ne!(sampler.attempts().load(Ordering::Acquire), 0);
                    sampler.update(|_| {});
                    unityrs_testkit::guard::wait(
                        &observation.lock().unwrap(),
                        "writer to sample a live budget charge",
                    );
                }
            }),
        };
        let mut controller = Controller::new(
            harness.prepared.take().unwrap(),
            harness.spool.clone(),
            harness.control.clone(),
            Arc::new(hooks),
        )
        .unwrap();
        controller.progress(service.sender());
        let mut execution = Execution::new(&controller).unwrap();
        assert_eq!(
            controller.drive_local(&mut execution),
            Boundary::ReadyForWinners
        );
        execution.quiesce().unwrap();
        assert_eq!(dispatched.load(Ordering::Acquire), 2);
        let snapshot = service.sender().snapshot().unwrap();
        assert_eq!(snapshot.memory.slots_reserved, 0);
        assert!(service.exit(
            snapshot,
            progress::Exit {
                status: Some("complete"),
                code: Code::Complete,
                reason: None,
                detail: None,
            },
            DEADLOCK_GUARD.min(std::time::Duration::from_secs(1)),
        ));
        reader.join().unwrap();
    });
}

struct WriterUsage {
    sender: live::Sender,
    seen: std::sync::atomic::AtomicBool,
    stopped: std::sync::atomic::AtomicBool,
}
impl unityrs_batch::control::Control for WriterUsage {
    fn stopped(&self) -> bool {
        use std::sync::atomic::Ordering;
        let used = self.sender.attempts().load(Ordering::Acquire);
        if used != 0 {
            assert!(self.sender.snapshot().unwrap().memory.slots_reserved >= used);
            self.seen.store(true, Ordering::Release);
        }
        self.stopped.load(Ordering::Acquire)
    }
    fn request_stop(&self, _: unityrs_batch::job::RunFailure, _: unityrs_batch::control::Culprit) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
#[test]
fn failure_record_writer_reports_its_borrowed_slot_and_live_budget() {
    use std::sync::{Arc, atomic::Ordering};
    use unityrs_batch::{
        controller::{Boundary, Controller},
        execution::Execution,
    };
    let () = unityrs_testkit::guard::finishes("failure writer progress", || {
        let mut harness = controller_support::Harness::new(
            &[("a", controller_support::clear(1), vec![])],
            &[],
            1,
            1,
        );
        std::fs::remove_file(harness.source(0)).unwrap();
        let service = live::Service::start(
            std::fs::File::create(harness.scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        let control = Arc::new(WriterUsage {
            sender: service.sender(),
            seen: std::sync::atomic::AtomicBool::new(false),
            stopped: std::sync::atomic::AtomicBool::new(false),
        });
        let mut controller = Controller::new(
            harness.prepared.take().unwrap(),
            harness.spool.clone(),
            control.clone(),
            Arc::new(controller_support::Observe::default()),
        )
        .unwrap();
        controller.progress(service.sender());
        let mut execution = Execution::new(&controller).unwrap();
        assert_eq!(
            controller.drive_local(&mut execution),
            Boundary::ReadyForWinners
        );
        execution.quiesce().unwrap();
        assert!(
            control.seen.load(Ordering::Acquire),
            "failure writer reported zero live usage"
        );
        assert_eq!(control.sender.attempts().load(Ordering::Acquire), 0);
        assert_eq!(control.sender.snapshot().unwrap().memory.slots_reserved, 0);
    });
}
