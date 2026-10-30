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
use unityrs_core::limits::{Charge, ChargeError, Meter, heap_cost};

/// Where a JSON text is written: a `String`, which grows as `String` does,
/// or a [`Text`].
pub trait Sink: fmt::Write + Send + Sync {
    /// Appends `s`.
    fn push_str(&mut self, s: &str);
    /// Appends `c`.
    fn push(&mut self, c: char);
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
        {
            self.buf.shrink_to_fit();
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
