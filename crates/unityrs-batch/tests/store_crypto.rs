//! A refused signing context must fail the try before sending a request.
//! The children also catch a regression to the dependency's aborting boxes.
mod store_support;
use object_store::{ObjectStoreExt, PutPayload, path::Path};
use std::{backtrace::Backtrace, cell::Cell, process::Command};
use unityrs_batch::{
    alloc::{self, Counting, Hook},
    store::{Kind, attempt::Fault, classify::Ended},
};
use unityrs_testkit::{child, guard::DEADLOCK_GUARD};

thread_local! {
    static TARGET: Cell<u8> = const { Cell::new(0) };
    static TRACING: Cell<bool> = const { Cell::new(false) };
    static REFUSED: Cell<bool> = const { Cell::new(false) };
}
struct ContextRefusal;
impl Hook for ContextRefusal {
    fn refuses(&self, _: usize) -> bool {
        let target = TARGET.with(Cell::get);
        if target == 0 || TRACING.with(Cell::get) {
            return false;
        }
        // Symbol resolution allocates; those diagnostic allocations pass.
        TRACING.with(|t| t.set(true));
        let stack = format!("{}", Backtrace::force_capture());
        let method = if target == 1 { "digest" } else { "hmac" };
        let context = if target == 1 { "Digest" } else { "Hmac" };
        let refused = stack.contains(&format!(
            "try_box_uninit<unityrs_batch::store::crypto::{context}>"
        )) || stack.lines().any(|line| {
            line.contains("object_store::client::crypto::CryptoProvider")
                && line.ends_with(&format!(">::{method}"))
        });
        TRACING.with(|t| t.set(false));
        if refused {
            TARGET.with(|t| t.set(0));
            REFUSED.with(|t| t.set(true));
        }
        refused
    }
}
#[global_allocator]
static ALLOCATOR: Counting<ContextRefusal> = Counting(ContextRefusal);
const PROBE: &str = "UNITYRS_STORE_CRYPTO_REFUSAL";

#[test]
#[ignore = "run by signing_context_refusals_return_memory"]
fn signing_context_refusal_child() {
    let Some(target) = std::env::var_os(PROBE) else {
        return;
    };
    let target: u8 = target.to_str().unwrap().parse().unwrap();
    let h =
        store_support::Harness::new([store_support::Reply::status(200, "ETag: \"tag\"\r\n", b"")]);
    let path = Path::parse("key").unwrap();
    let mut tries = h.tries();
    tries.policy.timeout = DEADLOCK_GUARD;
    let before = alloc::refusals();
    let result = store_support::run(async {
        TARGET.with(|t| t.set(target));
        let result = tries
            .run(&h.slot, Kind::Put, |_| async {
                h.store
                    .put(&path, PutPayload::from_static(b"payload"))
                    .await
                    .map_err(|_| Ended::Error)
            })
            .await;
        TARGET.with(|t| t.set(0));
        result
    });
    assert!(
        REFUSED.with(Cell::get),
        "context allocation was not reached"
    );
    assert!(matches!(result, Err(Fault::Memory)), "{result:?}");
    assert_eq!(alloc::refusals(), before + 1);
    assert_eq!(h.script.seen.count(), 0, "refusal must precede transport");
    assert_eq!(h.slot.record().fault, Some(Fault::Memory));
    assert_eq!(h.slot.take_permit().unwrap().charged(), 0);
}

#[test]
fn signing_context_refusals_return_memory() {
    for target in [1, 2] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "signing_context_refusal_child",
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(PROBE, target.to_string()),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "context {target}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
