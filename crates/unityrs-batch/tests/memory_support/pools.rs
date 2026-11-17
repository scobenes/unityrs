//! Real blocking lookup cells on each runtime; only fixture futures and the
//! observer thread are prepared outside the measured increments.
use super::*;
use std::{future::Future, io, time::Instant};
use unityrs_batch::http::{
    permit::PermitPool,
    resolve::{Addresses, Resolver},
};
use unityrs_testkit::guard::DEADLOCK_GUARD;

struct Held {
    entered: AtomicU64,
    release: AtomicBool,
}
impl Resolver for Held {
    fn resolve(&self, _: &str, _: u16) -> io::Result<Addresses> {
        self.entered.fetch_add(1, SeqCst);
        let until = Instant::now() + DEADLOCK_GUARD;
        while !self.release.load(SeqCst) {
            assert!(Instant::now() < until, "lookup release guard");
            std::thread::yield_now();
        }
        Ok(Addresses::first(["127.0.0.1:80".parse().unwrap()]))
    }
}
#[test]
fn actual_lookup_cells_on_both_pools_fit_the_pool_increment() {
    for dc in [1, 4, 16] {
        alone("real initialized lookup cells", move || {
            let n = u64::from(dc + 1);
            let runtimes = unityrs_batch::lifetime::Runtimes::prepare_with(dc, register).unwrap();
            LEDGER.arm(u64::MAX);
            let tables = [
                PermitPool::with_base(0, usize::try_from(n).unwrap()).unwrap(),
                PermitPool::with_base(257, usize::try_from(n).unwrap()).unwrap(),
            ];
            runtimes.prewarm().unwrap();
            let startup = u64::try_from(LEDGER.observe().peak_above_arm)
                .unwrap()
                .div_ceil(2 * n);
            for (runtime, table) in [&runtimes.controller, &runtimes.finisher]
                .into_iter()
                .zip(&tables)
            {
                let resolver = Arc::new(Held {
                    entered: AtomicU64::new(0),
                    release: AtomicBool::new(false),
                });
                let held = resolver.clone();
                // Poll harness storage, futures and observer are outside arm.
                let mut permits: Vec<_> = (0..n).map(|_| table.acquire().unwrap()).collect();
                let host = "h".repeat(253);
                let mut futures: Vec<_> = permits
                    .iter_mut()
                    .map(|permit| {
                        Some(Box::pin(unityrs_batch::run::hooks::memory::probe_lookup(
                            runtime.handle(),
                            permit,
                            resolver.clone(),
                            &host,
                        )))
                    })
                    .collect();
                let observer = std::thread::spawn(move || {
                    let until = Instant::now() + DEADLOCK_GUARD;
                    while held.entered.load(SeqCst) != n {
                        assert!(Instant::now() < until, "lookup entry guard");
                        std::thread::yield_now();
                    }
                    let peak = LEDGER.observe().peak_above_arm;
                    held.release.store(true, SeqCst);
                    peak
                });
                LEDGER.arm(u64::MAX);
                runtime.block_on(std::future::poll_fn(|cx| {
                    let mut pending = false;
                    for slot in &mut futures {
                        if let Some(future) = slot {
                            if future.as_mut().poll(cx).is_ready() {
                                *slot = None;
                            } else {
                                pending = true;
                            }
                        }
                    }
                    if pending {
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(())
                    }
                }));
                let cells = u64::try_from(observer.join().unwrap()).unwrap().div_ceil(n);
                // Adding maxima is conservative; no incompatible peak is
                // ever subtracted to make a coefficient appear smaller.
                let raw = startup + cells;
                assert!(raw <= PROVISIONAL.m_pool, "pool increment {raw}");
                println!("\nB12_COEFFICIENT m_pool {raw}");
            }
        });
    }
}

#[test]
fn store_owner_tables_at_maximum_concurrency_fit_the_pool_slope() {
    for dc in [1_u32, 16, 256] {
        alone("bounded store owner tables", move || {
            let text = format!(
                r#"{{"spec_version":1,"run_id":"r","attempt":1,
                "threads":1,"download_concurrency":{dc},"max_inflight_uncompressed_bytes":1073741824,
                "max_decoded_bytes":16777216,"dep_cache_bytes":16777216,
                "deadline":"2099-01-01T00:00:00Z","spool":{{"dir":"/spool","size_bytes":1073741824,"reserve_bytes":0}},
                "outputs":{{"textures":{{"key":"{{name}}.png"}}}},"inputs":[{{"id":"a","path":"/input"}}],
                "sink":{{"kind":"object_store","bucket":"bucket","region":"us-east-1"}}}}"#
            );
            let spec = Arc::new(unityrs_batch::spec::parse(text.into_bytes()).unwrap().spec);
            let provider = unityrs_batch::store::credentials::read(
                |name| match name {
                    "AWS_ACCESS_KEY_ID" => Some("K".repeat(128).into()),
                    "AWS_SECRET_ACCESS_KEY" => Some("S".repeat(128).into()),
                    "AWS_SESSION_TOKEN" => Some("T".repeat(16384).into()),
                    _ => None,
                },
                None,
            )
            .unwrap()
            .into_provider();
            let slots = 2 * u64::from(dc + 1);
            LEDGER.arm(slots * PROVISIONAL.m_pool);
            unityrs_batch::run::hooks::memory::probe_store_preparation(spec, &provider, || {
                let o = LEDGER.observe();
                assert!(o.first_violation.is_none(), "store tables: {o:?}");
                let raw = u64::try_from(o.peak_above_arm).unwrap().div_ceil(slots);
                println!("\nB12_COEFFICIENT m_pool {raw}");
            })
            .unwrap();
        });
    }
}
