use super::{paths::INTERRUPTED_RETRIES, retry_interrupted};
use std::io;

#[test]
fn an_interrupted_call_is_retried_a_bounded_number_of_times() {
    let mut calls = 0;
    let result: io::Result<()> = retry_interrupted(|| {
        calls += 1;
        Err(io::Error::from(io::ErrorKind::Interrupted))
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert_eq!(calls, INTERRUPTED_RETRIES + 1);

    let mut calls = 0;
    let result = retry_interrupted(|| {
        calls += 1;
        if calls < 3 {
            Err(io::Error::from(io::ErrorKind::Interrupted))
        } else {
            Ok(calls)
        }
    });
    assert_eq!(result.unwrap(), 3);

    let mut calls = 0;
    let result: io::Result<()> = retry_interrupted(|| {
        calls += 1;
        Err(io::Error::from(io::ErrorKind::NotFound))
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
    assert_eq!(calls, 1);
}
