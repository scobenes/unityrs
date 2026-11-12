//! CR8's counters (`unityrs_core::testing`; `docs/plans/batch.md` section
//! 6, CR8) on the provider's total ([`TpkProvider::MAX_TOTAL_ROWS`]): a
//! root past it is refused when its walk is asked for, counted then and
//! once, as the core counts a kept failure. Built under this crate's
//! `test-counters` feature, which turns the core's on. The counters are the
//! process's, so this binary holds one test and nothing else refuses.

#![cfg(feature = "test-counters")]

use unityrs_core::LimitKind;
use unityrs_core::limits::Limits;
use unityrs_core::testing;
use unityrs_tpk::TpkProvider;

use common::{Blob, V2, limit_of, load, v2};

mod common;

/// The `Count` refusals `f` makes.
fn counted(f: impl FnOnce()) -> u64 {
    let before = testing::refusals();
    f();
    testing::refusals().since(&before).limit(LimitKind::Count)
}

/// Round 4: the provider built each root's total refusal at load, so a
/// load counted one for every root past the total, asked for or not.
#[test]
fn a_root_past_the_total_counts_its_refusal_when_first_asked() {
    // Nodes 0-16 a doubling chain, whose tree passes the row bound, and a
    // node listing node 0 for each of 33 roots: each walk takes one row
    // past the row bound, so the first 31 fit the total and the last two
    // are past it.
    let mut nodes: Vec<Vec<u16>> = (0..17_u16)
        .map(|i| if i < 16 { vec![i + 1; 2] } else { vec![] })
        .collect();
    let first = u16::try_from(nodes.len()).unwrap();
    nodes.extend((0..33).map(|_| vec![0]));
    let blob = Blob {
        versions: vec![V2],
        classes: (0..33_i32)
            .map(|id| (id, vec![(V2, first + u16::try_from(id).unwrap())]))
            .collect(),
        nodes,
        strings: 2,
        ..Blob::default()
    };
    let mut provider = None;
    assert_eq!(
        counted(|| provider = Some(load(&blob, &Limits::default()).unwrap())),
        0,
        "a load counts nothing"
    );
    let provider = provider.unwrap();
    let mut e = None;
    assert_eq!(
        counted(|| e = Some(provider.node_list(32, &v2()).unwrap_err())),
        1,
        "the first ask counts its refusal"
    );
    let first_ask = limit_of(&e.take().unwrap());
    assert_eq!(
        (first_ask.kind(), first_ask.max()),
        (
            LimitKind::Count,
            u64::try_from(TpkProvider::MAX_TOTAL_ROWS).unwrap()
        )
    );
    assert_eq!(
        counted(|| e = Some(provider.node_list(32, &v2()).unwrap_err())),
        0,
        "a second ask copies the kept refusal"
    );
    assert_eq!(limit_of(&e.take().unwrap()), first_ask);
}
