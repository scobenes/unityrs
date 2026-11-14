//! Synthetic split and ZIP loading; oracle replay is in `loading_oracle.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::{Bytes, File, RawStr};
use unityrs_testkit::scratch::Scratch;

#[test]
fn a_split_path_loads_once_under_its_base_and_cab_name() {
    let s = Scratch::new("split-loading");
    let base = s.path().join("CAB-A");
    std::fs::write(base.with_file_name("CAB-A.split0"), b"abc").unwrap();
    std::fs::write(base.with_file_name("CAB-A.split1"), b"def").unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    let file = env
        .load(Source::Path(base.with_file_name("CAB-A.split1")))
        .unwrap();
    let File::Raw(raw) = file else {
        panic!("not raw")
    };
    assert_eq!(raw.data.as_ref(), b"abcdef");
    assert_eq!(
        env.files().next().unwrap().name.as_bytes(),
        base.as_os_str().as_encoded_bytes()
    );
    assert!(env.registry().keys().any(|k| k.as_bytes() == b"cab-a"));
}

#[test]
fn zip_members_load_under_their_names() {
    let env = Environment::new(EnvOptions::unitypy());
    let data = unityrs_testkit::zip::archive(&[("dir/", b"", false), ("dir/CAB-X", b"abc", true)]);
    env.load_zip_file(Source::Bytes {
        name: RawStr::from("a.zip"),
        data: Bytes::from(data),
    })
    .unwrap();
    assert_eq!(
        env.files().map(|e| e.name.to_string()).collect::<Vec<_>>(),
        ["dir/", "dir/CAB-X"]
    );
    assert!(env.registry().keys().any(|k| k.as_bytes().is_empty()));
    assert!(env.registry().keys().any(|k| k.as_bytes() == b"cab-x"));
}

fn raw(env: &Environment, path: std::path::PathBuf) -> Vec<u8> {
    let File::Raw(r) = env.load(Source::Path(path)).unwrap() else {
        panic!("not raw")
    };
    r.data.to_vec()
}

#[test]
fn split_rules_pin_gaps_missing_zero_and_terminal_suffixes() {
    let s = Scratch::new("split-rules");
    let env = Environment::new(EnvOptions::unitypy());
    std::fs::write(s.path().join("x.split1"), b"one").unwrap();
    std::fs::write(s.path().join("x.split3"), b"three").unwrap();
    assert_eq!(raw(&env, s.path().join("x.split9")), b"one");
    assert_eq!(raw(&env, s.path().join("absent.split0")), b"");
    std::fs::write(s.path().join("x.split0.bak"), b"backup").unwrap();
    assert_eq!(raw(&env, s.path().join("x.split0.bak")), b"backup");
    std::fs::create_dir(s.path().join("folder.split1")).unwrap();
    std::fs::write(s.path().join("folder.split1/inner"), b"inner").unwrap();
    assert_eq!(raw(&env, s.path().join("folder.split1/inner")), b"inner");
    assert!(
        env.load(Source::Path(s.path().join("folder.split1")))
            .is_err()
    );
    std::fs::write(s.path().join("unicode.split0"), b"decimal").unwrap();
    assert_eq!(raw(&env, s.path().join("unicode.split١")), b"decimal");
    std::fs::write(s.path().join("unicode.split²"), b"numeric").unwrap();
    assert_eq!(raw(&env, s.path().join("unicode.split²")), b"numeric");
    std::fs::create_dir(s.path().join("line\nbreak")).unwrap();
    let p = s.path().join("line\nbreak/x.split0");
    std::fs::write(&p, b"fragment").unwrap();
    let File::Raw(r) = env.load(Source::Path(p.clone())).unwrap() else {
        panic!("not raw")
    };
    assert_eq!(r.name.as_bytes(), p.as_os_str().as_encoded_bytes());
}

#[test]
fn split_loop_stops_before_part_999_and_load_files_deduplicates() {
    let s = Scratch::new("split-limit");
    for i in 0..1000 {
        std::fs::write(s.path().join(format!("x.split{i}")), b"x").unwrap();
    }
    let env = Environment::new(EnvOptions::unitypy());
    assert_eq!(raw(&env, s.path().join("x.split0")).len(), 999);
    let env = Environment::new(EnvOptions::unitypy());
    env.load_files([s.path().join("x.split0"), s.path().join("x.split9")])
        .unwrap();
    assert_eq!(env.stats().loaded, 1);
}

