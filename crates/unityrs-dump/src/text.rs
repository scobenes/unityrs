//! Where the dump writes JSON text ([`Sink`]): a `String`, or a [`Text`],
//! whose every growth is paid for before it happens.
//!
//! **Why a [`Text`].** The run holds one input's records until it writes
//! them ([`crate::run`]), and they can be thousands of times the input: an
//! object of 4 data bytes can read as 2^20 empty structs, each written out,
//! and every object record repeats its file's whole container path. Built
//! in plain `String`s, they were charged to nothing, so a 2 KB input could
//! make the dump ask the allocator for more than the process had, and Rust
//! aborts the process when the allocator refuses: every input's records
//! were lost. A [`Text`] charges each growth to the input's meter first, so
//! the records count toward `Limits::max_input_memory` (and the caller's
//! budget) with everything the input's parse holds, and reserves it with
//! `try_reserve_exact`, so a refusal of the meter, the budget or the
//! allocator is an error, which [`crate::input`] makes the input's one
//! record, and the run goes on.

use std::fmt;
use std::sync::Arc;

use unityrs_core::budget::BudgetExceeded;
use unityrs_core::limits::{Charge, ChargeError, Meter, heap_cost, shrink_moves};

/// Where a JSON text is written: a `String`, which grows as `String` does,
/// or a [`Text`].
pub trait Sink: fmt::Write + Send + Sync {
    /// Appends `s`.
    fn push_str(&mut self, s: &str);
    /// Appends `c`.
    fn push(&mut self, c: char);
    /// Counts `bytes` of work the writer is about to do that writes nothing
    /// of their size (a summary's sha256), before it does it: `false` when
    /// the sink refuses it, and the work must not be done. A `String`
    /// counts nothing; a [`Text`] counts it as its meter's work.
    fn count_work(&mut self, bytes: usize) -> bool {
        let _ = bytes;
        true
    }
    /// The meter that memory a writer holds beside the text while it writes
    /// (the value encoder's stack of open values) is charged to: a
    /// [`Text`]'s. A `String` has none, and that memory is charged to an
    /// unlimited meter, so only the allocator refuses it.
    fn meter(&self) -> Option<&Arc<Meter>> {
        None
    }
    /// Stops the text on `e`, a refusal of memory a writer charged to
    /// [`Sink::meter`], as a refused growth stops it: `None` once the sink
    /// holds the refusal, which [`Text::finish`] returns; `Some(e)` back from
    /// a sink that holds none (a `String`), for the writer to report.
    fn stop(&mut self, e: ChargeError) -> Option<ChargeError> {
        Some(e)
    }
}

const _: () = crate::send_sync::<dyn Sink>();

impl Sink for String {
    fn push_str(&mut self, s: &str) {
        String::push_str(self, s);
    }

    fn push(&mut self, c: char) {
        String::push(self, c);
    }
}

/// The least room a [`Text`] asks for.
const FIRST: usize = 64;

