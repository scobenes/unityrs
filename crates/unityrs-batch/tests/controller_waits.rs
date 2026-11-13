//! B5's finite W1–W6 wait inventory. Startup's W20 and explicit test/host
//! quiescence are listed separately; neither runs on the handover path.
//! Method names here are literal scan tokens, not timed test calls; tests
//! that actually wait use the testkit `DEADLOCK_GUARD` through `guard`.
use std::{collections::BTreeMap, path::Path};
use unityrs_testkit::waits::live_code;
const TOKENS: &[&str] = &[
    ".lock(",
    ".recv(",
    ".recv_timeout(",
    ".wait_timeout_while(",
    ".block_on(",
    "worker.join(",
    "libc::poll(",
    "thread::sleep(",
    "thread::park(",
    "thread::yield_now(",
    "spin_loop(",
];
fn inventory(name: &str, text: &str, out: &mut BTreeMap<(String, String), usize>) {
    let code: String = live_code(text).split_whitespace().collect();
    for token in TOKENS {
        let n = code.matches(token).count();
        if n != 0 {
            out.insert((name.to_owned(), (*token).to_owned()), n);
        }
    }
}
fn scan(dir: &Path, root: &Path, out: &mut BTreeMap<(String, String), usize>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path.strip_prefix(root).unwrap().to_str().unwrap();
            // Existing progress/early has its own bounded wait tests. This
            // inventory owns B5's added execution and orchestration only.
            if name.starts_with("controller/")
                || [
                    "execution.rs",
                    "worker.rs",
                    "sys.rs",
                    "control.rs",
                    "phase1.rs",
                ]
                .contains(&name)
            {
                inventory(name, &std::fs::read_to_string(&path).unwrap(), out);
            }
        }
    }
}
#[test]
fn every_engine_wait_has_a_bounded_or_idle_role_and_dispatch_unlocks_before_core() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeMap::new();
    scan(&root, &root, &mut found);
    let expected = [
        ("execution.rs", ".lock(", 2),               // W20: prewarm gate only
        ("execution.rs", ".recv_timeout(", 1),       // W20: startup deadline
        ("execution.rs", ".wait_timeout_while(", 1), // W20: released at startup
        ("execution.rs", ".block_on(", 1),           // W2: already-finished pool handle
        ("execution.rs", "worker.join(", 1),         // explicit quiesce only
        ("worker.rs", ".lock(", 2),                  // W3 receive, and moving a completed owner
        ("worker.rs", ".recv(", 1),                  // W3 idle worker
        ("sys.rs", "libc::poll(", 1),                // W1/W2: 1 s, stop and end descriptors
    ]
    .into_iter()
    .map(|(file, token, n)| ((file.to_owned(), token.to_owned()), n))
    .collect();
    assert_eq!(found, expected);
    let code = live_code(&std::fs::read_to_string(root.join("worker.rs")).unwrap());
    let lock = code
        .split("let job = {")
        .nth(1)
        .unwrap()
        .split("let Ok(job)")
        .next()
        .unwrap();
    assert!(lock.contains("receive.lock()") && lock.contains("receiver.recv()"));
    assert!(
        !lock.contains("execute(") && !lock.contains("run_attempt(") && !lock.contains("run_peek(")
    );
    let mut mutant = BTreeMap::new();
    inventory(
        "worker.rs",
        "fn bad() { std::thread::sleep(Duration::ZERO); }",
        &mut mutant,
    );
    assert_eq!(
        mutant.get(&("worker.rs".to_owned(), "thread::sleep(".to_owned())),
        Some(&1)
    );
}
