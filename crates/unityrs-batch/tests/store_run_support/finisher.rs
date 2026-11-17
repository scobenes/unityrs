use super::*;
use std::{
    io,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use unityrs_testkit::guard::DEADLOCK_GUARD;
struct HeldLookup {
    address: std::net::SocketAddr,
    calls: AtomicUsize,
    entered: AtomicBool,
    release: AtomicBool,
}
impl unityrs_batch::http::resolve::Resolver for HeldLookup {
    fn resolve(&self, _: &str, _: u16) -> io::Result<unityrs_batch::http::resolve::Addresses> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 3 {
            self.entered.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + DEADLOCK_GUARD;
            while !self.release.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "held finisher lookup guard");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(unityrs_batch::http::resolve::Addresses::first([
            self.address
        ]))
    }
}
#[test]
fn x10_the_head_uses_another_token_while_a_finisher_lookup_is_quarantined() {
    store_support::run(async {
        let fake = Fake::start_with(store_support::fake::Config {
            tls: true,
            ..Default::default()
        })
        .await;
        let scratch = Scratch::new("store-finisher-lookup");
        let mut spec = make_spec(&scratch, &fake);
        spec["download_concurrency"] = 1.into();
        spec["sink"]["timeout_ms"] = 1000.into();
        let resolver = Arc::new(HeldLookup {
            address: fake.address(),
            calls: AtomicUsize::new(0),
            entered: AtomicBool::new(false),
            release: AtomicBool::new(false),
        });
        let mut network = run::Network::system();
        network.hooks = Some(run::Hooks {
            resolver: resolver.clone(),
            extra_roots: vec![store_support::fake::ROOT.to_vec().into()],
        });
        let lifetime = Arc::new(run::Lifetime::default());
        let control = lifetime.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(drive_options(
                &spec,
                &scratch,
                run::Options {
                    network: Some(network),
                    lifetime: control,
                    ..Default::default()
                },
            ));
        });
        while !resolver.entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let now = unityrs_batch::lifetime::Boot::now().unwrap();
        lifetime
            .state()
            .arm(unityrs_batch::lifetime::Boot(now.0 + 3100));
        let result = receive.await.unwrap().unwrap();
        // Return the quarantined operation before asserting, so a failing test
        // does not retain a deliberately held blocking-pool thread.
        resolver.release.store(true, Ordering::SeqCst);
        assert_eq!(result.code, Code::Incomplete);
        assert_eq!(head(&fake)["write_seq"], 1);
        assert!(resolver.calls.load(Ordering::SeqCst) > 4);
    });
}
