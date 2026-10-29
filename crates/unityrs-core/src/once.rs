//! Lazily computed values, published without holding anything while they
//! are computed.
//!
//! A lazy value in the core (a parsed bundle entry, a compiled tree, a
//! container, a registry slot) may take caller code to compute: a
//! [`crate::codec::BlockCodec`], a [`crate::provider::TypeTreeProvider`], the
//! [`crate::budget::MemoryBudget`] every charge books with. Computing it
//! inside `OnceLock::get_or_init` would run that code while the cell is
//! locked, so a callback that reaches the same cell deadlocks, and a binding
//! thread holding Python's GIL while it waits on the cell deadlocks against
//! a computing thread whose Python callback needs the GIL (plan section
//! 3.16). [`get_or_publish`] computes with nothing held and then publishes:
//! the first value published wins, and a value computed by a thread that
//! lost the race is dropped (its charges with it).
//!
//! Owner: the scaffold (implemented). Every lazily computed value in the
//! core goes through it; no `get_or_init` runs anything but a move.

use std::sync::OnceLock;

/// The value in `cell`, computing it with `compute` first when there is
/// none. `compute` runs with no lock held, so it may call caller code, and
/// it may even re-enter this cell; when two threads compute at once, the
/// first to publish wins and the other's value is dropped.
pub fn get_or_publish<T>(cell: &OnceLock<T>, compute: impl FnOnce() -> T) -> &T {
    if let Some(v) = cell.get() {
        return v;
    }
    let v = compute();
    // A move: the only thing that ever runs inside the cell's lock.
    cell.get_or_init(move || v)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock, mpsc};
    use std::time::Duration;

    use super::get_or_publish;

    /// Runs `f` on a thread and fails if it has not returned in 10 s (a
    /// deadlock), rather than hanging the test run.
    fn finishes<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("deadlocked: the compute ran inside the cell")
    }

    #[test]
    fn a_compute_that_re_enters_its_cell_does_not_deadlock() {
        let got = finishes(|| {
            let cell = OnceLock::new();
            // The callback asks for the same lazy value (a codec or provider
            // that reads the same entry): the inner call computes and
            // publishes, and the outer call's value loses.
            let v = *get_or_publish(&cell, || *get_or_publish(&cell, || 2) + 1);
            (v, cell.get().copied())
        });
        assert_eq!(got, (2, Some(2)));
    }

    #[test]
    fn a_thread_holding_a_lock_the_compute_needs_does_not_wait_on_the_cell() {
        // Thread A computes and needs `gil` (a Python codec); thread B holds
        // `gil` and asks for the same value. B must not wait on A's compute.
        let got = finishes(|| {
            let cell = Arc::new(OnceLock::new());
            let gil = Arc::new(std::sync::Mutex::new(()));
            let started = Arc::new(std::sync::Barrier::new(2));
            let a = {
                let (cell, gil, started) = (cell.clone(), gil.clone(), started.clone());
                std::thread::spawn(move || {
                    *get_or_publish(&cell, || {
                        started.wait();
                        let _held = gil.lock().unwrap();
                        1
                    })
                })
            };
            let held = gil.lock().unwrap();
            started.wait();
            let b = *get_or_publish(&cell, || 2);
            drop(held);
            let a = a.join().unwrap();
            (a, b)
        });
        assert_eq!(got.0, got.1, "both see the one published value");
    }

    #[test]
    fn the_first_published_value_wins_and_later_computes_never_run() {
        let runs = AtomicUsize::new(0);
        let cell = OnceLock::new();
        let a = *get_or_publish(&cell, || {
            runs.fetch_add(1, Ordering::Relaxed);
            7
        });
        let b = *get_or_publish(&cell, || {
            runs.fetch_add(1, Ordering::Relaxed);
            8
        });
        assert_eq!((a, b, runs.load(Ordering::Relaxed)), (7, 7, 1));
    }
}
