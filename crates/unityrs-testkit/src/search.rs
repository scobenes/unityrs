//! A limit's **need** (`docs/plans/extract.md` X13): the least value at
//! which a run meets no refusal of it. [`least`] brackets and bisects a
//! refusal predicate, then checks it is monotone where the bisection
//! assumed it: at the need's adjacent values and at sampled values on both
//! sides, besides every value the bisection itself tried. A run's
//! allocation path adapts (a growth retried smaller, an eviction), so
//! monotonicity is checked, not assumed; a [`Counterexample`] is no need,
//! and comes back for a spec ruling instead of a squeeze result.

use std::collections::BTreeMap;
use std::fmt;

/// What [`least`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Least {
    /// The least value whose run met no refusal.
    pub need: u64,
    /// Every value tried, and whether its run was refused.
    pub tried: BTreeMap<u64, bool>,
}

/// A value whose answer the need contradicts: refused at or above it, or
/// not refused below it; or a top value that is itself refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counterexample {
    /// The value.
    pub value: u64,
    /// Whether its run was refused.
    pub refused: bool,
    /// The need the bisection found (`None`: the top was refused).
    pub need: Option<u64>,
}

impl fmt::Display for Counterexample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.need {
            None => write!(f, "the top value {} is itself refused", self.value),
            Some(need) => write!(
                f,
                "not monotone: {} is {} while the bisected need is {need}; this needs a spec \
                 ruling, not a squeeze result",
                self.value,
                if self.refused { "refused" } else { "clear" },
            ),
        }
    }
}

/// The values [`least`] checks beside the bisection's own: the need's
/// neighbours, `samples` evenly spaced on each side, and powers of two away
/// from the need on each side, all within `[lo, hi]`.
fn checks(lo: u64, need: u64, hi: u64, samples: u64) -> Vec<u64> {
    let mut v = vec![need, need.saturating_add(1).min(hi)];
    if need > lo {
        v.push(need - 1);
    }
    let mut step = 2_u64;
    let within = |x: Option<u64>| x.is_some_and(|x| (lo..=hi).contains(&x));
    while step > 0 && (within(need.checked_sub(step)) || within(need.checked_add(step))) {
        if let Some(below) = need.checked_sub(step).filter(|&x| x >= lo) {
            v.push(below);
        }
        if let Some(above) = need.checked_add(step).filter(|&x| x <= hi) {
            v.push(above);
        }
        step = step.checked_mul(2).unwrap_or(0);
    }
    for k in 1..=samples {
        if need > lo {
            v.push(between(lo, need, k, samples));
        }
        v.push(between(need, hi, k, samples));
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The `k`th of `samples` evenly spaced values in `[a, b]`, in arithmetic
/// wide enough for any `u64` span.
fn between(a: u64, b: u64, k: u64, samples: u64) -> u64 {
    let span = u128::from(b - a) * u128::from(k) / (u128::from(samples) + 1);
    // At most `b - a`, so it fits.
    a.saturating_add(u64::try_from(span).unwrap_or(b - a))
}

/// The least value in `[lo, hi]` that `refused` answers `false` for, by
/// bracketing (`hi` must be clear) and bisection, then checked at the
/// need's adjacent values, at `samples` spaced values on each side and at
/// powers of two away from it.
///
/// # Errors
/// A [`Counterexample`]: `hi` refused, or a value whose answer is not the
/// need's side's.
pub fn least(
    lo: u64,
    hi: u64,
    samples: u64,
    mut refused: impl FnMut(u64) -> bool,
) -> Result<Least, Counterexample> {
    let mut tried = BTreeMap::new();
    let mut ask =
        |v: u64, tried: &mut BTreeMap<u64, bool>| *tried.entry(v).or_insert_with(|| refused(v));
    if ask(hi, &mut tried) {
        return Err(Counterexample {
            value: hi,
            refused: true,
            need: None,
        });
    }
    let need = if ask(lo, &mut tried) {
        let (mut bad, mut good) = (lo, hi);
        while good - bad > 1 {
            let mid = bad + (good - bad) / 2;
            if ask(mid, &mut tried) {
                bad = mid;
            } else {
                good = mid;
            }
        }
        good
    } else {
        lo
    };
    for v in checks(lo, need, hi, samples) {
        ask(v, &mut tried);
    }
    if let Some((&value, &r)) = tried.iter().find(|&(&v, &r)| r != (v < need)) {
        return Err(Counterexample {
            value,
            refused: r,
            need: Some(need),
        });
    }
    Ok(Least { need, tried })
}

#[cfg(test)]
mod tests {
    use super::{Counterexample, least};

    #[test]
    fn the_need_is_the_least_clear_value_and_its_neighbours_are_tried() {
        let found = least(0, 1 << 20, 3, |v| v < 777).unwrap();
        assert_eq!(found.need, 777);
        assert_eq!(found.tried.get(&776), Some(&true));
        assert_eq!(found.tried.get(&777), Some(&false));
        assert_eq!(found.tried.get(&778), Some(&false));
        assert!(found.tried.len() < 80, "{}", found.tried.len());
    }

    #[test]
    fn a_need_of_zero_is_tested_at_zero_alone() {
        let found = least(0, 100, 2, |_| false).unwrap();
        assert_eq!(found.need, 0);
        assert!(found.tried.keys().all(|&v| v <= 100));
    }

    /// A predicate clear in a window below its true need, which bisection
    /// alone can step over: a sampled value finds it.
    #[test]
    fn a_window_below_the_need_is_a_counterexample() {
        let pred = |v: u64| !((250..=260).contains(&v) || v >= 1000);
        let e = least(0, 1 << 12, 7, pred).unwrap_err();
        assert!(e.value < 1000 && !e.refused, "{e:?}");
        assert!(e.to_string().contains("spec ruling"), "{e}");
    }

    /// The full `u64` range: the sampling's arithmetic never overflows.
    #[test]
    fn the_whole_range_is_searched_without_overflow() {
        let l = least(0, u64::MAX, 4, |v| v < 777).unwrap();
        assert_eq!(l.need, 777);
        assert!(l.tried.contains_key(&u64::MAX));
        assert!(l.tried.keys().any(|&v| v > u64::MAX / 2 && v < u64::MAX));
        let top = least(u64::MAX - 3, u64::MAX, 4, |v| v < u64::MAX).unwrap();
        assert_eq!(top.need, u64::MAX);
    }

    #[test]
    fn a_refused_top_is_no_bracket() {
        assert_eq!(
            least(0, 10, 1, |_| true),
            Err(Counterexample {
                value: 10,
                refused: true,
                need: None
            })
        );
    }
}