#[test]
fn constructor_suffix_dispatch_is_separate_from_byte_detection() {
    let s = Scratch::new("zip-dispatch");
    let mut data = unityrs_testkit::zip::archive(&[("CAB-X", b"abc", false)]);
    data[4] = 20;
    for name in ["a.zip", "a.apk", "a.ZIP", "a.bin"] {
        let path = s.path().join(name);
        std::fs::write(&path, &data).unwrap();
        let env = Environment::new(EnvOptions::unitypy());
        env.load_path(&path).unwrap();
        let key = env.files().next().unwrap().name;
        if matches!(name, "a.zip" | "a.apk") {
            assert_eq!(key.as_bytes(), b"CAB-X");
        } else {
            assert_eq!(key.as_bytes(), path.as_os_str().as_encoded_bytes());
        }
    }
    data[4] = 0;
    let path = s.path().join("detected.bin");
    std::fs::write(&path, data).unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    env.load_path(&path).unwrap();
    assert_eq!(env.files().next().unwrap().name.as_bytes(), b"CAB-X");
}

#[test]
fn archive_splits_use_members_and_first_position_with_last_duplicate_bytes() {
    let env = Environment::new(EnvOptions::unitypy());
    let data = unityrs_testkit::zip::archive(&[
        ("sub/CAB-X.split1", b"def", true),
        ("sub/CAB-X.split0", b"abc", false),
        ("sub/CAB-X.split0", b"ABC", true),
        ("raw", b"one", false),
        ("raw", b"two", false),
    ]);
    env.load_zip_file(Source::Bytes {
        name: RawStr::from("z"),
        data: data.into(),
    })
    .unwrap();
    let entries: Vec<_> = env.files().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name.as_bytes(), b"sub/CAB-X");
    let File::Raw(r) = &entries[0].file else {
        panic!("not raw")
    };
    assert_eq!(&r.data[..], b"ABCdef");
    let File::Raw(r) = &entries[1].file else {
        panic!("not raw")
    };
    assert_eq!(&r.data[..], b"two");
}

fn meter() -> std::sync::Arc<unityrs_core::limits::Meter> {
    unityrs_core::limits::Meter::for_input(
        &unityrs_core::limits::Limits::default(),
        std::sync::Arc::new(unityrs_core::budget::Unbounded),
        unityrs_core::budget::Reservation::Input,
    )
}

#[test]
fn malformed_archives_are_refused() {
    let original = unityrs_testkit::zip::archive(&[("a", b"abc", false), ("b", b"def", true)]);
    let central = original
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .unwrap();
    let end = original.len() - 22;
    let cases: Vec<(usize, Vec<u8>)> = vec![
        (end + 16, u32::MAX.to_le_bytes().to_vec()), // ZIP64 sentinel
        (
            end + 16,
            (u32::try_from(end).unwrap() + 100).to_le_bytes().to_vec(),
        ), // directory past EOF
        (central + 8, 1_u16.to_le_bytes().to_vec()), // encrypted
        (central + 10, 12_u16.to_le_bytes().to_vec()), // bzip2
        (central + 42, 0xffff_0000_u32.to_le_bytes().to_vec()), // bad local offset
        (central + 47 + 42, 0_u32.to_le_bytes().to_vec()), // overlapping local header
        (central + 20, 10000_u32.to_le_bytes().to_vec()), // size past archive
    ];
    for (at, value) in cases {
        let mut bytes = original.clone();
        bytes[at..at + value.len()].copy_from_slice(&value);
        let env = Environment::new(EnvOptions::unitypy());
        assert!(
            env.load_zip_file(Source::Bytes {
                name: RawStr::from("z"),
                data: bytes.into()
            })
            .is_err(),
            "offset {at}"
        );
    }
    for n in 0..original.len() {
        assert!(
            unityrs_core::zip::ZipArchive::open(Bytes::copy_from_slice(&original[..n]), &meter())
                .is_err(),
            "truncated {n}"
        );
    }
}

