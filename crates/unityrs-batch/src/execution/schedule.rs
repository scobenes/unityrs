//! Deterministic blocking-completion schedule for allocation sweeps only.
use std::sync::atomic::{AtomicBool, Ordering};

static DEFER: AtomicBool = AtomicBool::new(false);

/// Make each store payload task yield before its result can be observed.
/// Install before starting a sweep child; no clock or worker speed selects
/// whether a transfer continues in its admission poll.
pub fn defer_blocking_completions() {
    DEFER.store(true, Ordering::SeqCst);
}

pub(crate) async fn blocking_ready() {
    yield_once(DEFER.load(Ordering::SeqCst)).await;
}

fn yield_once(mut enabled: bool) -> impl std::future::Future<Output = ()> {
    std::future::poll_fn(move |cx| {
        if std::mem::take(&mut enabled) {
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };

    #[derive(Default)]
    struct Wakes(std::sync::atomic::AtomicUsize);
    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_ready_completion_is_deferred_once_and_schedules_its_next_poll() {
        let counter = Arc::new(Wakes::default());
        let waker = Waker::from(counter.clone());
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(yield_once(true));
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(()));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_unselected_schedule_adds_no_pending_poll() {
        let mut cx = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(yield_once(false));
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(()));
    }
}
