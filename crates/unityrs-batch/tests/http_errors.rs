//! Closed download detail/retry mappings; diagnostics contain fixed facts only.
use unityrs_batch::http::{Detail, Error, Failure, RetryClass};
#[test]
fn every_detail_and_status_has_the_contracts_retry_class() {
    for (detail, retry) in [
        (Detail::Connect, Some(RetryClass::Connect)),
        (Detail::Tls, Some(RetryClass::Connect)),
        (Detail::HeadTooLarge, None),
        (Detail::NoResponse, Some(RetryClass::NoResponse)),
        (Detail::Timeout, Some(RetryClass::Timeout)),
        (Detail::Truncated, Some(RetryClass::Truncated)),
        (Detail::Status(408), Some(RetryClass::Timeout)),
        (Detail::Status(429), Some(RetryClass::Status429)),
        (Detail::Status(500), Some(RetryClass::Status5xx)),
        (Detail::Status(599), Some(RetryClass::Status5xx)),
        (Detail::Status(101), None),
        (Detail::Status(206), None),
        (Detail::Status(301), None),
        (Detail::Status(404), None),
    ] {
        assert_eq!(detail.retry_class(), retry, "{detail:?}");
    }
    for error in [
        Error::Memory,
        Error::Busy,
        Error::Pool,
        Error::InvalidRequest,
    ] {
        assert_eq!(error.detail(), None);
    }
    let failure = Error::Transport(Failure {
        detail: Detail::Connect,
        errno: Some(111),
    });
    assert_eq!(failure.detail(), Some(Detail::Connect));
    assert_eq!(
        format!("{failure:?}"),
        "Transport(Failure { detail: Connect, errno: Some(111) })"
    );
}
