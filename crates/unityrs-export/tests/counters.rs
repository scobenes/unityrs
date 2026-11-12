//! CR8's counters (`unityrs_core::testing`; `docs/plans/batch.md` section
//! 6, CR8) on the NGUI register's one refusal of its own: bytes in hand
//! past [`unityrs_core::limits::Limits::max_input_bytes`], refused before
//! they are charged (`ngui/register.rs`, step 2), counted once, through
//! `LimitError::refused`. Built under this crate's `test-counters` feature,
//! which turns the core's on. The counters are the process's, so this
//! binary holds one test and nothing else refuses.

#![cfg(feature = "test-counters")]

use std::sync::Arc;

use unityrs_core::limits::Meter;
use unityrs_core::testing;
use unityrs_core::{Bytes, FileContext, LimitKind, RawStr};
use unityrs_export::ngui::{self, Data, Failure, NoHooks};

/// Round 4: the refusal was built through the counted constructor by
/// reading alone, and no test ran export under the feature.
#[test]
fn bytes_past_the_input_bound_are_refused_and_counted_once() {
    let mut context = FileContext::default();
    context.limits.max_input_bytes = 8;
    let context = Arc::new(context);
    let data = Bytes::from(vec![0_u8; 9]);
    let before = testing::refusals();
    let got = ngui::peek(
        &RawStr::from("x"),
        Data::Bytes(&data),
        &context,
        &Meter::unlimited(),
        &NoHooks,
    );
    let counted = testing::refusals().since(&before);
    assert!(matches!(got, Err(Failure::Input(_))), "{got:?}");
    assert_eq!(counted.limit(LimitKind::InputBytes), 1);
    assert_eq!(counted.total(), 1, "nothing else refused: {counted:?}");
}
