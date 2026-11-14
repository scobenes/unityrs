//! What B10c's HTTP runtime needs: both of Tokio's IO and time drivers.
use crate::http::{self, Error, Service, permit::PermitPool, socket::SocketTable};
use std::{os::fd::RawFd, sync::Arc, time::Duration};

struct NoSockets;
impl SocketTable for NoSockets {
    fn register(&self, _: RawFd) -> Result<usize, Error> {
        unreachable!("no IO driver")
    }
    fn remove(&self, _: usize) {
        unreachable!("no socket registered")
    }
    fn closing(&self) -> bool {
        false
    }
}

// The integration precondition: `Execution` runs no Tokio runtime (its
// pool is the engine's own reserved threads), so B10c must build HTTP's
// runtime with the IO and time drivers. These guards show that a runtime
// without them cannot poll a request.
#[test]
#[should_panic(expected = "timers are disabled")]
fn a_runtime_without_drivers_cannot_poll_http() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let service = Service::new(runtime.handle().clone(), Arc::new(NoSockets)).unwrap();
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    let url = url::Url::parse("http://127.0.0.1/").unwrap();
    let request = http::prepare(
        &url,
        hyper::http::Method::GET,
        hyper::http::HeaderMap::new(),
        http::Payload::default(),
    )
    .unwrap();
    let _ = runtime.block_on(service.request(&mut permit, &url, request, Duration::from_secs(1)));
}

#[test]
#[should_panic(expected = "IO is disabled")]
fn a_time_driver_alone_is_insufficient_for_http() {
    struct Table;
    impl SocketTable for Table {
        fn register(&self, _: RawFd) -> Result<usize, Error> {
            Ok(0)
        }
        fn remove(&self, _: usize) {}
        fn closing(&self) -> bool {
            false
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let service = Service::new(runtime.handle().clone(), Arc::new(Table)).unwrap();
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    let url = url::Url::parse("http://127.0.0.1/").unwrap();
    let request = http::prepare(
        &url,
        hyper::http::Method::GET,
        hyper::http::HeaderMap::new(),
        http::Payload::default(),
    )
    .unwrap();
    let _ = runtime.block_on(service.request(&mut permit, &url, request, Duration::from_secs(1)));
}
