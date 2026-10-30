//! Round 5 (L3): a growth the allocator refuses while a compressed TPK
//! decodes is the load's `OutOfMemory` on the provider's reservation, as
//! every other growth of the provider's is, not a decode failure: the
//! refusal depends on what the process held, not on the TPK, and the same
//! bytes load once the allocator gives the memory.

use std::cell::Cell;

use unityrs_core::limits::Limits;
use unityrs_testkit::bundle::{TestCodec, compress};
use unityrs_testkit::counted::{Allocator, Tally};
use unityrs_tpk::{TpkError, TpkProvider};

std::thread_local! {
    /// Whether this thread's requests above 1 MiB are refused.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Refuses every request above 1 MiB of a thread that armed it. It counts
/// nothing.
struct LargeRefusing;

impl Tally for LargeRefusing {
    fn refuses(&self, size: usize) -> bool {
        size > 1 << 20 && ARMED.try_with(Cell::get).unwrap_or(false)
    }

    fn grew(&self, _size: usize, _zeroed: bool) {}

    fn shrank(&self, _size: usize) {}
}

#[global_allocator]
static ALLOC: Allocator<LargeRefusing> = Allocator(LargeRefusing);

/// An LZ4 TPK whose payload decodes to 4 MiB: a blob of one version and one
/// class whose one entry removes it, and zeros after its tables, which the
/// blob ignores, as UnityPy does.
fn lz4_tpk() -> Vec<u8> {
    let at = 2020_u64 << 48;
    let mut payload = 1_i64.to_le_bytes().to_vec();
    payload.extend_from_slice(&1_i32.to_le_bytes());
    payload.extend_from_slice(&at.to_le_bytes());
    for n in [1_i32, 1, 1] {
        // The classes, the class's id, its entries.
        payload.extend_from_slice(&n.to_le_bytes());
    }
    payload.extend_from_slice(&at.to_le_bytes());
    payload.push(0);
    payload.resize(4 << 20, 0);
    let data = compress(TestCodec::Lz4, &payload);
    assert!(data.len() < 1 << 20, "{} bytes", data.len());
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 1, 0, 0]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[test]
fn a_refused_decode_is_the_loads_out_of_memory_not_a_decode_failure() {
    let tpk = lz4_tpk();
    ARMED.with(|a| a.set(true));
    let got = TpkProvider::from_bytes(&tpk, &Limits::default());
    ARMED.with(|a| a.set(false));
    match got {
        Err(TpkError::Io(e)) => {
            assert_eq!(e.kind(), std::io::ErrorKind::OutOfMemory, "{e}");
            assert!(e.to_string().contains("for Provider"), "{e}");
        }
        got => panic!("not the load's OutOfMemory: {got:?}"),
    }
    let loaded = TpkProvider::from_bytes(&tpk, &Limits::default());
    assert_eq!(loaded.unwrap().class_count(), 1);
}