#[test]
fn crc_sizes_truncation_bombs_and_memory_limits_are_checked() {
    use unityrs_core::limits::Limits;
    use unityrs_core::{Error, LimitKind};
    let original = unityrs_testkit::zip::archive(&[("a", b"abc", true)]);
    let central = original
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .unwrap();
    for (local, center, value) in [(14, 16, 0_u32), (22, 24, 2)] {
        let mut bytes = original.clone();
        bytes[local..local + 4].copy_from_slice(&value.to_le_bytes());
        bytes[central + center..central + center + 4].copy_from_slice(&value.to_le_bytes());
        let m = meter();
        let zip = unityrs_core::zip::ZipArchive::open(bytes.into(), &m).unwrap();
        assert!(zip.read(0, &Limits::default()).is_err());
        drop(zip);
        assert_eq!(m.used(), 0);
    }
    let m = meter();
    let zip = unityrs_core::zip::ZipArchive::open(original.into(), &m).unwrap();
    let mut limits = Limits::default();
    limits.max_inflate = 2;
    assert!(matches!(zip.read(0, &limits), Err(Error::Limit(e)) if e.kind() == LimitKind::Inflate));
    drop(zip);
    assert_eq!(m.used(), 0);
    let pool = std::sync::Arc::new(unityrs_core::budget::Counting::new(0));
    let mut ctx = unityrs_core::FileContext::unitypy();
    ctx.budget = pool.clone();
    let mut options = EnvOptions::unitypy();
    options.context = std::sync::Arc::new(ctx);
    let env = Environment::new(options);
    assert!(
        env.load_zip_file(Source::Bytes {
            name: RawStr::from("z"),
            data: unityrs_testkit::zip::archive(&[("a", b"abc", true)]).into()
        })
        .is_err()
    );
    assert_eq!(pool.used(), 0);
}

#[test]
fn joined_source_is_limited_as_one_input_and_folder_loads_it_once() {
    let s = Scratch::new("split-folder");
    std::fs::write(s.path().join("a.split0"), b"abc").unwrap();
    std::fs::write(s.path().join("a.split1"), b"def").unwrap();
    let mut ctx = unityrs_core::FileContext::unitypy();
    ctx.limits.max_input_bytes = 5;
    let mut options = EnvOptions::unitypy();
    options.context = std::sync::Arc::new(ctx);
    let env = Environment::new(options);
    assert!(
        matches!(env.load(Source::Path(s.path().join("a.split0"))), Err(unityrs_core::Error::Limit(e)) if e.kind() == unityrs_core::LimitKind::InputBytes)
    );
    assert_eq!(env.files().count(), 0);
    let data = unityrs_testkit::zip::archive(&[("CAB-Z", b"zip", true)]);
    std::fs::write(s.path().join("archive"), data).unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    env.load_folder(s.path()).unwrap();
    assert_eq!(env.files().count(), 2);
    assert!(env.registry().keys().any(|k| k.as_bytes() == b"cab-z"));
}

#[test]
fn zip_member_detection_charges_nested_gzip_work() {
    use std::io::Write;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&vec![0; 200_000]).unwrap();
    let gzip = gzip.finish().unwrap();
    let data = unityrs_testkit::zip::archive(&[("gzip", &gzip, false)]);
    let mut ctx = unityrs_core::FileContext::unitypy();
    ctx.limits.max_decode_work = 10_000;
    let mut options = EnvOptions::unitypy();
    options.context = std::sync::Arc::new(ctx);
    let env = Environment::new(options);
    assert!(
        env.load_zip_file(Source::Bytes {
            name: RawStr::from("z"),
            data: data.into()
        })
        .is_err()
    );
    assert_eq!(env.files().count(), 0);
}

