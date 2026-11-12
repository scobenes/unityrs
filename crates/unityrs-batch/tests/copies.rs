//! No refused `endpoint` or capped sink string is copied before its
//! checks (review rounds 3 and 4, G3 and F1): an escaped string reaches
//! the parser's visitor unescaped, and the sink's scalar keeps a stand-in
//! for one its as-written checks or its byte cap refuse
//! (`docs/BATCH_SPEC.md` 1.6), never a copy. A copy is a block of the
//! unescaped string's exact length, which the counting allocator below
//! looks for while the spec is parsed.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::{base, store};
use serde_json::json;
use unityrs_batch::spec;
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The block size looked for, and how many blocks of it were made.
static SIZE: AtomicUsize = AtomicUsize::new(0);
static SEEN: AtomicUsize = AtomicUsize::new(0);

struct Sizes;

impl Tally for Sizes {
    fn grew(&self, size: usize, _zeroed: bool) {
        LIVE.fetch_add(size, Ordering::Relaxed);
        if size == SIZE.load(Ordering::Relaxed) {
            SEEN.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Sizes> = Allocator(Sizes);

/// The blocks of `len` bytes, the unescaped value's length, made while a
/// spec whose sink member `name` holds `escaped` (JSON text) is parsed,
/// and its refusal's detail, or `ok`. `serde_json` unescapes into a buffer
/// of its own, which grows to the length the text before the last escape
/// needs, so each value ends in an escape that takes it past `len`.
fn copies(name: &str, escaped: &str, len: usize) -> (usize, String) {
    let mut v = base();
    v["deadline"] = json!("2026-01-01T00:00:00Z");
    v["sink"] = store();
    v["sink"][name] = json!("E");
    let text = v
        .to_string()
        .replacen(r#""E""#, &format!(r#""{escaped}""#), 1);
    let bytes = text.into_bytes();
    SIZE.store(len, Ordering::Relaxed);
    SEEN.store(0, Ordering::Relaxed);
    let detail = match spec::parse(bytes) {
        Ok(_) => "ok".to_owned(),
        Err(e) => e.detail(),
    };
    (SEEN.load(Ordering::Relaxed), detail)
}

/// [`copies`] of the endpoint `https:\/\/<host>\/`.
fn endpoint(host: &str) -> (usize, String) {
    copies(
        "endpoint",
        &format!(r"https:\/\/{host}\/"),
        "https://".len() + host.len() + 1,
    )
}

/// [`copies`] of the bucket `<name>\u0061` (its last byte an `a`).
fn bucket(name: &str) -> (usize, String) {
    copies("bucket", &format!(r"{name}\u0061"), name.len() + 1)
}

#[test]
fn a_refused_sink_string_is_never_copied() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    // The control: an endpoint its checks accept is copied (and `url`'s
    // parse may take a buffer of its length too), so the count sees a copy
    // where there is one. Hosts of odd lengths, which no other block of
    // the parse has.
    let (seen, detail) = endpoint(&"a".repeat(201));
    assert_eq!(detail, "ok");
    assert!(seen >= 1, "the accepted endpoint's copy was not seen");
    // A host past 253 bytes as written, the endpoint within 1 KiB: no copy.
    let (seen, detail) = endpoint(&"a".repeat(777));
    assert_eq!(detail, "range /sink/endpoint");
    assert_eq!(seen, 0, "a refused endpoint was copied");
    // A capped sink string (F1): a bucket within its 255 bytes is copied,
    // one past it is not.
    let (seen, detail) = bucket(&"a".repeat(201));
    assert_eq!(detail, "ok");
    assert!(seen >= 1, "the accepted bucket's copy was not seen");
    let (seen, detail) = bucket(&"a".repeat(777));
    assert_eq!(detail, "range /sink/bucket");
    assert_eq!(seen, 0, "a refused bucket was copied");
}
