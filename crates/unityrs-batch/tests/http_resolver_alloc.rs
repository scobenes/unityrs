//! Oversized NSS answers cross the same fixed-storage walker as real DNS.
#![cfg(feature = "test-hooks")]
#[path = "http_support/count.rs"]
mod count;
mod http_support;
use http_support::tls::Table;
use std::sync::Arc;
use unityrs_batch::{
    alloc::Counting,
    http::{
        Detail, Error, Service,
        permit::PermitPool,
        resolve::{Addresses, LOOKUP_ENVELOPE, MAX_ADDRESSES, Resolver},
    },
    sys::TestResolverAnswers,
};
use unityrs_testkit::guard::DEADLOCK_GUARD;

#[global_allocator]
static ALLOCATOR: Counting<count::Network> = Counting(count::Network);

struct Oversized(TestResolverAnswers);
impl Resolver for Oversized {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Addresses> {
        assert_eq!(host, "localhost");
        assert_eq!(port, 80);
        let before = count::allocations();
        let answers = self.0.resolve()?;
        assert_eq!(
            count::allocations(),
            before,
            "blocking answer walker allocated"
        );
        assert_eq!(answers.len(), MAX_ADDRESSES);
        assert!(answers.iter().enumerate().all(|(index, address)| {
            address == std::net::SocketAddr::from(([127, 0, 0, u8::try_from(index).unwrap()], 80))
        }));
        // Stop before TCP: this scene isolates the blocking lookup charge.
        Err(std::io::ErrorKind::NotFound.into())
    }
}

#[test]
fn refused_name_buffer_is_memory_and_releases_the_permit_charge() {
    let _serial = count::lock();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pool = PermitPool::new(1).unwrap();
    let mut permit = pool.acquire().unwrap();
    let answers = Arc::new(Oversized(TestResolverAnswers::new(16).unwrap()));
    let service = Service::new(runtime.handle().clone(), Arc::new(Table::default()))
        .unwrap()
        .with_test_hooks(answers, &[])
        .unwrap();
    let url = url::Url::parse("http://localhost/").unwrap();
    let request = http_support::get();
    let before = unityrs_batch::alloc::refusals();
    let result = runtime.block_on(async {
        count::refuse_next(url.host_str().unwrap().len() + 1);
        let result = tokio::time::timeout(
            DEADLOCK_GUARD,
            service.request(&mut permit, &url, request, DEADLOCK_GUARD),
        )
        .await;
        count::refuse_next(0);
        result.expect("deadlock guard").err().unwrap()
    });
    assert_eq!(result, Error::Memory);
    assert_eq!(unityrs_batch::alloc::refusals(), before + 1);
    assert_eq!(permit.charged(), 0);
    assert_eq!(pool.snapshot().outstanding, 0);
}

#[test]
fn oversized_answer_list_stays_within_the_lookup_envelope() {
    let _serial = count::lock();
    // This models a C/NSS-owned answer list larger than the Rust charge.
    // Fixture construction and destruction are outside the counted window.
    let answers = Arc::new(Oversized(TestResolverAnswers::new(200_000).unwrap()));
    count::start();
    let result = {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .on_thread_start(count::mark)
            .build()
            .unwrap();
        let pool = PermitPool::new(1).unwrap();
        let mut permit = pool.acquire().unwrap();
        let service = Service::new(runtime.handle().clone(), Arc::new(Table::default()))
            .unwrap()
            .with_test_hooks(answers.clone(), &[])
            .unwrap();
        let url = url::Url::parse("http://localhost/").unwrap();
        let result = runtime.block_on(async {
            tokio::time::timeout(
                DEADLOCK_GUARD,
                service.request(&mut permit, &url, http_support::get(), DEADLOCK_GUARD),
            )
            .await
            .expect("deadlock guard")
            .err()
            .unwrap()
        });
        assert_eq!(permit.charged(), 0);
        assert_eq!(pool.snapshot().outstanding, 0);
        result
    };
    let figures = count::finish();
    assert_eq!(result.detail(), Some(Detail::Connect));
    assert!(figures.peak <= LOOKUP_ENVELOPE, "lookup: {figures:?}");
    println!(
        "HTTP_RESOLVER_MEMORY peak={} allowance={LOOKUP_ENVELOPE}",
        figures.peak
    );
}