#[test]
fn zip_depth_continues_through_bundles_and_root_depth_is_zero() {
    let asset =
        unityrs_testkit::env::serialized(&[], &[unityrs_testkit::env::Obj::named(1, "s", &[])]);
    let inner = unityrs_testkit::env::bundle(&[("CAB-A", asset)]);
    let outer = unityrs_testkit::env::bundle(&[("inner", inner)]);
    let data = unityrs_testkit::zip::archive(&[("outer", &outer, false)]);
    let mut ctx = unityrs_core::FileContext::unitypy();
    ctx.limits.max_container_depth = 1;
    let mut options = EnvOptions::unitypy();
    options.context = std::sync::Arc::new(ctx);
    let env = Environment::new(options);
    assert!(
        env.load_zip_file(Source::Bytes {
            name: RawStr::from("z"),
            data: data.into()
        })
        .is_err()
    );
    let mut ctx = unityrs_core::FileContext::unitypy();
    ctx.limits.max_container_depth = 0;
    let mut options = EnvOptions::unitypy();
    options.context = std::sync::Arc::new(ctx);
    let env = Environment::new(options);
    env.load_zip_file(Source::Bytes {
        name: RawStr::from("z"),
        data: unityrs_testkit::zip::archive(&[("raw", b"abc", false)]).into(),
    })
    .unwrap();
}

#[test]
fn zip_member_reader_is_stream_and_names_decode_utf8_or_cp437() {
    let mut data = unityrs_testkit::zip::archive(&[("x", &[b'A'; 20], false)]);
    let center = data.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    data[30] = 0x82;
    data[center + 46] = 0x82;
    data[6..8].copy_from_slice(&0_u16.to_le_bytes());
    data[center + 8..center + 10].copy_from_slice(&0_u16.to_le_bytes());
    let env = Environment::new(EnvOptions::unitypy());
    env.load_zip_file(Source::Bytes {
        name: RawStr::from("z"),
        data: data.clone().into(),
    })
    .unwrap();
    assert_eq!(env.files().next().unwrap().name.to_str(), Some("é"));
    data[6..8].copy_from_slice(&0x800_u16.to_le_bytes());
    data[center + 8..center + 10].copy_from_slice(&0x800_u16.to_le_bytes());
    assert!(unityrs_core::zip::ZipArchive::open(data.into(), &meter()).is_err());
}

#[test]
fn data_descriptors_empty_archives_zip64_extras_and_real_overlaps() {
    use unityrs_core::zip::ZipArchive;
    let original = unityrs_testkit::zip::archive(&[("a", b"abc", true)]);
    let center = original
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .unwrap();
    let mut descriptor = b"PK\x07\x08".to_vec();
    descriptor.extend(&original[14..26]);
    for signed in [true, false] {
        let descriptor = if signed {
            &descriptor[..]
        } else {
            &descriptor[4..]
        };
        let n = descriptor.len();
        let mut data = original.clone();
        data.splice(center..center, descriptor.iter().copied());
        data[6..8].copy_from_slice(&0x808_u16.to_le_bytes());
        data[14..26].fill(0);
        data[center + n + 8..center + n + 10].copy_from_slice(&0x808_u16.to_le_bytes());
        let end = data.len() - 22;
        data[end + 16..end + 20].copy_from_slice(&u32::try_from(center + n).unwrap().to_le_bytes());
        let zip = ZipArchive::open(data.into(), &meter()).unwrap();
        assert_eq!(
            &zip.read(0, &unityrs_core::limits::Limits::default())
                .unwrap()[..],
            b"abc"
        );
    }
    assert_eq!(
        ZipArchive::open(unityrs_testkit::zip::archive(&[]).into(), &meter())
            .unwrap()
            .members()
            .len(),
        0
    );
    let mut data = original;
    data.splice(center + 47..center + 47, [1, 0, 0, 0]);
    data[center + 30..center + 32].copy_from_slice(&4_u16.to_le_bytes());
    let end = data.len() - 22;
    let size = u32::try_from(end - center).unwrap();
    data[end + 12..end + 16].copy_from_slice(&size.to_le_bytes());
    assert!(ZipArchive::open(data.into(), &meter()).is_ok());
    let mut data = unityrs_testkit::zip::archive(&[("a", b"abc", false), ("a", b"abc", false)]);
    let center = data.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    data[center + 47 + 42..center + 47 + 46].copy_from_slice(&0_u32.to_le_bytes());
    let zip = ZipArchive::open(data.into(), &meter()).unwrap();
    let err = zip
        .read(1, &unityrs_core::limits::Limits::default())
        .unwrap_err();
    assert!(err.to_string().contains("overlapping"));
}
