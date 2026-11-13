//! Drop ordering without a runtime, operating system socket, or timer.
use super::{Registration, SocketTable};
use crate::http::Error;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct Owner(Arc<AtomicBool>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl tokio::io::AsyncWrite for Owner {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        panic!("registered sockets must close without a graceful half-close");
    }
}
struct Table {
    closed: Arc<AtomicBool>,
    removed: AtomicUsize,
}
impl SocketTable for Table {
    fn register(&self, _: std::os::fd::RawFd) -> Result<usize, Error> {
        assert!(!self.closed.load(Ordering::Acquire));
        Ok(0)
    }
    fn closing(&self) -> bool {
        false
    }
    fn remove(&self, entry: usize) {
        assert_eq!(entry, 0);
        assert!(self.closed.load(Ordering::Acquire), "close before removal");
        self.removed.fetch_add(1, Ordering::AcqRel);
    }
}
#[test]
fn owner_closes_before_registration_is_removed_exactly_once() {
    let closed = Arc::new(AtomicBool::new(false));
    let table = Arc::new(Table {
        closed: closed.clone(),
        removed: AtomicUsize::new(0),
    });
    let adapter: Arc<dyn SocketTable> = table.clone();
    let registration = Registration::new(&adapter, 0).unwrap();
    registration.check().unwrap();
    let owner = registration.wrap(Owner(closed.clone()));
    assert!(!closed.load(Ordering::Acquire));
    drop(owner);
    assert_eq!(table.removed.load(Ordering::Acquire), 1);
}

#[test]
fn shutdown_closes_before_removal_and_repeated_shutdown_and_drop_are_idempotent() {
    use tokio::io::AsyncWrite;
    let closed = Arc::new(AtomicBool::new(false));
    let table = Arc::new(Table {
        closed: closed.clone(),
        removed: AtomicUsize::new(0),
    });
    let adapter: Arc<dyn SocketTable> = table.clone();
    let mut owner = Registration::new(&adapter, 0)
        .unwrap()
        .wrap(Owner(closed.clone()));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    for _ in 0..2 {
        assert!(matches!(
            std::pin::Pin::new(&mut owner).poll_shutdown(&mut cx),
            std::task::Poll::Ready(Ok(()))
        ));
        assert!(closed.load(Ordering::Acquire));
        assert_eq!(table.removed.load(Ordering::Acquire), 1);
    }
    drop(owner);
    assert_eq!(table.removed.load(Ordering::Acquire), 1);
}
