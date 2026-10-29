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
//! **Fallible values** go through [`get_or_publish_result`], which publishes
//! a value and the errors the input decides, and never a *transient* one: a
//! budget or meter refusal depends on what else is held at that moment (two
//! threads computing the same value at once can cause it themselves), so
//! publishing it would fail every later ask of a shared dependency for the
//! handle's life, however empty the pool later is. A transient error is
//! returned unpublished and the next ask computes again. A published error
//! is charged to the input's meter for the heap it owns ([`CellError`]), so
//! a table of failing cells is paid for like any other.
//!
//! **Work.** A computation that ends in an error its cell does not keep runs
//! again on the next ask, so an input could make its readers repeat it
//! without end (one type whose compile alone needs more than the meter,
//! asked by every object of the type). Every byte the meter granted while
//! such a computation ran is therefore counted as the open's work
//! ([`crate::limits::Limits::max_decode_work`], shared with block decodes),
//! and once that allowance is used up an ask that would compute keeps a
//! [`crate::LimitKind::DecodeWork`] refusal instead, which is not
//! transient. The count is the meter's grants between the computation's
//! start and its end, so a grant another thread made on the same meter in
//! that window counts too: the bound errs towards stopping early, never
//! late.
//!
//! A compute has no side effect but its charges: what follows from a value
//! (registering a nested container's entries) is done from the published
//! value, after publishing, so a value that lost the race leaves nothing.
//!
//! Owner: the scaffold (implemented). Every lazily computed value in the
//! core goes through it; no `get_or_init` runs anything but a move.

use std::fmt;
use std::sync::{Arc, OnceLock};

use crate::error::LimitError;
use crate::limits::{Charge, Meter};

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

/// What a cell must know about an error before it keeps it.
pub trait CellError: Send + Sync {
    /// Whether the error depends on more than the input: a budget or meter
    /// refusal ([`crate::LimitKind::InputMemory`]), an I/O failure, a
    /// provider's failure. Such an error is returned, never published.
    fn is_transient(&self) -> bool;
    /// The heap the error owns ([`crate::limits::heap_cost`] of each box and
    /// string it holds, counted whether or not a string shares a file's
    /// buffer), charged to the input's meter while a cell keeps it.
    fn heap_bytes(&self) -> u64;
    /// The error for an ask refused because its open's work allowance is
    /// used up (`limit` is a [`crate::LimitKind::DecodeWork`] refusal): not
    /// transient, so the cell keeps it (module docs).
    fn work_refused(limit: LimitError) -> Self
    where
        Self: Sized;
}

/// An error a cell keeps, with the charge for its heap.
pub struct KeptError<E> {
    error: E,
    _charge: Charge,
}

impl<E> KeptError<E> {
    /// The error.
    pub fn error(&self) -> &E {
        &self.error
    }
}

impl<E: fmt::Debug> fmt::Debug for KeptError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

/// A cell of a fallible value: the value, or an input-determined error with
/// its charge.
pub type ResultCell<T, E> = OnceLock<Result<T, KeptError<E>>>;

/// The value in `cell`, computing it with `compute` first when there is
/// none (as [`get_or_publish`]). An error the input decides is published
/// with a charge on `meter` for its heap, and returned (cloned) to every
/// later ask; a transient error ([`CellError::is_transient`]), or one whose
/// charge the meter refuses, is returned unpublished, so the next ask
/// computes again. `meter` is the meter of the open the computation charges:
/// what it granted during a computation that ended unpublished counts as
/// that open's work, and once the work allowance is used up an ask keeps a
/// refusal instead of computing (module docs).
pub fn get_or_publish_result<'a, T, E: CellError + Clone>(
    cell: &'a ResultCell<T, E>,
    meter: &Arc<Meter>,
    compute: impl FnOnce() -> Result<T, E>,
) -> Result<&'a T, E> {
    let published = |r: &'a Result<T, KeptError<E>>| r.as_ref().map_err(|k| k.error.clone());
    if let Some(r) = cell.get() {
        return published(r);
    }
    if let Some(used_up) = meter.work_used_up() {
        return keep(cell, meter, E::work_refused(used_up));
    }
    let before = meter.granted();
    match compute() {
        Ok(v) => published(cell.get_or_init(move || Ok(v))),
        Err(e) if e.is_transient() => {
            meter.add_work(meter.granted().saturating_sub(before));
            Err(e)
        }
        Err(e) => keep(cell, meter, e),
    }
}

