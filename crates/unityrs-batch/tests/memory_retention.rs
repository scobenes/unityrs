//! E57: RSS retention is separate from the charge-unit X12 argument.
#![cfg(target_os = "linux")]
use std::{fs, process::Command};
use unityrs_testkit::child;

fn rss() -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024
}
#[test]
fn retention_child() {
    let Ok(settings) = std::env::var("B12_RETENTION") else {
        return;
    };
    if settings == "on" {
        unityrs_batch::sys::allocator(1).unwrap();
    }
    let before = rss();
    let mut peak = before;
    for _ in 0..16 {
        let mut bytes = vec![0_u8; 32 << 20];
        for page in bytes.chunks_mut(4096) {
            page[0] = 1;
        }
        peak = peak.max(rss());
        std::hint::black_box(&bytes);
        drop(bytes);
    }
    let after = rss();
    assert!(
        peak >= before + (24 << 20),
        "RSS scene did not touch mappings"
    );
    if settings == "on" {
        assert!(
            after <= before + (8 << 20),
            "RSS retention {before} {after}"
        );
    }
    println!(
        "B12_RSS {} {before} {peak} {after}",
        u8::from(settings == "on")
    );
}
#[test]
fn rss_tracks_live_heap_after_large_frees_with_and_without_settings() {
    for settings in ["off", "on"] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "retention_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("B12_RETENTION", settings),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