fn bytes(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// `buf` with no spare room, `charge` (which holds [`heap_cost`] of its
/// capacity) following it, as the core's `MeteredVec::shrink_to_fit` does:
/// shrunk in place by the allocator, which holds no second block, so
/// nothing more is charged and the difference is returned after (perf stage
/// 2: the smaller block was charged beside the old one); or moved to a new
/// allocation where a shrink would keep a mapped block, a page or more
/// against the charge of a heap chunk ([`shrink_moves`]; review round 13),
/// that block charged first, as both exist while the bytes move. A move the
/// meter or the allocator refuses leaves `buf` and `charge` as they were.
fn shrink(buf: &mut String, charge: &mut Charge) {
    let (len, cap) = (bytes(buf.len()), bytes(buf.capacity()));
    if len >= cap {
        return;
    }
    if shrink_moves(len, cap) {
        let Ok(smaller) = charge.meter().charge(heap_cost(len)) else {
            return;
        };
        let mut exact = String::new();
        if exact.try_reserve_exact(buf.len()).is_err() {
            return;
        }
        exact.push_str(buf);
        *buf = exact;
        charge.release(heap_cost(cap));
        charge.absorb(smaller);
    } else {
        buf.shrink_to_fit();
        charge.release(heap_cost(cap) - heap_cost(len));
    }
}

/// A JSON text whose every growth is charged to a [`Meter`] before it is
/// reserved, and reserved fallibly (module docs). After a refusal it writes
/// nothing more, and [`Text::finish`] returns the refusal.
#[derive(Debug)]
pub struct Text {
    buf: String,
    /// The charge for `buf`'s allocation.
    charge: Charge,
    refused: Option<ChargeError>,
}

const _: () = crate::send_sync::<Text>();

impl Text {
    /// An empty text charging `meter`.
    #[must_use]
    pub fn on(meter: &Arc<Meter>) -> Self {
        Self {
            buf: String::new(),
            charge: Charge::zero(meter),
            refused: None,
        }
    }

    /// What has been written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.buf
    }

    /// The refusal that stopped this text, if one did.
    #[must_use]
    pub fn refused(&self) -> Option<&ChargeError> {
        self.refused.as_ref()
    }

    /// The text and the charge for its allocation, which must live as long
    /// as the text does; spare room is given back first: in place, or, where
    /// the bytes move, when the meter grants the smaller block (both exist
    /// while they move).
    ///
    /// # Errors
    /// The refusal that stopped a growth.
    pub fn finish(mut self) -> Result<(String, Charge), ChargeError> {
        if let Some(e) = self.refused {
            return Err(e);
        }
        shrink(&mut self.buf, &mut self.charge);
        Ok((self.buf, self.charge))
    }

    /// Room for `more` bytes, growing geometrically and charged first. A
    /// growth the meter, the budget or the allocator refuses is asked for
    /// again with half the room past what is needed, and so on down to
    /// exactly what is needed, so a text that meets a bound grows a few
    /// dozen more times at most, each time by at least half of what could
    /// still be granted. (Fix pass 15: once a doubling was refused it asked
    /// for the doubling again on every write past its room, then for
    /// exactly that write's bytes, so each such write cost a refused
    /// request, a failed `mmap` where the allocator is what refuses:
    /// `a_text_that_meets_its_bound_asks_a_few_dozen_times`.) `false` once
    /// a growth of exactly what is needed has been refused.
    fn reserve(&mut self, more: usize) -> bool {
        if self.refused.is_some() {
            return false;
        }
        let need = self.buf.len().saturating_add(more);
        if need <= self.buf.capacity() {
            return true;
        }
        let mut target = self.buf.capacity().saturating_mul(2).max(need).max(FIRST);
        loop {
            match self.grow_to(target) {
                Ok(()) => return true,
                Err(_) if target > need => target = need + (target - need) / 2,
                Err(e) => {
                    self.refused = Some(e);
                    return false;
                }
            }
        }
    }

    /// Grows the allocation to `cap` bytes: the new block is charged before
    /// it is asked for, and the old one's charge returned once it is gone.
    fn grow_to(&mut self, cap: usize) -> Result<(), ChargeError> {
        let old = heap_cost(bytes(self.buf.capacity()));
        let new = heap_cost(bytes(cap));
        let meter = self.charge.meter().clone();
        let mut extra = meter.charge(new)?;
        if self
            .buf
            .try_reserve_exact(cap.saturating_sub(self.buf.len()))
            .is_err()
        {
            // `extra` drops here: nothing stays charged for a failed allocation.
            return Err(ChargeError::Budget(BudgetExceeded {
                requested: new,
                what: meter.reservation(),
            }));
        }
        let got = heap_cost(bytes(self.buf.capacity()));
        if got > new && extra.grow(got - new).is_err() {
            self.buf.shrink_to(cap);
        }
        self.charge.release(old);
        self.charge.absorb(extra);
        Ok(())
    }
}

impl Sink for Text {
    fn meter(&self) -> Option<&Arc<Meter>> {
        Some(self.charge.meter())
    }

    /// Holds `e` as a refused growth is held, unless an earlier refusal is.
    fn stop(&mut self, e: ChargeError) -> Option<ChargeError> {
        self.refused.get_or_insert(e);
        None
    }

    fn push_str(&mut self, s: &str) {
        if self.reserve(s.len()) {
            self.buf.push_str(s);
        }
    }

    fn push(&mut self, c: char) {
        if self.reserve(c.len_utf8()) {
            self.buf.push(c);
        }
    }

    /// Counted toward the meter's `max_decode_work` ([`Meter::count_decode`]):
    /// a value's bytes are a zero-copy slice of its file, which objects may
    /// share, so summarising them is work no memory bound stops. A refusal
    /// stops the text, as a refused growth does.
    fn count_work(&mut self, bytes: usize) -> bool {
        if self.refused.is_some() {
            return false;
        }
        match self.charge.meter().count_decode(self::bytes(bytes)) {
            Ok(()) => true,
            Err(e) => {
                self.refused = Some(e);
                false
            }
        }
    }
}

