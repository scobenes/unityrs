//! Exact plain-HTTP read schedules. All plans and storage exist before the
//! running head. The hook gathers short OS reads, exposes each scheduled
//! boundary, then holds until the next stage poll. No clock, sleep or peer write
//! boundary decides which parser/body/stop path a child takes.
use std::{
    io,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
};
use tokio::io::ReadBuf;
use unityrs_batch::{
    http::{PlainReadHooks, arm_plain_reads},
    lifetime::Stop,
    reason::StopReason,
    run::Lifetime,
    sys::reset::RegisteredSocket,
};

pub const SCOPES: &[&str] = &[
    "url-head-split",
    "url-body-split",
    "url-stop-before",
    "url-stop-inside",
    "url-stop-after",
    "url-cut-read",
];
pub fn contains(scope: &str) -> bool {
    SCOPES.contains(&scope)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Complete,
    StopBefore,
    StopAfter,
    Eof,
}
struct Plan {
    boundaries: Vec<usize>,
    end: End,
}
struct Script {
    plans: Vec<Plan>,
    connection: usize,
    offset: usize,
    filled: usize,
    buffer: Vec<u8>,
    epoch: usize,
    blocked: Option<usize>,
    stopped: bool,
    lifetime: Arc<Lifetime>,
    observed: usize,
}
static SCRIPT: OnceLock<Mutex<Script>> = OnceLock::new();
fn plan(scope: &str, size: usize) -> Plan {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\n\r\n").len();
    let (boundaries, end) = match scope {
        "url-head-split" => (vec![1, 9, head - 1, head, head + size], End::Complete),
        "url-body-split" => (
            vec![head, head + size / 3, head + size / 2, head + size],
            End::Complete,
        ),
        "url-stop-before" => (vec![head], End::StopBefore),
        "url-stop-inside" => (vec![head, head + size / 3], End::StopBefore),
        "url-stop-after" => (vec![head, head + size], End::StopAfter),
        "url-cut-read" => (vec![head, head + size / 3], End::Eof),
        _ => panic!("unknown read schedule"),
    };
    Plan { boundaries, end }
}
pub fn arm(scope: &str, sizes: [usize; 2], lifetime: Arc<Lifetime>) {
    let mut plans = vec![plan(scope, sizes[0])];
    if scope == "url-cut-read" {
        plans.push(plan("url-body-split", sizes[0]));
    }
    plans.push(plan("url-body-split", sizes[1]));
    assert!(
        SCRIPT
            .set(Mutex::new(Script {
                plans,
                connection: usize::MAX,
                offset: 0,
                filled: 0,
                buffer: vec![0; 65536],
                epoch: 0,
                blocked: None,
                stopped: false,
                lifetime,
                observed: 0,
            }))
            .is_ok()
    );
    // Darwin may lazily allocate a mutex's OS storage. Prime it before
    // the running head, so the scripted read callbacks allocate nothing.
    drop(SCRIPT.get().unwrap().lock().unwrap());
    arm_plain_reads(PlainReadHooks { connected, read });
}
fn connected() {
    let mut script = SCRIPT.get().unwrap().lock().unwrap();
    script.connection = script.connection.wrapping_add(1);
    assert!(script.connection < script.plans.len(), "extra connection");
    script.offset = 0;
    script.filled = 0;
    script.blocked = None;
}
/// A piece exposed during one stage poll cannot be followed by another
/// in that same poll, even if hyper polls its driver more than once.
pub fn poll() {
    if let Some(script) = SCRIPT.get() {
        let mut script = script.lock().unwrap();
        script.epoch += 1;
    }
}
fn read(
    socket: &RegisteredSocket,
    cx: &mut Context<'_>,
    buf: &mut ReadBuf<'_>,
) -> Poll<io::Result<()>> {
    let mut script = SCRIPT.get().unwrap().lock().unwrap();
    if script.stopped {
        return Poll::Pending;
    }
    if script.blocked == Some(script.epoch) {
        cx.waker().wake_by_ref();
        return Poll::Pending;
    }
    let current = &script.plans[script.connection];
    let next = current
        .boundaries
        .iter()
        .copied()
        .find(|n| *n > script.offset);
    let end = current.end;
    let Some(next) = next else {
        match end {
            End::StopBefore | End::StopAfter => {
                script.stopped = true;
                script.lifetime.request(Stop::external(StopReason::Sigterm));
                return Poll::Pending;
            }
            // The deterministic peer cut: EOF reaches hyper's incomplete
            // body decoder on the same prefix in every child.
            End::Eof | End::Complete => return Poll::Ready(Ok(())),
        }
    };
    let wanted = (next - script.offset)
        .min(buf.remaining())
        .min(script.buffer.len());
    while script.filled < wanted {
        let filled = script.filled;
        match socket.poll_read(cx, &mut script.buffer[filled..wanted]) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(0)) => panic!("peer ended before scheduled read"),
            Poll::Ready(Ok(n)) => script.filled += n,
        }
    }
    buf.put_slice(&script.buffer[..wanted]);
    script.filled = 0;
    script.offset += wanted;
    if script.offset == next {
        script.observed += 1;
        script.blocked = Some(script.epoch);
        let plan = &script.plans[script.connection];
        if end == End::StopAfter && plan.boundaries.last() == Some(&script.offset) {
            script.stopped = true;
            script.lifetime.request(Stop::external(StopReason::Sigterm));
        }
    }
    Poll::Ready(Ok(()))
}
/// The census must actually complete its prescribed splits or stop point.
/// Refusing children may stop sooner because `host_memory` outranks the scene.
pub fn check(scope: &str) {
    let script = SCRIPT.get().unwrap().lock().unwrap();
    let expected_connections = if scope.contains("stop") {
        1
    } else {
        script.plans.len()
    };
    assert_eq!(script.connection + 1, expected_connections);
    let expected_boundaries: usize = script
        .plans
        .iter()
        .take(expected_connections)
        .map(|p| p.boundaries.len())
        .sum();
    assert_eq!(script.observed, expected_boundaries);
    assert_eq!(script.stopped, scope.contains("stop"));
    println!(
        "CONTROLLER_SCHEDULE kind={scope} connections={expected_connections} boundaries={expected_boundaries}"
    );
}
