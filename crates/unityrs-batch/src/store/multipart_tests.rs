use super::*;
use crate::{alloc, test_alloc::REFUSE};

#[test]
fn maximal_part_ids_are_bounded_and_every_copy_allocation_is_fallible() {
    let tags = vec![Etag::new(&[b'x'; 100]).unwrap(); 10000];
    let (ids, measured) = alloc::measured(|| copy_parts(&tags).unwrap());
    assert_eq!(ids.len(), 10000);
    assert!(ids.iter().all(|id| id.content_id.len() == 100));
    assert!(measured.peak < 2 * 1024 * 1024, "{}", measured.peak);
    drop(ids);
    for ordinal in [1, 2, 3, 5001, 10001] {
        REFUSE.with(|refuse| refuse.set(ordinal));
        let result = copy_parts(&tags);
        REFUSE.with(|refuse| refuse.set(0));
        assert_eq!(result.unwrap_err(), Fault::Memory);
    }
}

#[test]
fn payload_buffer_allocation_refusals_return_host_memory_and_release_charges() {
    let account = Account::new(32 << 20);
    for size in [1, PART_SIZE] {
        REFUSE.with(|refuse| refuse.set(1));
        let result = Payload::read(&account, size, &mut std::io::repeat(0));
        REFUSE.with(|refuse| refuse.set(0));
        assert!(
            matches!(result,Err(Error::Phase(crate::phase::Error::Run(failure))) if failure.reason==crate::attempt::ReasonCode::HostMemory)
        );
        assert_eq!(account.snapshot().input.used, 0);
    }
}
