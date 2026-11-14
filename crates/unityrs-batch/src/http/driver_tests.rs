//! What the Execution's HTTP runtime needs: both of Tokio's IO and time drivers.
use crate::{
    http::{self, Service, permit::PermitPool},
    sys::reset::SocketTable,
};
use std::{sync::Arc, time::Duration};

// The integration precondition: the runtime that polls a request needs both
// of Tokio's drivers (the library Execution's and the host's controller
// runtime enable both). These guards show that one without them cannot.
#[test]
#[should_panic(expected = "timers are disabled")]
fn a_runtime_without_drivers_cannot_poll_http() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let service = Service::new(runtime.handle().clone(), Arc::new(SocketTable::new())).unwrap();
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let service = Service::new(runtime.handle().clone(), Arc::new(SocketTable::new())).unwrap();
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
