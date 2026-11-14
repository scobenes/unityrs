use super::*;
#[test]
fn every_valid_word_roundtrips_and_impossible_words_refuse() {
    for reason in [
        StopReason::Deadline,
        StopReason::Sigterm,
        StopReason::Sigint,
        StopReason::Sighup,
        StopReason::ParentDied,
        StopReason::StdinEof,
        StopReason::SpoolFull,
        StopReason::HostIo,
        StopReason::HostMemory,
        StopReason::Internal,
    ] {
        for culprit in [
            Culprit::None,
            Culprit::Input(0),
            Culprit::Input(u32::MAX),
            Culprit::Bundle(0),
            Culprit::Bundle(u32::MAX),
        ] {
            for detail in [
                None,
                Some(SpoolDetail::SizeBytes),
                Some(SpoolDetail::Reserve),
                Some(SpoolDetail::Enospc),
                Some(SpoolDetail::Edquot),
            ] {
                let stop = Stop {
                    reason,
                    detail,
                    culprit,
                };
                let valid = (reason == StopReason::SpoolFull) == detail.is_some()
                    && (!matches!(
                        reason,
                        StopReason::Deadline
                            | StopReason::Sigterm
                            | StopReason::Sigint
                            | StopReason::Sighup
                            | StopReason::ParentDied
                            | StopReason::StdinEof
                    ) || culprit == Culprit::None);
                assert_eq!(stop.pack().is_some(), valid);
                if let Some(word) = stop.pack() {
                    assert_eq!(Stop::unpack(word), Some(stop));
                    assert_eq!(Stop::unpack(word | TAKEN), Some(stop));
                    assert_eq!(Stop::unpack(word | 1 << 63), None);
                }
            }
        }
    }
    for word in [
        0,
        11,
        15,
        1 | 1 << 4,
        7,
        0xa | 3 << 7,
        0xa | 1 << 9,
        7 | 5 << 4,
    ] {
        assert_eq!(Stop::unpack(word), None);
    }
}
#[test]
fn reduction_keeps_first_equal_tuple_and_take_never_changes() {
    let stops = [
        Stop {
            reason: StopReason::SpoolFull,
            detail: Some(SpoolDetail::Reserve),
            culprit: Culprit::Input(1),
        },
        Stop::external(StopReason::Deadline),
        Stop::external(StopReason::HostMemory),
        Stop::external(StopReason::Internal),
    ];
    for first in stops {
        for second in stops {
            let state = State::new();
            state.request(first);
            state.request(second);
            let expected = if first.reason.code().severity() <= second.reason.code().severity() {
                first
            } else {
                second
            };
            assert_eq!(state.live(), Some(expected));
        }
    }
    let state = State::new();
    state.request(stops[0]);
    let captured = state.take();
    state.request(stops[3]);
    assert_eq!(captured, Some(stops[0]));
    assert_eq!(state.take(), None);
    assert_eq!(state.end_code(None), Code::Internal);
    state.store_final(Code::CouldNotStart);
    assert_eq!(
        state.end_code(Some(StopReason::Sigint)),
        Code::CouldNotStart
    );
    assert_eq!(state.arm(Boot(20)), Boot(20));
    assert_eq!(state.arm(Boot(30)), Boot(20));
    assert!(!state.second_signal());
    assert!(state.second_signal());
}
#[test]
fn concurrent_requests_cannot_tear_reason_detail_or_culprit() {
    let state = State::new();
    std::thread::scope(|scope| {
        for i in 0..64 {
            let state = &state;
            scope.spawn(move || {
                state.request(Stop {
                    reason: StopReason::SpoolFull,
                    detail: Some(if i % 2 == 0 {
                        SpoolDetail::Enospc
                    } else {
                        SpoolDetail::Edquot
                    }),
                    culprit: Culprit::Input(i),
                });
            });
        }
    });
    let stop = state.take().unwrap();
    let Culprit::Input(i) = stop.culprit else {
        panic!("wrong culprit")
    };
    assert_eq!(
        stop.detail,
        Some(if i % 2 == 0 {
            SpoolDetail::Enospc
        } else {
            SpoolDetail::Edquot
        })
    );
}
