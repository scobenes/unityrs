//! `testing`'s unit tests: the counter indexes and a table's counting.

use super::*;

/// Every kind and every reservation has a counter of its own.
#[test]
fn every_kind_has_its_own_counter() {
    let kinds = [
        LimitKind::InputMemory,
        LimitKind::InputBytes,
        LimitKind::BlockOutput,
        LimitKind::DecodeWork,
        LimitKind::Inflate,
        LimitKind::ContainerDepth,
        LimitKind::TreeDepth,
        LimitKind::ValueDepth,
        LimitKind::ValuesPerObject,
        LimitKind::ZeroSizeElements,
        LimitKind::Count,
    ];
    let mut seen: Vec<usize> = kinds.iter().map(|k| kind_index(*k)).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..LIMIT_KINDS).collect::<Vec<_>>());
    let whats = [
        Reservation::Input,
        Reservation::Dependency,
        Reservation::Provider,
    ];
    let mut seen: Vec<usize> = whats.iter().map(|w| reservation_index(*w)).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..RESERVATIONS).collect::<Vec<_>>());
}

/// A table counts by load, path and whole, and refuses what does not
/// fit without allocating.
#[test]
fn a_table_counts_by_key_and_refuses_what_does_not_fit() {
    let mut t = Tables::with_room(2, 3, 2).unwrap();
    let top = Key::Top(LoadId(7));
    assert!(t.count(&top, SpanRow::row(1)));
    assert!(t.count(&top, SpanRow::row(1)));
    assert!(t.count(&Key::Top(LoadId(8)), SpanRow::row(1)));
    assert!(t.count(&top, SpanRow::WHOLE));
    // Full: a fourth key does not fit, a counted one still does.
    assert!(!t.count(&top, SpanRow::row(2)));
    assert!(t.count(&top, SpanRow::row(1)));
    // An untracked bundle records nothing and is no overflow.
    assert!(t.count(&Key::Untracked, SpanRow::row(0)));
    let counts: Vec<_> = t
        .slots
        .iter()
        .map(|s| (s.load, s.len, s.whole, s.count))
        .collect();
    assert_eq!(
        counts,
        [
            (LoadId(7), 1, false, 3),
            (LoadId(8), 1, false, 1),
            (LoadId(7), 0, true, 1)
        ]
    );
    assert_eq!(t.paths.get(..4), Some(&[1, 0, 1, 0][..]));
    // Deeper than the width: a row needs one index, a whole read none.
    let mut narrow = Tables::with_room(0, 2, 0).unwrap();
    assert!(!narrow.count(&top, SpanRow::row(0)));
    assert!(narrow.count(&top, SpanRow::WHOLE));
    // A nested key's path, from its nodes: `[3, 1]`, then its row.
    let inner = Arc::new(Ancestry { up: None, row: 3 });
    let deeper = Key::Nested {
        load: LoadId(7),
        node: Arc::new(Ancestry {
            up: Some(inner),
            row: 1,
        }),
    };
    let mut nest = Tables::with_room(0, 2, 3).unwrap();
    assert!(nest.count(&deeper, SpanRow::row(2)));
    assert!(nest.count(&deeper, SpanRow::WHOLE));
    assert_eq!(nest.paths.get(..6), Some(&[3, 1, 2, 3, 1, 0][..]));
    assert!(
        !Tables::with_room(0, 2, 2)
            .unwrap()
            .count(&deeper, SpanRow::row(2))
    );
    assert!(t.name(7, b"a"));
    assert!(!t.name(8, &[b'x'; NAME_BYTES + 1]));
    assert!(t.name(8, b"b"));
    assert!(!t.name(9, b"c"));
}
