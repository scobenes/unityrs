use super::*;
use crate::store::scan::CodeScan;

fn code(name: &str) -> Code {
    let mut scan = CodeScan::default();
    scan.feed(format!("<Error><Code>{name}</Code></Error>").as_bytes());
    scan.code()
}

fn facts(status: Option<u16>, name: &str) -> Facts {
    Facts {
        connected: true,
        status,
        code: code(name),
    }
}

const RETRY: Action = Action::Retry { retry_after: true };
const PLAIN: Action = Action::Retry { retry_after: false };
const ALL: [Kind; 11] = [
    Kind::Put,
    Kind::Part,
    Kind::Create,
    Kind::Complete,
    Kind::Abort,
    Kind::ListProof,
    Kind::ListPage,
    Kind::HeadRead,
    Kind::HeadWrite { if_match: false },
    Kind::HeadWrite { if_match: true },
    Kind::Head,
];

fn v(settled: bool, action: Action) -> Verdict {
    Verdict { settled, action }
}

#[test]
fn row_one_nothing_sent_before_the_connection_was_up() {
    for kind in ALL {
        for ended in [Ended::Error, Ended::TimedOut] {
            let f = Facts {
                connected: false,
                ..Facts::default()
            };
            assert_eq!(classify(kind, false, &f, ended), v(true, PLAIN), "{kind:?}");
        }
    }
}

#[test]
fn row_two_no_response_otherwise() {
    for kind in ALL {
        for ended in [Ended::Error, Ended::TimedOut, Ended::Rejected] {
            let f = facts(None, "");
            assert_eq!(classify(kind, true, &f, ended), v(false, PLAIN), "{kind:?}");
        }
    }
}

#[test]
fn row_three_a_5xx_or_a_2xx_the_call_returns_as_an_error() {
    for kind in ALL {
        for status in [500, 503, 599] {
            for ended in [Ended::Ok, Ended::Error, Ended::TimedOut] {
                let f = facts(Some(status), "ExpiredToken");
                assert_eq!(classify(kind, false, &f, ended), v(false, RETRY));
            }
        }
        if kind != Kind::ListProof {
            for ended in [Ended::Error, Ended::TimedOut, Ended::Rejected] {
                assert_eq!(
                    classify(kind, false, &facts(Some(200), ""), ended),
                    v(false, RETRY)
                );
            }
        }
    }
}

#[test]
fn row_four_a_2xx() {
    for kind in ALL {
        for status in [200, 201, 204, 206, 299] {
            assert_eq!(
                classify(kind, false, &facts(Some(status), ""), Ended::Ok),
                v(true, Action::Success)
            );
        }
    }
}

#[test]
fn row_five_a_408_or_a_429() {
    for kind in ALL {
        for status in [408, 429] {
            // Before a head write's rows and ExpiredToken's.
            let f = facts(Some(status), "ExpiredToken");
            assert_eq!(classify(kind, true, &f, Ended::Error), v(true, RETRY));
        }
    }
}

#[test]
fn row_six_a_head_writes_failed_precondition() {
    for if_match in [false, true] {
        for status in [304, 409, 412] {
            let f = facts(Some(status), "ExpiredToken");
            let kind = Kind::HeadWrite { if_match };
            assert_eq!(
                classify(kind, false, &f, Ended::Error),
                v(true, Action::ReadBack)
            );
        }
    }
    let f = facts(Some(404), "NoSuchKey");
    assert_eq!(
        classify(Kind::HeadWrite { if_match: true }, false, &f, Ended::Error),
        v(true, Action::ReadBack)
    );
    // A `Create` write's 404 is not a precondition.
    assert_eq!(
        classify(Kind::HeadWrite { if_match: false }, false, &f, Ended::Error),
        v(true, Action::PublishFailed)
    );
    // The same statuses elsewhere are any other 3xx or 4xx.
    for kind in [Kind::Put, Kind::Part, Kind::Complete, Kind::ListPage] {
        for status in [304, 409, 412] {
            assert_eq!(
                classify(kind, false, &facts(Some(status), ""), Ended::Error),
                v(true, Action::PublishFailed)
            );
        }
    }
}

#[test]
fn row_seven_a_heads_read_finds_none() {
    let f = facts(Some(404), "ExpiredToken");
    assert_eq!(
        classify(Kind::HeadRead, false, &f, Ended::Error),
        v(true, Action::NoHead)
    );
    assert_eq!(
        classify(Kind::Put, false, &facts(Some(404), ""), Ended::Error),
        v(true, Action::PublishFailed)
    );
}

#[test]
fn row_eight_no_such_upload_on_a_retried_complete() {
    let f = facts(Some(404), "NoSuchUpload");
    assert_eq!(
        classify(Kind::Complete, true, &f, Ended::Error),
        v(true, Action::HeadCheck)
    );
    // The first try's is any other 4xx, and so is another kind's.
    assert_eq!(
        classify(Kind::Complete, false, &f, Ended::Error),
        v(true, Action::PublishFailed)
    );
    assert_eq!(
        classify(Kind::Part, true, &f, Ended::Error),
        v(true, Action::PublishFailed)
    );
    // An abort's NoSuchUpload is a successful abort.
    assert_eq!(
        classify(Kind::Abort, false, &f, Ended::Error),
        v(true, Action::Success)
    );
    assert_eq!(
        classify(
            Kind::Abort,
            false,
            &facts(Some(404), "NoSuchKey"),
            Ended::Error
        ),
        v(true, Action::PublishFailed)
    );
}

#[test]
fn row_nine_expired_token() {
    for kind in ALL {
        for status in [400, 403] {
            let f = facts(Some(status), "ExpiredToken");
            assert_eq!(
                classify(kind, true, &f, Ended::Error),
                v(true, Action::CredentialsExpired)
            );
        }
    }
    // A code that only begins so names something else.
    assert_eq!(
        classify(
            Kind::Put,
            false,
            &facts(Some(400), "ExpiredTokenX"),
            Ended::Error
        ),
        v(true, Action::PublishFailed)
    );
}

#[test]
fn row_ten_any_other_3xx_or_4xx() {
    for kind in ALL {
        for status in [300, 301, 307, 308, 400, 401, 403, 405, 411, 416, 451, 499] {
            if kind == Kind::Abort {
                continue;
            }
            assert_eq!(
                classify(
                    kind,
                    false,
                    &facts(Some(status), "AccessDenied"),
                    Ended::Error
                ),
                v(true, Action::PublishFailed)
            );
        }
    }
}

#[test]
fn row_eleven_anything_else() {
    for kind in ALL {
        for status in [100, 101, 199, 600, 999] {
            assert_eq!(
                classify(kind, false, &facts(Some(status), ""), Ended::Error),
                v(false, Action::Unsettled)
            );
        }
    }
}

#[test]
fn the_proof_takes_any_2xx_whatever_its_body() {
    for ended in [Ended::Ok, Ended::Error, Ended::Rejected, Ended::TimedOut] {
        assert_eq!(
            classify(Kind::ListProof, false, &facts(Some(200), ""), ended),
            v(true, Action::Success)
        );
    }
    assert_eq!(
        classify(
            Kind::ListProof,
            false,
            &facts(Some(404), "NoSuchBucket"),
            Ended::Error
        ),
        v(true, Action::PublishFailed)
    );
    assert!(v(false, RETRY).retried());
    assert!(!v(true, Action::Success).retried());
}