impl fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        Sink::push_str(self, s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use unityrs_core::LimitKind;
    use unityrs_core::budget::{Counting, Reservation, Unbounded};

    #[test]
    fn a_text_holds_what_it_charged_and_gives_it_back() {
        let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let mut t = Text::on(&meter);
        for i in 0..1000 {
            let _ = write!(t, "{i},");
        }
        assert_eq!(t.as_str().len(), 3890);
        assert_eq!(meter.used(), heap_cost(bytes(t.buf.capacity())));
        let (s, charge) = t.finish().unwrap();
        assert!(s.starts_with("0,1,2,") && s.ends_with("998,999,"));
        assert_eq!(s.capacity(), s.len(), "spare room is given back");
        assert_eq!(meter.used(), heap_cost(bytes(s.len())));
        drop(charge);
        assert_eq!(meter.used(), 0);
    }

    /// Perf stage 2 (fix B): spare room is given back in place, charging
    /// nothing beside the old block, so a meter with no room for a second
    /// block still gets it back. 200,000 bytes written 1,000 at a time end
    /// in room of 256,000 (both over 128 KiB, so shrunk in place,
    /// `shrink_moves`); the meter holds the last doubling, 128,000 and
    /// 256,000 at once, and no more. Before, the 200,000 bytes were charged
    /// beside the 256,000, which the meter refused, so the text kept its
    /// room, charged.
    #[test]
    fn spare_room_is_given_back_in_place_charging_nothing_beside_it() {
        let limit = heap_cost(128_000) + heap_cost(256_000);
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        let mut t = Text::on(&meter);
        let piece = "x".repeat(1000);
        for _ in 0..200 {
            t.write_str(&piece).unwrap();
        }
        assert_eq!((t.as_str().len(), t.buf.capacity()), (200_000, 256_000));
        assert!(!shrink_moves(200_000, 256_000));
        let (s, charge) = t.finish().unwrap();
        assert_eq!(s.capacity(), 200_000, "spare room is given back");
        assert_eq!(
            (meter.used(), charge.bytes()),
            (heap_cost(200_000), heap_cost(200_000))
        );
    }

    /// Fix B's other branch (perf stage 2's review): spare room the
    /// allocator would keep mapped (a block of 128 KiB or more whose bytes
    /// would be a heap chunk, `shrink_moves`) is given back by moving the
    /// bytes, the smaller block charged first, as both exist while they
    /// move, and the old block's charge then returned. 100,000 bytes, then
    /// 10,000 more, end in room of 200,000. With room for both blocks the
    /// text ends at its length, charged at its size; without room for the
    /// smaller block beside the old one it keeps its room, charged as it is.
    #[test]
    fn spare_room_given_back_by_a_move_is_charged_before_it_moves() {
        assert!(shrink_moves(110_000, 200_000));
        let both = heap_cost(200_000) + heap_cost(110_000);
        for (limit, cap) in [(both, 110_000), (both - 1, 200_000)] {
            let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
            let mut t = Text::on(&meter);
            t.write_str(&"x".repeat(100_000)).unwrap();
            t.write_str(&"y".repeat(10_000)).unwrap();
            assert_eq!(t.buf.capacity(), 200_000);
            let (s, charge) = t.finish().unwrap();
            assert_eq!((s.len(), s.capacity()), (110_000, cap), "{limit}");
            assert_eq!(
                (meter.used(), charge.bytes()),
                (heap_cost(bytes(cap)), heap_cost(bytes(cap))),
                "{limit}"
            );
        }
    }

    /// The meter's bound and the budget each stop a text, and a refused
    /// growth writes nothing more and keeps nothing charged.
    #[test]
    fn a_refused_growth_stops_the_text() {
        let meter = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
        let mut t = Text::on(&meter);
        t.push_str(&"x".repeat(500));
        t.push_str(&"y".repeat(600));
        t.push('z');
        assert_eq!(t.as_str(), "x".repeat(500));
        assert!(matches!(
            t.refused(),
            Some(ChargeError::Limit(l)) if l.kind() == LimitKind::InputMemory
        ));
        assert!(matches!(t.finish(), Err(ChargeError::Limit(_))));
        assert_eq!(meter.used(), 0);

        let budget = Arc::new(Counting::new(1000));
        let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let mut t = Text::on(&meter);
        t.push_str(&"x".repeat(2000));
        assert!(matches!(t.finish(), Err(ChargeError::Budget(_))));
        assert_eq!(budget.used(), 0);
    }

    /// A refusal of memory a writer charged to the text's meter beside it
    /// (the value encoder's stack) stops the text as a refused growth does,
    /// the first refusal kept; a `String` holds none and gives it back.
    #[test]
    fn a_stop_is_held_as_a_refused_growth() {
        let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let refusal = |requested| {
            ChargeError::Budget(BudgetExceeded {
                requested,
                what: Reservation::Input,
            })
        };
        let mut t = Text::on(&meter);
        assert!(Arc::ptr_eq(t.meter().unwrap(), &meter));
        t.push('a');
        assert!(t.stop(refusal(1)).is_none());
        assert!(t.stop(refusal(2)).is_none());
        t.push_str("b");
        assert!(!t.count_work(0));
        assert_eq!(t.as_str(), "a");
        assert!(matches!(
            t.finish(),
            Err(ChargeError::Budget(BudgetExceeded { requested: 1, .. }))
        ));
        assert_eq!(meter.used(), 0);
        let mut s = String::new();
        assert!(s.meter().is_none());
        assert!(matches!(s.stop(refusal(3)), Some(ChargeError::Budget(_))));
    }

    /// A summary's work is counted as the meter's, before it is done, and
    /// past the bound it stops the text with the meter's `DecodeWork`
    /// refusal.
    #[test]
    fn a_texts_work_is_its_meters() {
        let mut limits = unityrs_core::limits::Limits::default();
        limits.max_decode_work = 100;
        let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        let mut t = Text::on(&meter);
        assert!(t.count_work(60));
        t.push('a');
        assert!(t.count_work(40));
        assert_eq!(meter.decoded(), 100);
        assert!(!t.count_work(1));
        t.push('b');
        assert_eq!(t.as_str(), "a");
        assert!(!t.count_work(0), "a stopped text counts nothing more");
        assert!(matches!(
            t.finish(),
            Err(ChargeError::Limit(l)) if l.kind() == LimitKind::DecodeWork
        ));
        let mut s = String::new();
        assert!(s.count_work(usize::MAX), "a String counts nothing");
    }

    /// A doubling the meter refuses is asked again with less room, down to
    /// exactly what is needed, so a growth that fits the meter (both
    /// blocks, while the allocator moves the text) is never refused by the
    /// growth policy.
    #[test]
    fn a_growth_that_fits_is_never_refused_by_doubling() {
        let limit = heap_cost(600) + heap_cost(1000);
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        let mut t = Text::on(&meter);
        t.push_str(&"x".repeat(600));
        t.push_str(&"y".repeat(400));
        assert_eq!(t.finish().unwrap().0.len(), 1000);
        // One byte less, and the exact growth is refused too.
        let meter = Meter::new(limit - 1, Arc::new(Unbounded), Reservation::Input);
        let mut t = Text::on(&meter);
        t.push_str(&"x".repeat(600));
        t.push_str(&"y".repeat(400));
        assert!(t.finish().is_err());
    }

    /// A meter of `limit` bytes that counts the charges it refuses.
    fn refusals_counted(limit: u64) -> (Arc<Meter>, Arc<std::sync::atomic::AtomicU64>) {
        use std::sync::atomic::{AtomicU64, Ordering};
        use unityrs_core::budget::{MemoryBudget, Permit};
        #[derive(Debug)]
        struct Tally(Counting, Arc<AtomicU64>);
        impl MemoryBudget for Tally {
            fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
                self.0.try_reserve(bytes, what).inspect_err(|_| {
                    self.1.fetch_add(1, Ordering::Relaxed);
                })
            }
        }
        let refused = Arc::new(AtomicU64::new(0));
        let budget = Tally(Counting::new(limit), refused.clone());
        let meter = Meter::new(u64::MAX, Arc::new(budget), Reservation::Input);
        (meter, refused)
    }

    /// A text that meets its bound grows a few dozen more times at most:
    /// a refused growth is asked again with half the room past what is
    /// needed, down to exactly that, never once for every write past its
    /// room. Here 1,000,000 one-byte writes meet a budget of 1,200 KiB
    /// with 23 refused requests, 655,360 bytes written. Before fix pass 15
    /// the refused doubling was asked again on every write past the room,
    /// then exactly that write's bytes: 90,090 refused requests (each a
    /// failed `mmap` where the allocator is what refuses), 614,376 bytes
    /// written. A bound 128 times the size asks about as many, in writes of
    /// 4 KiB.
    #[test]
    fn a_text_that_meets_its_bound_asks_a_few_dozen_times() {
        use std::sync::atomic::Ordering;
        // Room of 512 KiB doubled is refused, and up to about 640 KiB fits
        // beside it: the writes past 512 KiB are where the growths are asked.
        for (limit, write, writes, at_least) in [
            (1_200_u64 << 10, 1, 1_000_000, 512 << 10),
            (150_u64 << 20, 4096, 40_000, 64 << 20),
        ] {
            let (meter, refused) = refusals_counted(limit);
            let mut t = Text::on(&meter);
            let piece = "x".repeat(write);
            let mut written = 0;
            for _ in 0..writes {
                t.push_str(&piece);
                if t.refused().is_none() {
                    written += write;
                }
            }
            let refusals = refused.load(Ordering::Relaxed);
            assert!(written > at_least, "{written}");
            assert!(t.refused().is_some());
            assert!(
                refusals <= 64,
                "{refusals} refused requests for {written} bytes written"
            );
        }
    }
}
