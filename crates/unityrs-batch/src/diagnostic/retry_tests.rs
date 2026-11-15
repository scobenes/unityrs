//! Exercise real pending-line accounting under controlled syscall failures.
use super::*;
use std::{cell::RefCell, io, time::Instant};

struct Probe {
    now: Instant,
    attempts: usize,
    pauses: usize,
    interrupts: usize,
    delivered: Vec<u8>,
}
thread_local! {
    static PROBE: RefCell<Probe> = RefCell::new(Probe {
        now: Instant::now(), attempts: 0, pauses: 0,
        interrupts: usize::MAX, delivered: Vec::new(),
    });
}
fn write(bytes: &[u8]) -> io::Result<usize> {
    PROBE.with_borrow_mut(|p| {
        p.attempts += 1;
        assert!(p.attempts <= 1024, "EINTR storm exceeded the attempt cap");
        if p.attempts <= p.interrupts {
            Err(io::ErrorKind::Interrupted.into())
        } else {
            p.delivered.extend_from_slice(bytes);
            Ok(bytes.len())
        }
    })
}
fn pause(wait: std::time::Duration) {
    assert!(!wait.is_zero());
    assert!(wait <= crate::progress::early::RETRY);
    PROBE.with_borrow_mut(|p| p.pauses += 1);
}
fn warning(interrupts: usize) -> Writer {
    PROBE.with_borrow_mut(|p| {
        p.attempts = 0;
        p.pauses = 0;
        p.interrupts = interrupts;
        p.delivered.clear();
    });
    crate::sys::stderr_retry_hooks(Some(crate::sys::StderrRetryHooks {
        write,
        now: || PROBE.with_borrow(|p| p.now),
        sleep: pause,
        finished: || {},
    }));
    let writer = Writer {
        route: crate::sys::stderr::Route::Ordinary,
        dropped: AtomicU64::new(0),
    };
    let pending = writer.pending_line();
    assert_eq!(writer.dropped(), 1, "unresolved line counted once");
    pending.within(
        "warning\n",
        &Secrets {
            values: &[],
            credentials: &OnceLock::new(),
        },
        &mut [0; LINE_BYTES],
        crate::progress::early::OWN_WAIT,
    );
    crate::sys::stderr_retry_hooks(None);
    writer
}

#[test]
fn an_eintr_storm_stops_at_the_cap_and_keeps_exactly_one_drop() {
    let writer = warning(usize::MAX);
    assert_eq!(writer.dropped(), 1);
    PROBE.with_borrow(|p| {
        assert_eq!(p.attempts, 1024);
        assert_eq!(p.pauses, 1023, "back off between interrupted attempts");
        assert_eq!(p.delivered, b"");
    });
}

#[test]
fn a_transient_eintr_burst_delivers_and_clears_the_pending_drop() {
    let writer = warning(3);
    assert_eq!(writer.dropped(), 0);
    PROBE.with_borrow(|p| {
        assert_eq!(p.attempts, 4);
        assert_eq!(p.pauses, 3);
        assert_eq!(p.delivered, b"warning\n");
    });
}
