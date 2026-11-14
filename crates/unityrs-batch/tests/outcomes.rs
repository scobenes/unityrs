use std::{
    io,
    sync::{Arc, Mutex},
};
use unityrs_batch::{
    outcomes::{self, Archive, Code, Outcomes},
    spool::{self, Counter, random::Random},
};
use unityrs_testkit::scratch::Scratch;
#[derive(Default)]
struct Memory {
    bytes: Mutex<Vec<u8>>,
    writes: Mutex<Vec<u64>>,
    fail: Mutex<Option<u64>>,
    max_write: Mutex<usize>,
    space: Mutex<Option<u64>>,
}
#[derive(Clone, Default)]
struct Shared(Arc<Memory>);
impl std::ops::Deref for Shared {
    type Target = Memory;
    fn deref(&self) -> &Memory {
        &self.0
    }
}
impl Random for Shared {
    fn available(&self) -> io::Result<u64> {
        Ok(self.space.lock().unwrap().unwrap_or(u64::MAX))
    }
    fn read_at(&self, b: &mut [u8], offset: u64) -> io::Result<usize> {
        let bytes = self.bytes.lock().unwrap();
        let src = bytes
            .get(usize::try_from(offset).unwrap()..)
            .unwrap_or_default();
        let n = src.len().min(b.len());
        b[..n].copy_from_slice(&src[..n]);
        Ok(n)
    }
    fn write_at(&self, b: &[u8], offset: u64) -> io::Result<usize> {
        self.writes.lock().unwrap().push(offset);
        if *self.fail.lock().unwrap() == Some(offset) {
            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
        }
        let maximum = *self.max_write.lock().unwrap();
        let b = if maximum == 0 {
            b
        } else {
            &b[..b.len().min(maximum)]
        };
        let mut bytes = self.bytes.lock().unwrap();
        let at = usize::try_from(offset).unwrap();
        let length = (at + b.len()).max(bytes.len());
        bytes.resize(length, 0);
        bytes[at..at + b.len()].copy_from_slice(b);
        Ok(b.len())
    }
}
struct Fs {
    loose: Shared,
    archives: Shared,
}
impl spool::FileSystem for Fs {
    fn available(&self, _: &std::path::Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn create_random(&self, p: &std::path::Path) -> io::Result<Box<dyn Random>> {
        Ok(Box::new(if p.ends_with("outcomes") {
            self.loose.clone()
        } else {
            self.archives.clone()
        }))
    }
}
#[test]
fn exact_layout_and_metadata_first_status_last() {
    let s = Scratch::new("outcome-layout");
    let fs = Fs {
        loose: Shared::default(),
        archives: Shared::default(),
    };
    let counter = Counter::new(8192, 0, 4096).unwrap();
    let out = Outcomes::create(s.path(), 3, 2, &counter, &fs, &[0; 128], &|| false).unwrap();
    assert_eq!(counter.used(), 8192);
    for (i, code) in [Code::Written, Code::Present, Code::PublishFailed]
        .into_iter()
        .enumerate()
    {
        out.set(u64::try_from(i).unwrap(), code).unwrap();
        assert_eq!(out.loose(u64::try_from(i).unwrap()).unwrap(), code);
    }
    let record =
        Archive::success(Code::Written, 0x0807_0605_0403_0201, [7; 16], Some("abc")).unwrap();
    fs.archives.writes.lock().unwrap().clear();
    out.set_archive(1, &record).unwrap();
    assert_eq!(*fs.archives.writes.lock().unwrap(), [129, 128]);
    let bytes = fs.archives.bytes.lock().unwrap();
    assert_eq!(bytes[128], 1);
    assert_eq!(bytes[129], 3);
    assert_eq!(&bytes[130..138], &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(&bytes[138..154], &[7; 16]);
    assert_eq!(&bytes[154..157], b"abc");
    assert!(bytes[157..256].iter().all(|b| *b == 0));
    drop(bytes);
    assert_eq!(out.archive(1).unwrap(), record);
    assert_eq!(counter.used(), 8192);
    assert!(out.set(3, Code::Written).is_err());
    assert!(out.set(0, Code::ArchiveTooLarge).is_err());
    *fs.archives.fail.lock().unwrap() = Some(1);
    assert!(out.set_archive(0, &record).is_err());
    assert_eq!(out.archive(0).unwrap().code, Code::NotWritten);
}
#[test]
fn every_short_archive_is_unknown_and_corruption_is_not_silenced() {
    let mem = Shared::default();
    let record = Archive::success(Code::Present, 42, [1; 16], None).unwrap();
    let full = outcomes::encode(&record).unwrap();
    for n in 0..128 {
        *mem.bytes.lock().unwrap() = full[..n].to_vec();
        assert_eq!(
            outcomes::read_archive(Some(&mem), 0).unwrap().code,
            Code::NotWritten,
            "{n}"
        );
    }
    assert_eq!(
        outcomes::read_archive(None, u64::MAX).unwrap().code,
        Code::NotWritten
    );
    assert!(outcomes::read_archive(Some(&mem), u64::MAX).is_err());
    *mem.bytes.lock().unwrap() = full.to_vec();
    mem.bytes.lock().unwrap()[0] = 7;
    assert!(outcomes::read_archive(Some(&mem), 0).is_err());
    assert_eq!(outcomes::read_loose(None, 0).unwrap(), Code::NotWritten);
    *mem.bytes.lock().unwrap() = vec![6];
    assert!(outcomes::read_loose(Some(&mem), 0).is_err());
}

#[test]
fn partial_metadata_and_failed_verification_preserve_conservative_facts() {
    let scratch = Scratch::new("archive-update");
    let fs = Fs {
        loose: Shared::default(),
        archives: Shared::default(),
    };
    let counter = Counter::new(8192, 0, 4096).unwrap();
    let out = Outcomes::create(scratch.path(), 0, 2, &counter, &fs, &[0; 128], &|| false).unwrap();
    let success = Archive::success(Code::Written, 777, [9; 16], None).unwrap();
    *fs.archives.max_write.lock().unwrap() = 3;
    *fs.archives.fail.lock().unwrap() = Some(4);
    assert!(out.set_archive(0, &success).is_err());
    assert_eq!(out.archive(0).unwrap().code, Code::NotWritten);
    *fs.archives.fail.lock().unwrap() = None;
    out.set_archive(1, &success).unwrap();
    *fs.archives.fail.lock().unwrap() = Some(128);
    assert!(
        out.set_archive(1, &Archive::failed(Code::PublishMismatch).unwrap())
            .is_err()
    );
    assert_eq!(out.archive(1).unwrap(), success);
    *fs.archives.fail.lock().unwrap() = None;
    assert!(
        out.set_archive(
            1,
            &Archive::success(Code::Present, 1, [0; 16], None).unwrap()
        )
        .is_err()
    );
    assert_eq!(out.archive(1).unwrap(), success);
    out.set_archive(1, &Archive::failed(Code::PublishMismatch).unwrap())
        .unwrap();
    assert_eq!(out.archive(1).unwrap().code, Code::PublishMismatch);
}
#[test]
fn archive_validation_covers_reserved_bytes_padding_length_and_utf8() {
    let mem = Shared::default();
    let archive = Archive::success(Code::Present, 99, [2; 16], Some("tag é")).unwrap();
    let bytes = outcomes::encode(&archive).unwrap();
    *mem.bytes.lock().unwrap() = bytes.to_vec();
    assert_eq!(outcomes::read_archive(Some(&mem), 0).unwrap(), archive);
    for (offset, value) in [(127, 1), (125, 1), (1, 101), (26, 255)] {
        *mem.bytes.lock().unwrap() = bytes.to_vec();
        mem.bytes.lock().unwrap()[offset] = value;
        assert!(outcomes::read_archive(Some(&mem), 0).is_err(), "{offset}");
    }
}
#[test]
fn positional_updates_recheck_reserve_without_claiming_the_extent_again() {
    let scratch = Scratch::new("outcome-reserve");
    let fs = Fs {
        loose: Shared::default(),
        archives: Shared::default(),
    };
    let counter = Counter::new(4096, 100, 4096).unwrap();
    let out = Outcomes::create(scratch.path(), 1, 0, &counter, &fs, &[0; 128], &|| false).unwrap();
    *fs.loose.space.lock().unwrap() = Some(99);
    assert!(out.set(0, Code::Written).is_err());
    assert_eq!(out.loose(0).unwrap(), Code::NotWritten);
    assert_eq!(counter.used(), 4096);
    *fs.loose.space.lock().unwrap() = Some(100);
    out.set(0, Code::Written).unwrap();
    assert_eq!(out.loose(0).unwrap(), Code::Written);
    assert_eq!(counter.used(), 4096);
}

#[test]
fn not_written_cannot_reset_an_archive_success() {
    let scratch = Scratch::new("archive-reset");
    let fs = Fs {
        loose: Shared::default(),
        archives: Shared::default(),
    };
    let counter = Counter::new(8192, 0, 4096).unwrap();
    let out = Outcomes::create(scratch.path(), 0, 1, &counter, &fs, &[0; 128], &|| false).unwrap();
    let success = Archive::success(Code::Written, 42, [1; 16], None).unwrap();
    out.set_archive(0, &success).unwrap();
    assert!(out.set_archive(0, &Archive::default()).is_err());
    assert_eq!(out.archive(0).unwrap(), success);
    assert!(out.set_archive(0, &success).is_err());
}
#[test]
fn quoted_and_nul_etags_are_internal_on_write_and_read() {
    let mem = Shared::default();
    let valid =
        outcomes::encode(&Archive::success(Code::Written, 1, [0; 16], Some("tag")).unwrap())
            .unwrap();
    for byte in [b'"', 0] {
        let mut bytes = valid;
        bytes[27] = byte;
        *mem.bytes.lock().unwrap() = bytes.to_vec();
        assert!(
            outcomes::read_archive(Some(&mem), 0).is_err(),
            "read {byte}"
        );
        assert!(
            Archive::success(
                Code::Present,
                1,
                [0; 16],
                Some(if byte == 0 { "t\0g" } else { "t\"g" })
            )
            .is_err(),
            "write {byte}"
        );
    }
}

#[test]
fn failed_archive_constructor_accepts_only_failure_codes() {
    for code in [Code::NotWritten, Code::Written, Code::Present] {
        assert!(Archive::failed(code).is_err(), "{code:?}");
    }
    for code in [
        Code::PublishFailed,
        Code::PublishMismatch,
        Code::CredentialsExpired,
        Code::ArchiveTooLarge,
    ] {
        assert_eq!(Archive::failed(code).unwrap().code, code);
    }
}

/// The stopped finisher's view: a file whose zero-writes never ran, or ran
/// short, reads `not_written` past its end; what was written reads back.
#[test]
fn a_missing_or_short_outcome_file_reads_not_written() {
    use outcomes::Read;
    let scratch = Scratch::new("outcome-view");
    let view = outcomes::View::open(scratch.path(), 3, 2, &spool::Host).unwrap();
    for index in 0..3 {
        assert_eq!(view.loose(index).unwrap(), Code::NotWritten);
    }
    assert_eq!(view.archive(1).unwrap(), Archive::default());
    assert!(view.loose(3).is_err() && view.archive(2).is_err());

    let counter = Counter::new(1 << 20, 0, 4096).unwrap();
    let out = Outcomes::create(
        scratch.path(),
        3,
        2,
        &counter,
        &spool::Host,
        &[0; 128],
        &|| false,
    )
    .unwrap();
    out.set(0, Code::Written).unwrap();
    out.set(1, Code::Present).unwrap();
    drop(out);
    // Cut both files short: the loose file after its second record, the
    // archive file inside its first.
    let cut = |name: &str, len: u64| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(scratch.join(name))
            .unwrap()
            .set_len(len)
            .unwrap();
    };
    cut("outcomes", 2);
    cut("archive_outcomes", 64);
    let view = outcomes::View::open(scratch.path(), 3, 2, &spool::Host).unwrap();
    assert_eq!(view.loose(0).unwrap(), Code::Written);
    assert_eq!(view.loose(1).unwrap(), Code::Present);
    assert_eq!(view.loose(2).unwrap(), Code::NotWritten);
    assert_eq!(view.archive(0).unwrap(), Archive::default());
    assert_eq!(view.archive(1).unwrap(), Archive::default());
}
