//! `copy_out`'s rule on a buffer of its own: which slices it copies, that it
//! copies each byte once, where each slice lands in the copy, and that a
//! refused copy changes nothing (module docs).

use std::sync::Arc;

use bytes::Bytes;

use super::{Slice, Slices, copy_out, merge, place};
use crate::budget::{OWNER_COST, Reservation, Unbounded};
use crate::limits::{Meter, heap_cost};

/// Slices, visited in order.
struct Many(Vec<Bytes>);

impl Slices for Many {
    fn each(&mut self, f: &mut dyn FnMut(Slice<'_>)) {
        for b in &mut self.0 {
            f(Slice::Bytes(b));
        }
    }
}

/// 1,000 bytes, no two neighbours equal.
fn whole() -> Bytes {
    Bytes::from(
        (0..1000_u32)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect::<Vec<_>>(),
    )
}

/// Slices of `whole()[100..900]` (`within`) and around it: two that overlap
/// (100..110, 105..120), the first again, one of its own (300..310) and one
/// that touches it (310..312), then one before `within`, one that crosses
/// its end, one after it, and one of another buffer.
fn slices(whole: &Bytes) -> Many {
    Many(vec![
        whole.slice(100..110),
        whole.slice(105..120),
        whole.slice(100..110),
        whole.slice(300..310),
        whole.slice(310..312),
        whole.slice(50..60),
        whole.slice(895..905),
        whole.slice(950..960),
        Bytes::from_static(b"elsewhere"),
    ])
}

/// The distinct bytes the slices in `within` name are 100..120 and
/// 300..312: 32, copied once, in one buffer, in the order they lie. Slices
/// that were one buffer are one buffer of the copy, slices that overlapped
/// overlap as they did, and the slices that are not wholly in `within` stay
/// where they were.
#[test]
fn each_byte_is_copied_once_and_each_slice_keeps_its_place() {
    let whole = whole();
    let within = whole.slice(100..900);
    let mut m = slices(&whole);
    let before: Vec<usize> = m.0.iter().map(|b| b.as_ptr().addr()).collect();
    let contents = m.0.clone();
    let meter = Meter::unlimited();
    assert_eq!(copy_out(&mut m, &within, 4 << 20, &meter), 32);
    assert_eq!(m.0, contents, "every slice reads as it did");
    let at: Vec<usize> = m.0.iter().map(|b| b.as_ptr().addr()).collect();
    let run = whole.as_ptr().addr()..whole.as_ptr().addr() + whole.len();
    assert!(!run.contains(&at[0]), "copied");
    assert_eq!(at[1], at[0] + 5, "an overlap stays an overlap");
    assert_eq!(at[2], at[0], "one buffer stays one buffer");
    assert_eq!(at[3], at[0] + 20, "the next bytes follow in the copy");
    assert_eq!(at[4], at[3] + 10, "a touching slice follows on");
    assert_eq!(&at[5..], &before[5..], "the others are not copied");
    // The copy and its owner record, and nothing else, stay charged.
    assert_eq!(meter.used(), heap_cost(32) + OWNER_COST);
}

/// A meter that refuses the table of slices, or grants it but refuses the
/// copy's room or its owner record, leaves every slice where it was and
/// nothing charged.
#[test]
fn a_refused_copy_changes_nothing() {
    let whole = whole();
    let within = whole.slice(100..900);
    // Five slices lie in `within`: a table of 80 bytes, held while the
    // copy's 32 bytes and then its owner record are charged.
    let all = heap_cost(80) + heap_cost(32) + OWNER_COST;
    for limit in [heap_cost(80) - 1, all - OWNER_COST - 1, all - 1] {
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        let mut m = slices(&whole);
        let before: Vec<usize> = m.0.iter().map(|b| b.as_ptr().addr()).collect();
        assert_eq!(copy_out(&mut m, &within, 4 << 20, &meter), 0, "{limit}");
        let after: Vec<usize> = m.0.iter().map(|b| b.as_ptr().addr()).collect();
        assert_eq!(after, before, "{limit}");
        assert_eq!(meter.used(), 0, "{limit}");
    }
    // With room for all three it copies.
    let meter = Meter::new(all, Arc::new(Unbounded), Reservation::Input);
    assert_eq!(copy_out(&mut slices(&whole), &within, 4 << 20, &meter), 32);
}

/// A slice is placed only when it is not empty and lies wholly in `within`.
#[test]
fn a_slice_is_placed_only_when_it_lies_in_within() {
    let whole = whole();
    let within = whole.slice(100..900);
    assert_eq!(place(&within, &whole[100..110]), Some((0, 10)));
    assert_eq!(place(&within, &whole[890..900]), Some((790, 800)));
    assert_eq!(place(&within, &whole[300..300]), None, "empty");
    assert_eq!(place(&within, &whole[95..105]), None, "starts before");
    assert_eq!(place(&within, &whole[895..905]), None, "ends after");
    assert_eq!(place(&within, &whole[900..910]), None, "after");
}

/// Spans are merged where they overlap or touch, a span inside another
/// adding nothing, and kept apart where a byte lies between them.
#[test]
fn spans_merge_where_they_overlap_or_touch() {
    let mut spans = [(20, 30), (0, 10), (10, 12), (2, 4), (21, 25), (13, 14)];
    let kept = merge(&mut spans);
    assert_eq!(&spans[..kept], &[(0, 12), (13, 14), (20, 30)]);
}
