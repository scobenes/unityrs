//! The connector's caps, counted: no server's
//! `Content-Length` sizes an allocation, nothing is read past a cap, and a
//! body's allocations do not grow with what lies past it.
mod store_support;
use object_store::{
    ObjectStoreExt, PutPayload, list::PaginatedListOptions, list::PaginatedListStore, path::Path,
};
use std::{cell::Cell, sync::atomic::Ordering};
use store_support::{End, Harness, Reply};
use unityrs_batch::{
    alloc::{self, Counting, Hook},
    http::HEAD_CAP,
    store::{Kind, LIST_CAP, attempt::BodyFault, classify::Action},
};

thread_local! { static BLOCKS: Cell<u64> = const { Cell::new(0) }; }
struct Blocks;
impl Hook for Blocks {
    fn grew(&self, _: usize, _: bool) {
        let _ = BLOCKS.try_with(|n| n.set(n.get() + 1));
    }
}
#[global_allocator]
static ALLOCATOR: Counting<Blocks> = Counting(Blocks);

const TIB: u64 = 1 << 40;
const MIB: u64 = 1 << 20;

struct Measure {
    peak: u64,
    blocks: u64,
    served: u64,
    action: Option<Action>,
    body: Option<BodyFault>,
}

/// Runs one try of `kind` over `reply` on this thread, counting it.
fn measure(kind: Kind, reply: Reply) -> Measure {
    let h = Harness::new([reply]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let path = Path::parse("out/a.png").unwrap();
    let before = BLOCKS.with(Cell::get);
    let (done, measured) = alloc::measured(|| {
        runtime.block_on(h.tries().run(&h.slot, kind, |_| {
            let (store, path) = (&h.store, &path);
            async move {
                if kind == Kind::Put {
                    store
                        .put(path, PutPayload::from_static(b"x"))
                        .await
                        .map(|_| ())
                } else {
                    store
                        .list_paginated(Some("out/"), PaginatedListOptions::default())
                        .await
                        .map(|_| ())
                }
                .map_err(|_| unityrs_batch::store::classify::Ended::Error)
            }
        }))
    });
    let done = done.unwrap();
    Measure {
        peak: measured.peak,
        blocks: BLOCKS.with(Cell::get) - before,
        served: h.script.seen.served.load(Ordering::Acquire),
        action: done.verdict.map(|v| v.action),
        body: done.record.body,
    }
}

fn reply(head: &str, chunk: &[u8], count: usize) -> (Reply, u64) {
    let mut chunks = vec![head.as_bytes().to_vec()];
    chunks.extend(std::iter::repeat_n(chunk.to_vec(), count));
    (Reply::chunks(chunks, End::Stall), head.len() as u64)
}

#[test]
fn an_error_body_under_a_huge_length_is_read_to_64_kib_once() {
    let head = format!("HTTP/1.1 503 X\r\nContent-Length: {TIB}\r\n\r\n");
    let chunk = [b'e'; 1_024];
    let (small, head_len) = reply(&head, &chunk, 2_048);
    let small = measure(Kind::Put, small);
    let (large, _) = reply(&head, &chunk, 20_480);
    let large = measure(Kind::Put, large);
    for m in [&small, &large] {
        assert_eq!(m.action, Some(Action::Retry { retry_after: true }));
        // Nothing past the cut is read but hyper's bounded read-ahead.
        assert!(
            m.served <= head_len + 64 * 1_024 + HEAD_CAP as u64,
            "{}",
            m.served
        );
        assert!(m.peak < 2 * MIB, "{}", m.peak);
    }
    // What lies past the cap allocates nothing.
    assert_eq!(small.blocks, large.blocks);
    assert_eq!(small.served, large.served);
}

#[test]
fn a_list_body_past_its_cap_allocates_nothing_more() {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {TIB}\r\n\r\n<r>");
    let mut chunk = b"<a>".to_vec();
    chunk.resize(6_003, b'x');
    chunk.extend_from_slice(b"</a>");
    let per = chunk.len() as u64;
    // Both bodies run past the cap and hyper's read-ahead, the far one by
    // megabytes more.
    let count = usize::try_from((LIST_CAP + HEAD_CAP as u64) / per).unwrap() + 4;
    let (near, head_len) = reply(&head, &chunk, count);
    let near = measure(Kind::ListPage, near);
    let (far, _) = reply(&head, &chunk, count * 4);
    let far = measure(Kind::ListPage, far);
    for m in [&near, &far] {
        assert_eq!(m.action, Some(Action::Retry { retry_after: true }));
        assert_eq!(m.body, Some(BodyFault::OverCap));
        // Past the cap only hyper's bounded read-ahead (its `max_buf_size`).
        assert!(
            m.served <= head_len + LIST_CAP + HEAD_CAP as u64,
            "{}",
            m.served
        );
        // The collected body is sized by the clamped hint, never 1 TiB.
        assert!(m.peak < 2 * LIST_CAP, "{}", m.peak);
    }
    assert_eq!(near.blocks, far.blocks);
    assert_eq!(near.served, far.served);
}
