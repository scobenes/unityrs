//! The provider's memory as a caller meets it. Round 5 (L3): a growth the
//! allocator refuses while a compressed TPK decodes is the load's
//! `OutOfMemory` on the provider's reservation, as every other growth of the
//! provider's is, not a decode failure: the refusal depends on what the
//! process held, not on the TPK, and the same bytes load once the allocator
//! gives the memory. (liblzma's own dictionary, which the C allocator makes,
//! is the codec's `Error::Budget` too: `unityrs_core::codec`'s LZMA test.)
//! Round 6: what the provider answers allocates nothing the TPK sizes.

use std::cell::Cell;

use unityrs_core::UnityVersion;
use unityrs_core::limits::Limits;
use unityrs_core::provider::{TreeRequest, TypeTreeProvider};
use unityrs_testkit::bundle::{TestCodec, compress};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread};
use unityrs_tpk::{TpkError, TpkProvider};

std::thread_local! {
    /// Whether this thread's requests above 1 MiB are refused.
    static ARMED: Cell<bool> = const { Cell::new(false) };
    /// The bytes this thread allocated, once it counts its allocations
    /// (`count_this_thread`).
    static GREW: Cell<usize> = const { Cell::new(0) };
}

/// Refuses every request above 1 MiB of a thread that armed it, and adds
/// what a counted thread allocates to its own `GREW`.
struct LargeRefusing;

impl Tally for LargeRefusing {
    fn refuses(&self, size: usize) -> bool {
        size > 1 << 20 && ARMED.try_with(Cell::get).unwrap_or(false)
    }

    fn grew(&self, size: usize, _zeroed: bool) {
        let _ = GREW.try_with(|g| g.set(g.get().saturating_add(size)));
    }

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

/// A stored TPK of [`TpkProvider::MAX_RECORDS`] classes, each with one entry
/// at 2.0.0 naming release root 0, a node with no children.
fn many_classes_tpk() -> Vec<u8> {
    let v = (2_u64 << 48).to_le_bytes();
    let mut blob = 1_i64.to_le_bytes().to_vec();
    blob.extend_from_slice(&1_i32.to_le_bytes());
    blob.extend_from_slice(&v);
    let classes = i32::try_from(TpkProvider::MAX_RECORDS).unwrap();
    blob.extend_from_slice(&classes.to_le_bytes());
    for id in 0..classes {
        blob.extend_from_slice(&id.to_le_bytes());
        blob.extend_from_slice(&1_i32.to_le_bytes());
        blob.extend_from_slice(&v);
        // A class (flag 1): name and base string 0, flags 192 (editor and
        // release roots), both node 0.
        blob.extend_from_slice(&[1, 0, 0, 0, 0, 192, 0, 0, 0, 0]);
    }
    // No common strings or indices; one node, strings 0 and 1, size -1,
    // version 1, flags 0, no children; the two strings.
    blob.extend_from_slice(&[0; 8]);
    blob.extend_from_slice(&1_i32.to_le_bytes());
    blob.extend_from_slice(&[
        0, 0, 1, 0, 0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    blob.extend_from_slice(&2_i32.to_le_bytes());
    blob.extend_from_slice(&[2, b's', b'0', 2, b's', b'1']);
    let size = u32::try_from(blob.len()).unwrap().to_le_bytes();
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&size);
    out.extend_from_slice(&size);
    out.extend_from_slice(&blob);
    out
}

/// The bytes the calling thread allocates while `f` runs.
fn allocated_by<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = GREW.with(Cell::get);
    let got = f();
    (got, GREW.with(Cell::get) - before)
}

/// Round 6 (MEDIUM): `class_ids()` collected the ids into a `Vec`, up to
/// [`TpkProvider::MAX_RECORDS`] of them (256 KiB), allocated outside the
/// provider's meter for as long as the caller kept it. It reads the
/// provider's own table now; and nothing else the provider answers
/// allocates what the TPK sizes: its versions and common strings are
/// borrowed, and a node list or tree already built is the kept `Arc`.
#[test]
fn what_the_provider_answers_allocates_nothing_the_tpk_sizes() {
    count_this_thread();
    let provider = TpkProvider::from_bytes(&many_classes_tpk(), &Limits::default()).unwrap();
    assert_eq!(provider.class_count(), TpkProvider::MAX_RECORDS);
    let ((count, last), bytes) = allocated_by(|| {
        let ids = provider.class_ids();
        let count = ids.len();
        (count, ids.last())
    });
    assert_eq!((count, last), (TpkProvider::MAX_RECORDS, Some(65_535)));
    assert_eq!(bytes, 0, "class_ids() allocated {bytes} bytes");
    let version = UnityVersion::from_packed(2 << 48);
    let request = TreeRequest {
        class_id: 7,
        version: &version,
        script: None,
    };
    // The first asks build and keep the list and the tree.
    assert!(provider.node_list(7, &version).unwrap().is_some());
    assert!(provider.tree(&request).unwrap().is_some());
    let (_, bytes) = allocated_by(|| {
        (
            provider.versions().len(),
            provider.common_strings().is_some(),
            provider.node_list(7, &version).unwrap(),
            provider.tree(&request).unwrap(),
        )
    });
    assert_eq!(bytes, 0, "a kept answer allocated {bytes} bytes");
}
