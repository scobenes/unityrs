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

#[test]
fn a_registered_wake_can_be_waited_on_again_without_spinning() {
    use std::{sync::Arc, time::Duration};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let wake = Arc::new(super::Wake::new().unwrap());
    let registered = super::RegisteredWake::new(&wake, runtime.handle()).unwrap();
    runtime.block_on(async {
        for _ in 0..2 {
            wake.notify().unwrap();
            tokio::time::timeout(unityrs_testkit::guard::DEADLOCK_GUARD, registered.wait())
                .await
                .unwrap()
                .unwrap();
            // Drained and cleared: with no new notify the next wait pends.
            assert!(
                tokio::time::timeout(Duration::from_millis(50), registered.wait())
                    .await
                    .is_err()
            );
        }
        // Two notifies before one wait coalesce into that one wake.
        wake.notify().unwrap();
        wake.notify().unwrap();
        registered.wait().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), registered.wait())
                .await
                .is_err()
        );
    });
}
