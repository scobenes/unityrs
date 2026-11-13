use std::sync::Arc;
use unityrs_batch::{
    phase::{Account, Error},
    sort::{self, Order},
    spool::{Counter, Host, workspace::Workspace},
};
use unityrs_testkit::scratch::Scratch;

struct Bytes;
impl Order for Bytes {
    fn tag(&self) -> u32 {
        17
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        a.cmp(b)
    }
}
fn header() -> Vec<u8> {
    let mut out = b"URSSORT\0".to_vec();
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&17_u32.to_le_bytes());
    out
}
#[test]
fn version_tag_lengths_truncation_and_out_of_order_frames_are_internal() {
    let scratch = Scratch::new("sort-format");
    let mut cases = vec![Vec::new(), b"URSSORT\0".to_vec()];
    for (offset, byte) in [(0, b'x'), (8, 2), (12, 18)] {
        let mut h = header();
        h[offset] = byte;
        cases.push(h);
    }
    for length in [0_u32, 3, 5, 65_537, u32::MAX] {
        let mut h = header();
        h.extend_from_slice(&length.to_le_bytes());
        cases.push(h);
    }
    let mut partial = header();
    partial.extend_from_slice(&[4, 0]);
    cases.push(partial);
    let mut unsorted = header();
    unsorted.extend_from_slice(&[5, 0, 0, 0, b'z', 5, 0, 0, 0, b'a']);
    cases.push(unsorted);
    for (index, bytes) in cases.iter().enumerate() {
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let mut workspace =
            Workspace::create(scratch.join(index.to_string()), counter, Arc::new(Host)).unwrap();
        let mut out = workspace.create_file(&|| false).unwrap();
        out.append(bytes, &|| false).unwrap();
        let file = out.seal(&|| false).unwrap();
        let account = Account::new(64 << 20);
        let result = (|| {
            let mut reader =
                sort::Reader::open(&workspace, &file, &Bytes, account.meter(), &|| false)?;
            while reader.next(&|| false)?.is_some() {}
            Ok::<_, Error>(())
        })();
        assert_eq!(result, Err(Error::internal()), "{index}");
        workspace.retire(file, &|| false).unwrap();
        assert_eq!(account.snapshot().input.used, 0);
    }
}
#[test]
fn maximum_frame_and_empty_payload_have_exact_little_endian_framing() {
    struct Source(bool);
    impl sort::Source for Source {
        fn next(&mut self, out: &mut sort::Frame, _: &dyn Fn() -> bool) -> Result<bool, Error> {
            if self.0 {
                return Ok(false);
            }
            self.0 = true;
            out.set(&vec![b'x'; sort::format::MAX_PAYLOAD])?;
            Ok(true)
        }
    }
    let scratch = Scratch::new("sort-max-frame");
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
    let account = Account::new(64 << 20);
    let report = sort::sort(
        &mut Source(false),
        &Bytes,
        &mut workspace,
        &account,
        sort::Options::default(),
        &|| false,
        &sort::NoHooks,
    )
    .unwrap();
    let file = report.file.unwrap();
    assert_eq!(file.length(), 16 + 65_536);
    let mut raw = workspace.open(&file, &|| false).unwrap();
    let mut h = [0; 20];
    std::io::Read::read_exact(&mut raw, &mut h).unwrap();
    assert_eq!(&h[..16], header());
    assert_eq!(&h[16..], &[0, 0, 1, 0]);
    drop(raw);
    workspace.retire(file, &|| false).unwrap();

    let mut out = workspace.create_file(&|| false).unwrap();
    out.append(&header(), &|| false).unwrap();
    out.append(&4_u32.to_le_bytes(), &|| false).unwrap();
    let file = out.seal(&|| false).unwrap();
    let mut reader =
        sort::Reader::open(&workspace, &file, &Bytes, account.meter(), &|| false).unwrap();
    assert_eq!(reader.next(&|| false).unwrap(), Some(&[][..]));
    assert_eq!(reader.next(&|| false).unwrap(), None);
}
#[test]
fn descriptors_cannot_cross_workspaces_and_failed_writers_keep_aggregate_claims() {
    let scratch = Scratch::new("workspace-capabilities");
    let counter = Counter::new(4096, 0, 4096).unwrap();
    let mut a = Workspace::create(scratch.join("a"), counter.clone(), Arc::new(Host)).unwrap();
    let b = Workspace::create(scratch.join("b"), counter.clone(), Arc::new(Host)).unwrap();
    let mut writer = a.create_file(&|| false).unwrap();
    writer.append(b"one", &|| false).unwrap();
    let sealed = writer.seal(&|| false).unwrap();
    assert!(matches!(
        b.open(&sealed, &|| false),
        Err(unityrs_batch::spool::Error::Internal)
    ));
    let mut failed = a.create_file(&|| false).unwrap();
    assert!(failed.append(b"two", &|| false).is_err());
    assert_eq!(a.claimed(), 4096);
    drop(failed);
    a.retire(sealed, &|| false).unwrap();
    assert_eq!(counter.used(), 0);
}

