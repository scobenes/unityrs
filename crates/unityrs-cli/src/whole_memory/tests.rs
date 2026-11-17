use super::*;
use std::io::Write;
use unityrs_batch::progress::Phase;

#[test]
fn cross_thread_growth_failed_growth_and_realloc_shrink_balance() {
    let before = WHOLE.observe();
    let held_before = HELD.observe();
    Bridge.growing(1024);
    let grown = WHOLE.observe();
    assert_eq!(
        grown.heap - before.heap,
        i64::try_from(heap_cost(1024)).unwrap()
    );
    // A moving realloc counts overlap before releasing the old block.
    std::thread::spawn(|| {
        crate::test_alloc::REFUSE.with(|r| r.set(true));
        Bridge.growing(2048);
        Bridge.shrank(1024);
        Bridge.growing(4096);
        Bridge.failed(4096);
        Bridge.shrunk(2048, 64);
        Bridge.shrank(64);
        let observer = observer();
        let charged = HELD.observe().charged;
        (observer.charge)(4096, true);
        (observer.network)(8192, true);
        assert_eq!(HELD.observe().charged - charged, 4096);
        assert_eq!(NETWORK.load(SeqCst), 8192);
        (observer.charge)(4096, false);
        (observer.network)(8192, false);
        assert_eq!(HELD.observe().charged, charged);
        assert_eq!(NETWORK.load(SeqCst), 0);
        assert!(crate::test_alloc::REFUSE.with(|r| r.replace(false)));
    })
    .join()
    .unwrap();
    let after = WHOLE.observe();
    assert_eq!(after.heap, before.heap);
    assert_eq!(HELD.observe().heap, held_before.heap);
    assert!(
        after.peak_above_arm
            >= i64::try_from(heap_cost(2048)).unwrap() + i64::try_from(heap_cost(4096)).unwrap()
    );
    assert_eq!(after.events - before.events, 7);
}

#[test]
fn reporting_is_opt_in_once_and_allocation_free_even_when_allocations_are_refused() {
    let mut bytes = [0; 512];
    let mut sink = std::io::Cursor::new(bytes.as_mut_slice());
    report_to(|bytes| sink.write_all(bytes).unwrap());
    assert_eq!(sink.position(), 0);
    configure(1, 1);
    for phase in [
        Phase::Validate,
        Phase::Preflight,
        Phase::Start,
        Phase::Stage,
        Phase::Extract,
        Phase::Winners,
        Phase::Publish,
        Phase::Finish,
    ] {
        point(Point::Phase(phase));
    }
    ENABLED.store(true, SeqCst);
    crate::test_alloc::REFUSE.with(|r| r.set(true));
    report_to(|bytes| sink.write_all(bytes).unwrap());
    // Every allocation would have consumed the refusal (or aborted).
    assert!(crate::test_alloc::REFUSE.with(|r| r.replace(false)));
    let used = sink.position();
    report_to(|bytes| sink.write_all(bytes).unwrap());
    assert_eq!(sink.position(), used);
    let text = std::str::from_utf8(&bytes[..usize::try_from(used).unwrap()]).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("B12_COEFFICIENT c_base "));
    let words: Vec<_> = lines[1].split(' ').collect();
    assert_eq!(words.len(), 13);
    assert_eq!(words[0], "WHOLE_MEMORY_V1");
    assert_eq!(&words[3..6], &["255", "1", "1"]);
    assert_eq!(lines[0].split(' ').nth(2), Some(words[1]));
    REPORTED.store(false, SeqCst);
    crate::test_alloc::REFUSE.with(|r| r.set(true));
    report(); // Exercise the real descriptor write too.
    assert!(crate::test_alloc::REFUSE.with(|r| r.replace(false)));
    ENABLED.store(false, SeqCst);
}

#[test]
fn stack_decimal_format_handles_zero_and_u64_max_without_allocation() {
    let mut line = Line::new();
    crate::test_alloc::REFUSE.with(|r| r.set(true));
    line.number(0);
    line.text(b" ");
    line.number(u64::MAX);
    assert!(crate::test_alloc::REFUSE.with(|r| r.replace(false)));
    assert_eq!(&line.bytes[..line.used], b"0 18446744073709551615");
}
