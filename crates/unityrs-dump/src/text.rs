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

/// `buf` with no spare room: shrunk by the allocator, or moved to a new
/// allocation where a shrink would keep a mapped block, a page or more
/// against the charge of a heap chunk ([`shrink_moves`]; review round 13).
/// `false`, with `buf` as it was, when the allocator refuses the new
/// allocation.
fn shrink(buf: &mut String) -> bool {
    if shrink_moves(bytes(buf.len()), bytes(buf.capacity())) {
        let mut exact = String::new();
        if exact.try_reserve_exact(buf.len()).is_err() {
            return false;
        }
        exact.push_str(buf);
        *buf = exact;
    } else {
        buf.shrink_to_fit();
    }
    true
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
    /// as the text does; spare room is given back first where the meter
    /// grants the smaller block (both exist while the allocator moves it).
    ///
    /// # Errors
    /// The refusal that stopped a growth.
    pub fn finish(mut self) -> Result<(String, Charge), ChargeError> {
        if let Some(e) = self.refused {
            return Err(e);
        }
        let (len, cap) = (self.buf.len(), self.buf.capacity());
        if cap > len
            && let Ok(smaller) = self.charge.meter().charge(heap_cost(bytes(len)))
            && shrink(&mut self.buf)
        {
            self.charge.release(heap_cost(bytes(cap)));
            self.charge.absorb(smaller);
        }
        Ok((self.buf, self.charge))
    }

    /// Room for `more` bytes, growing geometrically and charged first, at
    /// exactly what is needed when a doubling is refused; `false` once a
    /// growth has been refused.
    fn reserve(&mut self, more: usize) -> bool {
        if self.refused.is_some() {
            return false;
        }
        let need = self.buf.len().saturating_add(more);
        if need <= self.buf.capacity() {
            return true;
        }
        let doubled = self.buf.capacity().saturating_mul(2).max(need).max(FIRST);
        let grown = match self.grow_to(doubled) {
            Err(_) if doubled > need => self.grow_to(need),
            r => r,
        };
        match grown {
            Ok(()) => true,
            Err(e) => {
                self.refused = Some(e);
                false
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
            Some(ChargeError::Limit(l)) if l.kind == LimitKind::InputMemory
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
            Err(ChargeError::Limit(l)) if l.kind == LimitKind::DecodeWork
        ));
        let mut s = String::new();
        assert!(s.count_work(usize::MAX), "a String counts nothing");
    }

    /// A doubling the meter refuses is asked again at exactly what is
    /// needed, so a growth that fits the meter (both blocks, while the
    /// allocator moves the text) is never refused by the growth policy.
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
}