/// Publishes `e` in `cell` with a charge on `meter` for its heap and returns
/// what the cell holds; an error the meter has no room to keep is returned
/// unpublished.
fn keep<'a, T, E: CellError + Clone>(
    cell: &'a ResultCell<T, E>,
    meter: &Arc<Meter>,
    e: E,
) -> Result<&'a T, E> {
    match meter.charge(e.heap_bytes()) {
        Ok(charge) => {
            let kept = KeptError {
                error: e,
                _charge: charge,
            };
            cell.get_or_init(move || Err(kept))
                .as_ref()
                .map_err(|k| k.error.clone())
        }
        Err(_) => Err(e),
    }
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

    /// A failure for the cell tests: transient or not, owning `heap` bytes.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Failed {
        transient: bool,
        heap: u64,
    }

    impl super::CellError for Failed {
        fn is_transient(&self) -> bool {
            self.transient
        }
        fn heap_bytes(&self) -> u64 {
            self.heap
        }
        fn work_refused(_: crate::error::LimitError) -> Self {
            Self {
                transient: false,
                heap: 0,
            }
        }
    }

    #[test]
    fn a_refusal_is_not_published_and_the_next_ask_computes_again() {
        use crate::budget::{Counting, Reservation};
        use crate::error::Error;
        use crate::limits::Meter;
        // Round 3's probe: a pool of 100 bytes with 90 held elsewhere
        // refuses the compute's 20; once the other reader drops, the same
        // cell must compute again, not answer the old refusal.
        let pool = Arc::new(Counting::new(100));
        let dep = Meter::new(u64::MAX, pool.clone(), Reservation::Dependency);
        let other = dep.charge(90).unwrap();
        let cell: super::ResultCell<(u32, crate::limits::Charge), Error> = OnceLock::new();
        let compute = || dep.charge(20).map(|c| (7, c)).map_err(Error::from);
        let first = super::get_or_publish_result(&cell, &dep, compute).map(|v| v.0);
        assert!(matches!(first, Err(Error::Budget(_))), "{first:?}");
        assert!(cell.get().is_none(), "a refusal is not published");
        drop(other);
        let second = super::get_or_publish_result(&cell, &dep, compute).map(|v| v.0);
        assert_eq!(second.ok(), Some(7));
        assert_eq!(pool.used(), 20, "the published value keeps its charge");
    }

    #[test]
    fn a_compute_refused_on_its_own_stops_at_the_work_bound() {
        use crate::budget::{Reservation, Unbounded};
        use crate::error::{Error, LimitKind};
        use crate::limits::{Limits, Meter};
        // Round 5: a compute the meter refuses with nothing else held is not
        // the pool's contention but the input's own need, and it ran again
        // on every ask. Its grants count as work: 300 bytes per refused run
        // against 1,000 of work, so the fourth run uses the allowance up and
        // every later ask answers the kept refusal without computing.
        let limits = Limits {
            max_input_memory: 500,
            max_decode_work: 1000,
            ..Limits::default()
        };
        let m = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        let cell: super::ResultCell<u32, Error> = OnceLock::new();
        let runs = AtomicUsize::new(0);
        let compute = || {
            runs.fetch_add(1, Ordering::Relaxed);
            let _a = m.charge(300)?;
            let _b = m.charge(300)?;
            Ok(7)
        };
        let got: Vec<_> = (0..50)
            .map(|_| super::get_or_publish_result(&cell, &m, compute).copied())
            .collect();
        assert_eq!(runs.load(Ordering::Relaxed), 4);
        assert!(
            got[..4]
                .iter()
                .all(|r| matches!(r, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory))
        );
        assert!(
            got[4..]
                .iter()
                .all(|r| matches!(r, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork))
        );
        assert!(cell.get().is_some(), "the work refusal is kept");
        assert_eq!((m.decoded(), m.used()), (1200, 0));
    }

    #[test]
    fn an_input_error_is_published_with_a_charge_for_its_heap() {
        use crate::budget::{Reservation, Unbounded};
        use crate::limits::Meter;
        let m = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
        let cell: super::ResultCell<u32, Failed> = OnceLock::new();
        let bad = Failed {
            transient: false,
            heap: 48,
        };
        let runs = AtomicUsize::new(0);
        for _ in 0..3 {
            let got = super::get_or_publish_result(&cell, &m, || {
                runs.fetch_add(1, Ordering::Relaxed);
                Err(bad.clone())
            });
            assert_eq!(got, Err(bad.clone()));
        }
        assert_eq!(runs.load(Ordering::Relaxed), 1, "computed once");
        assert_eq!(m.used(), 48, "the kept error is charged");
        drop(cell);
        assert_eq!(m.used(), 0);
        // An error the meter has no room to keep is returned, not kept.
        let full = Meter::new(10, Arc::new(Unbounded), Reservation::Input);
        let cell: super::ResultCell<u32, Failed> = OnceLock::new();
        assert!(super::get_or_publish_result(&cell, &full, || Err(bad.clone())).is_err());
        assert!(cell.get().is_none());
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
