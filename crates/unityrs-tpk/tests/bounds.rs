//! `TpkProvider`'s loading bounds (`PARITY.md` section 19.3), where UnityPy
//! has none: the file, the decoded payload and each table of the blob. G7's
//! synthetic set holds a TPK past each bound with UnityPy's load recorded
//! (`tools/fixtures/tpk.py`'s `refused_*` cases); these cases are the ones
//! too large to commit, built here.

use std::io::Write;

use unityrs_core::limits::Limits;
use unityrs_testkit::bundle::{TestCodec, compress};
use unityrs_tpk::{TpkError, TpkProvider};

/// A TPK file around `payload` (`Tpk.py`'s `<IbbbbIII` header).
fn tpk(compression: u8, payload: &[u8], declared: u32) -> Vec<u8> {
    let data = match compression {
        1 => compress(TestCodec::Lz4, payload),
        2 => compress(TestCodec::Lzma, payload),
        _ => payload.to_vec(),
    };
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, compression, 0, 0]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&declared.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// A blob with one version, one class of `entries` entries that remove it
/// (9 bytes each), and empty tables after it.
fn removed_entries(entries: u32) -> Vec<u8> {
    let at = 2020_u64 << 48;
    let mut out = 1_i64.to_le_bytes().to_vec();
    out.extend_from_slice(&1_i32.to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&1_i32.to_le_bytes()); // classes
    out.extend_from_slice(&1_i32.to_le_bytes()); // its id
    out.extend_from_slice(&entries.to_le_bytes());
    for _ in 0..entries {
        out.extend_from_slice(&at.to_le_bytes());
        out.push(0);
    }
    for _ in 0..4 {
        out.extend_from_slice(&0_i32.to_le_bytes()); // common versions, indices, nodes, strings
    }
    out
}

fn too_large(got: Result<TpkProvider, TpkError>) -> (&'static str, u64, u64) {
    match got {
        Err(TpkError::TooLarge { what, value, max }) => (what, value, max),
        got => panic!("not refused by a bound: {got:?}"),
    }
}

/// A file past the bound is refused from its length, unread; a device,
/// whose length reads 0, is read only one byte past it.
#[test]
fn a_file_past_the_bound_is_refused_unread() {
    let max = TpkProvider::MAX_FILE_BYTES;
    let dir = std::env::temp_dir().join(format!("unityrs-tpk-bounds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("large.tpk");
    // Sparse: the length is set, nothing is written.
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(max + 1).unwrap();
    drop(file);
    let got = too_large(TpkProvider::open(&path));
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
    assert_eq!(got, ("file", max + 1, max));
    if std::path::Path::new("/dev/zero").exists() {
        assert_eq!(
            too_large(TpkProvider::open("/dev/zero")),
            ("file", max + 1, max)
        );
    }
    // Bytes in memory are held to the same bound.
    let bytes = vec![0u8; usize::try_from(max + 1).unwrap()];
    assert_eq!(
        too_large(TpkProvider::from_bytes(&bytes, &Limits::default())),
        ("file", max + 1, max)
    );
}

/// A file at the bound is read (and here fails as UnityPy fails, on its
/// magic): the bound is the file's length, not one byte less.
#[test]
fn a_file_at_the_bound_is_read() {
    let max = usize::try_from(TpkProvider::MAX_FILE_BYTES).unwrap();
    let mut file = tempfile("at-the-bound.tpk");
    file.1.write_all(&vec![0u8; max]).unwrap();
    let got = TpkProvider::open(&file.0);
    assert!(matches!(got, Err(TpkError::Malformed(_))), "{got:?}");
    drop(file);
}

/// An LZ4 payload that decodes past the bound is refused as it decodes,
/// whatever the header declares; one byte less loads (the blob reads its
/// tables and ignores the zeros after them, as UnityPy does).
#[test]
fn an_lz4_payload_past_the_decoded_bound_is_refused() {
    let max = TpkProvider::MAX_DECODED_BYTES;
    let mut payload = removed_entries(1);
    let fits = usize::try_from(max).unwrap();
    payload.resize(fits, 0);
    let declared = u32::try_from(max + 1).unwrap();
    let provider = TpkProvider::from_bytes(&tpk(1, &payload, declared), &Limits::default());
    assert_eq!(provider.unwrap().class_count(), 1);
    payload.push(0);
    let got = too_large(TpkProvider::from_bytes(
        &tpk(1, &payload, declared),
        &Limits::default(),
    ));
    assert_eq!(got.0, "decoded payload");
    assert_eq!(got.2, max);
}

/// One class with 2,000,000 entries that remove it, in an LZMA TPK under
/// 64 KiB, which rabex's reader kept whole, an entry record per 9 bytes. Its
/// payload (9 bytes an entry) passes the decoded bound; at 65,537 entries,
/// under it, the entries' bound refuses it, and at 65,536 it loads.
#[test]
fn many_entries_are_refused_by_the_payload_or_the_entry_bound() {
    let big = tpk(2, &removed_entries(2_000_000), 0);
    assert!(big.len() < 64 << 10, "{} bytes", big.len());
    let got = too_large(TpkProvider::from_bytes(&big, &Limits::default()));
    assert_eq!(got.0, "decoded payload");
    let max = u32::try_from(TpkProvider::MAX_RECORDS).unwrap();
    let got = too_large(TpkProvider::from_bytes(
        &tpk(2, &removed_entries(max + 1), 0),
        &Limits::default(),
    ));
    assert_eq!(got, ("class entries", u64::from(max) + 1, u64::from(max)));
    let at = TpkProvider::from_bytes(&tpk(2, &removed_entries(max), 0), &Limits::default());
    assert_eq!(at.unwrap().class_count(), 1);
}

/// A file removed when dropped, with its handle.
struct TempFile(std::path::PathBuf, std::fs::File);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn tempfile(name: &str) -> TempFile {
    let path =
        std::env::temp_dir().join(format!("unityrs-tpk-bounds-{}-{name}", std::process::id()));
    let file = std::fs::File::create(&path).unwrap();
    TempFile(path, file)
}
