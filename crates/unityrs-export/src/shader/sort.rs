//! Allocation-free sorting with an explicit bound on heap comparisons and moves.
use super::Error;
use unityrs_core::limits::Meter;

pub(super) fn by_key<T, K: Ord>(
    values: &mut [T],
    key: impl Fn(&T) -> K,
    meter: &Meter,
) -> Result<(), Error> {
    if values.len() < 2 {
        return Ok(());
    }
    // At most n build sifts and n removal sifts, each visiting at most
    // floor(log2(n)) + 1 levels; sixteen element touches per pair of levels
    // covers child/root comparisons and swaps. Charge before touching values.
    let n = values.len() as u64;
    meter.count_decode(
        n.saturating_mul(u64::from(n.ilog2()) + 1)
            .saturating_mul(16),
    )?;
    for start in (0..values.len() / 2).rev() {
        sift(values, start, &key);
    }
    for end in (1..values.len()).rev() {
        values.swap(0, end);
        if let Some(heap) = values.get_mut(..end) {
            sift(heap, 0, &key);
        }
    }
    Ok(())
}
fn sift<T, K: Ord>(values: &mut [T], mut root: usize, key: &impl Fn(&T) -> K) {
    while let Some(left) = root.checked_mul(2).and_then(|n| n.checked_add(1)) {
        let Some(child) = values.get(left) else { break };
        let largest = match values.get(left + 1) {
            Some(right) if key(child) < key(right) => left + 1,
            _ => left,
        };
        let Some(parent) = values.get(root) else {
            break;
        };
        let Some(child) = values.get(largest) else {
            break;
        };
        if key(parent) >= key(child) {
            break;
        }
        values.swap(root, largest);
        root = largest;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use unityrs_core::budget::{Reservation, Unbounded};
    #[test]
    fn sort_refuses_before_comparisons_and_accepts_exact_bound() {
        for work in [0, 95, 96] {
            let meter =
                Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input);
            let mut values = [3, 1, 2];
            let answer = by_key(&mut values, |n| *n, &meter);
            if work < 96 {
                assert!(matches!(answer, Err(Error::Charge(_))));
                assert_eq!(values, [3, 1, 2]);
                assert_eq!(meter.decoded(), 0);
            } else {
                answer.unwrap();
                assert_eq!(values, [1, 2, 3]);
                assert_eq!(meter.decoded(), 96);
            }
        }
    }
    #[test]
    fn heap_sort_handles_duplicates_and_both_input_orders() {
        for n in 0..128 {
            let mut values: Vec<_> = (0..n).rev().map(|n| n % 17).collect();
            let mut expected = values.clone();
            expected.sort_unstable();
            by_key(&mut values, |n| *n, &Meter::unlimited()).unwrap();
            assert_eq!(values, expected);
            by_key(&mut values, |n| *n, &Meter::unlimited()).unwrap();
            assert_eq!(values, expected);
        }
    }
}
