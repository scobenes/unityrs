//! Regressions from the third and fourth C1 loading reviews.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{path::PathBuf, sync::Arc};
use unityrs_core::{
    Error, File, FileContext, LimitKind, RawStr,
    env::{EnvOptions, Environment, Source},
};
use unityrs_testkit::{scratch::Scratch, zip::archive};

fn limited() -> Environment {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_memory = 80_000;
    ctx.limits.max_input_bytes = 40_000;
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(ctx);
    Environment::new(options)
}

fn independent_sources(folder: bool) {
    let scratch = Scratch::new("independent-input-meters");
    let paths: Vec<_> = ["a", "b"].map(|name| scratch.path().join(name)).into();
    for path in &paths {
        std::fs::write(path, vec![b'x'; 40_000]).unwrap();
        limited().load_files([path]).unwrap();
    }
    let env = limited();
    if folder {
        env.load_folder(scratch.path()).unwrap();
    } else {
        env.load_files(&paths).unwrap();
    }
    assert_eq!(env.files().count(), 2);
    for entry in env.files() {
        let File::Raw(raw) = entry.file else {
            panic!("not raw")
        };
        assert_eq!(raw.data.len(), 40_000);
    }
}

#[test]
fn load_files_limits_each_source_independently() {
    independent_sources(false);
}

#[test]
fn load_folder_limits_each_source_independently() {
    independent_sources(true);
}

#[test]
fn split_deduplication_still_has_a_call_work_bound() {
    let scratch = Scratch::new("split-dedup-meter");
    let path = scratch.path().join("a.split0");
    std::fs::write(&path, b"abc").unwrap();
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_decode_work = 20_000;
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(ctx);
    let env = Environment::new(options);
    let result = env.load_files(std::iter::repeat_n(&path, 2000));
    assert!(matches!(result, Err(Error::Limit(e)) if e.kind() == LimitKind::DecodeWork));
    assert_eq!(env.files().count(), 1);
}

fn central(data: &[u8]) -> usize {
    data.windows(4).position(|w| w == b"PK\x01\x02").unwrap()
}

fn load_zip(data: Vec<u8>) -> Result<Environment, Error> {
    let env = Environment::new(EnvOptions::unitypy());
    env.load_zip_file(Source::Bytes {
        name: RawStr::from("review.zip"),
        data: data.into(),
    })?;
    Ok(env)
}

#[test]
fn loading_zero_size_members_skips_payload_and_crc() {
    for deflate in [false, true] {
        let mut data = archive(&[("empty", b"", deflate)]);
        let c = central(&data);
        data[c + 16..c + 20].copy_from_slice(&123_u32.to_le_bytes());
        if deflate {
            data[35] = 7; // Invalid deflate block, never read by the loader.
        }
        let zip = unityrs_core::zip::ZipArchive::open(
            data.clone().into(),
            &unityrs_core::limits::Meter::unlimited(),
        )
        .unwrap();
        assert!(
            zip.read(0, &unityrs_core::limits::Limits::default())
                .is_err()
        );
        let env = load_zip(data).unwrap();
        let entry = env.files().next().unwrap();
        assert_eq!(entry.name.as_bytes(), b"empty");
        let File::Raw(raw) = entry.file else {
            panic!("not raw")
        };
        assert!(raw.data.is_empty());
    }
}

#[test]
fn loading_zero_size_members_still_checks_open_metadata() {
    let original = archive(&[("empty", b"", false)]);
    let c = central(&original);
    for (at, bytes) in [
        (30, b"other".to_vec()),                 // local name mismatch
        (c + 8, 1_u16.to_le_bytes().to_vec()),   // encrypted
        (c + 10, 99_u16.to_le_bytes().to_vec()), // unsupported method
        (c + 20, 1_u32.to_le_bytes().to_vec()),  // overlapping central directory
    ] {
        let mut data = original.clone();
        data[at..at + bytes.len()].copy_from_slice(&bytes);
        assert!(load_zip(data).is_err(), "metadata at {at}");
    }
}

#[test]
fn explicit_zip_paths_strip_local_protocols_without_joining_splits() {
    let scratch = Scratch::new("explicit-zip-protocol");
    for (filename, member) in [
        ("x.zip", "zip"),
        ("x.split0", "first"),
        ("x.split1", "second"),
    ] {
        std::fs::write(
            scratch.path().join(filename),
            archive(&[(member, b"abc", false)]),
        )
        .unwrap();
    }
    for prefix in ["file:", "file://", "local:", "local://"] {
        for (filename, member) in [("x.zip", "zip"), ("x.split0", "first")] {
            let path = PathBuf::from(format!(
                "{prefix}{}",
                scratch.path().join(filename).display()
            ));
            let env = Environment::new(EnvOptions::unitypy());
            env.load_zip_file(Source::Path(path)).unwrap();
            assert_eq!(
                env.files().map(|e| e.name.to_string()).collect::<Vec<_>>(),
                [member]
            );
        }
    }
}

#[test]
fn direct_split_paths_strip_protocols_but_keep_the_loading_key() {
    let scratch = Scratch::new("split-protocol");
    std::fs::write(scratch.path().join("a.split0"), b"abc").unwrap();
    std::fs::write(scratch.path().join("a.split1"), b"def").unwrap();
    for prefix in ["file:", "file://", "local:", "local://"] {
        let base = format!("{prefix}{}/a", scratch.path().display());
        let env = Environment::new(EnvOptions::unitypy());
        let file = env
            .load_file(Source::Path(format!("{base}.split0").into()))
            .unwrap()
            .unwrap();
        let File::Raw(raw) = file else {
            panic!("not raw")
        };
        assert_eq!(raw.data.as_ref(), b"abcdef");
        assert_eq!(env.files().next().unwrap().name.as_bytes(), base.as_bytes());
    }
}

#[test]
fn deflate_seek_carries_buffered_output_across_three_chunks() {
    // A hex oracle would exceed 100 MB. This synthetic stored-block stream
    // follows CPython 3.11.18 Lib/zipfile.py's ZipExtFile.read/_read1/seek.
    // Its six _read1 requests are 16777216, 1280, 16774405, 1275,
    // 16774400, 1275. read() carries 2811 then 2816 buffered bytes into
    // the next seek chunk. Losing those bytes reaches the invalid block.
    const SIZE: usize = 3 * (1 << 24) + 123;
    let plain = vec![b'a'; SIZE + 2700];
    let mut payload = Vec::new();
    for block in plain.chunks(65535) {
        let n = u16::try_from(block.len()).unwrap();
        payload.push(0);
        payload.extend(n.to_le_bytes());
        payload.extend((!n).to_le_bytes());
        payload.extend(block);
    }
    payload.push(7); // Invalid BTYPE, beyond CPython's last output window.
    let mut data = archive(&[("x", b"", true)]);
    let old = central(&data);
    data.splice(31..old, payload.iter().copied());
    let c = 31 + payload.len();
    let mut crc = flate2::Crc::new();
    crc.update(&plain[..SIZE]);
    data[c + 16..c + 20].copy_from_slice(&crc.sum().to_le_bytes());
    data[c + 20..c + 24].copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    data[c + 24..c + 28].copy_from_slice(&u32::try_from(SIZE).unwrap().to_le_bytes());
    let end = data.len() - 22;
    data[end + 16..end + 20].copy_from_slice(&u32::try_from(c).unwrap().to_le_bytes());
    let env = load_zip(data).unwrap();
    let File::Raw(raw) = env.files().next().unwrap().file else {
        panic!("not raw")
    };
    assert_eq!(raw.data.as_ref(), &plain[..SIZE]);
}
