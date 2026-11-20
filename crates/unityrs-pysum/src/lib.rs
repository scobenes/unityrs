// Ported from CPython 3.15.2 (PSF-2.0): Python/bltinmodule.c, builtin_sum_impl's float path, with its CompensatedSum, cs_from_double, cs_add and cs_to_double. See THIRD_PARTY_LICENSES.
// Altered: translated to Rust, for an iterator of f64 only (no int,
// complex or other items, no start value but the integer 0, and no
// fallback to generic addition); NOTICE-PSF summarizes the changes.

//! The float sum freeze v8 of the humanoid body model was fitted and
//! validated under: Python 3.15.2's builtin `sum`, which from 3.12
//! compensates float additions.
//!
//! It is Neumaier's improved Kahan-Babuska summation (A. Neumaier, 1974,
//! "Rundungsfehleranalyse einiger Verfahren zur Summation endlicher
//! Summen", ZAMM 54: 39-51), as `CPython`'s `sum` performs it: each step adds
//! `x` to the running sum `hi` and the rounding it lost, `(hi - t) + x` or
//! `(x - t) + hi` for whichever of `hi` and `x` is larger in magnitude, to a
//! compensation `lo`. As Python's `sum` does, it starts from the integer 0
//! (so the first float becomes `0.0 + x0`, a `-0.0` becoming `0.0`), and it
//! adds the compensation at the end only when that is nonzero and finite, so
//! an infinite or overflowed sum stays infinite and a negative zero keeps
//! its sign. An empty sum is 0. The tests hold it bit for bit to Python
//! 3.15.2's `sum` on random and adversarial vectors
//! (`tests/support/python_sum.json`, written by
//! `tools/parity/vectors/test_anim_python_sum.py`).

/// The running sum and its compensation.
#[derive(Clone, Copy)]
struct Compensated {
    hi: f64,
    lo: f64,
}

impl Compensated {
    /// One compensated step.
    fn add(self, x: f64) -> Self {
        let Self { hi, mut lo } = self;
        let t = hi + x;
        if hi.abs() >= x.abs() {
            lo += (hi - t) + x;
        } else {
            lo += (x - t) + hi;
        }
        Self { hi: t, lo }
    }

    /// The sum, with the compensation added when it is nonzero and finite.
    fn value(self) -> f64 {
        if self.lo != 0. && self.lo.is_finite() {
            self.hi + self.lo
        } else {
            self.hi
        }
    }
}

/// Python 3.12+'s `sum(values)` for float `values`.
#[must_use]
pub fn python_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut values = values.into_iter();
    let Some(first) = values.next() else {
        return 0.;
    };
    values
        .fold(
            Compensated {
                hi: 0. + first,
                lo: 0.,
            },
            Compensated::add,
        )
        .value()
}