#[test]
fn a_corrupt_single_run_catalog_cannot_return_a_ready_descriptor() {
    struct Corrupt;
    impl unityrs_batch::spool::FileSystem for Corrupt {
        fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn open(&self, path: &std::path::Path) -> std::io::Result<Box<dyn std::io::Read + Send>> {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(
                &mut unityrs_batch::spool::FileSystem::open(&Host, path)?,
                &mut bytes,
            )?;
            if bytes.get(12..16) == Some(&u32::MAX.to_le_bytes()) {
                // Wrong workspace identity in the first sealed descriptor.
                bytes[16..24].fill(0);
            }
            Ok(Box::new(std::io::Cursor::new(bytes)))
        }
    }
    struct One(bool);
    impl sort::Source for One {
        fn next(&mut self, out: &mut sort::Frame, _: &dyn Fn() -> bool) -> Result<bool, Error> {
            if self.0 {
                return Ok(false);
            }
            self.0 = true;
            out.set(b"one")?;
            Ok(true)
        }
    }
    let scratch = Scratch::new("sort-corrupt-catalog");
    let mut workspace = Workspace::create(
        scratch.join("w"),
        Counter::new(u64::MAX, 0, 4096).unwrap(),
        Arc::new(Corrupt),
    )
    .unwrap();
    let result = sort::sort(
        &mut One(false),
        &Bytes,
        &mut workspace,
        &Account::new(64 << 20),
        sort::Options::default(),
        &|| false,
        &sort::NoHooks,
    );
    assert_eq!(result.unwrap_err(), Error::internal());
}

#[test]
fn truncated_prefix_is_internal_before_a_failing_read() {
    struct PrefixFault;
    struct HeaderOnly(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for HeaderOnly {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if self.0.position() >= 16 {
                return Err(std::io::Error::from_raw_os_error(libc::EIO));
            }
            std::io::Read::read(&mut self.0, bytes)
        }
    }
    impl unityrs_batch::spool::FileSystem for PrefixFault {
        fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn open(&self, _: &std::path::Path) -> std::io::Result<Box<dyn std::io::Read + Send>> {
            Ok(Box::new(HeaderOnly(std::io::Cursor::new(header()))))
        }
    }
    let scratch = Scratch::new("sort-prefix-fault");
    for remaining in 1..4 {
        let mut workspace = Workspace::create(
            scratch.join(remaining.to_string()),
            Counter::new(u64::MAX, 0, 4096).unwrap(),
            Arc::new(PrefixFault),
        )
        .unwrap();
        let mut out = workspace.create_file(&|| false).unwrap();
        out.append(&header(), &|| false).unwrap();
        out.append(&vec![0; remaining], &|| false).unwrap();
        let file = out.seal(&|| false).unwrap();
        let account = Account::new(64 << 20);
        let mut reader =
            sort::Reader::open(&workspace, &file, &Bytes, account.meter(), &|| false).unwrap();
        assert_eq!(reader.next(&|| false).unwrap_err(), Error::internal());
        drop(reader);
        workspace.retire(file, &|| false).unwrap();
    }
}
