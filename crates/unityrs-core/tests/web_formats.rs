//! Legacy bundles, WebFile member loading, signed reads and bounded inflation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::limits::Meter;
use unityrs_core::options::FileContext;
use unityrs_core::{BundleFile, Bytes, Error, File, LimitKind, RawStr, WebFile};
use unityrs_testkit::web;
fn setup() -> (Arc<FileContext>, Arc<Meter>) {
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::new(
        ctx.limits.max_input_memory,
        Arc::new(Unbounded),
        Reservation::Input,
    );
    (ctx, meter)
}
#[test]
fn every_legacy_format_loads_its_directory_and_payload() {
    for signature in [b"UnityRaw".as_slice(), b"UnityWeb".as_slice()] {
        for format in 1..=6 {
            let bytes = web::legacy(signature, format, &[("dir/raw.resS", b"payload")]);
            let (ctx, m) = setup();
            let b = BundleFile::open(RawStr::from("x"), bytes.into(), None, &m, &ctx).unwrap();
            b.load_all().unwrap();
            assert_eq!(b.entry_bytes(0).unwrap().as_ref(), b"payload");
            assert_eq!(b.directory()[0].path.as_bytes(), b"dir/raw.resS");
        }
    }
}
#[test]
fn web_packers_load_nested_files_and_register_cabs() {
    let asset = unityrs_testkit::env::serialized(
        &[],
        &[unityrs_testkit::env::Obj::named(7, "synthetic", &[])],
    );
    let inner = web::plain("TuanjieWebData1.0", &[("dir/CAB-A", &asset)]);
    let plain = web::plain(
        "UnityWebData1.0",
        &[("folder/inner", &inner), ("raw.resS", b"abc")],
    );
    for bytes in [plain.clone(), web::gzip(&plain), web::brotli(&plain)] {
        let env = Environment::new(EnvOptions::unitypy());
        let file = env
            .load_file(Source::Bytes {
                name: RawStr::from("dir/x.data"),
                data: bytes.into(),
            })
            .unwrap()
            .unwrap();
        assert!(matches!(file, File::Web(_)));
        assert_eq!(env.objects().count(), 1);
        assert!(env.registry().keys().any(|k| k.as_bytes() == b"cab-a"));
    }
}
#[test]
fn inflation_is_bounded_and_charged_as_work() {
    let plain = web::plain("UnityWebData1.0", &[("raw", &[42; 10000])]);
    for bytes in [web::gzip(&plain), web::brotli(&plain)] {
        let mut c = FileContext::unitypy();
        c.limits.max_inflate = 100;
        let ctx = Arc::new(c);
        let m = Meter::new(
            ctx.limits.max_input_memory,
            Arc::new(Unbounded),
            Reservation::Input,
        );
        let e = WebFile::open(RawStr::from("x"), Bytes::from(bytes), None, &m, &ctx).unwrap_err();
        assert!(
            matches!(e,Error::Limit(l) if l.kind()==LimitKind::Inflate),
            "{e:?}"
        );
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn signed_names_sizes_and_stream_reads_follow_the_reader() {
    use unityrs_core::stream::ReaderKind;
    let base = web::plain("UnityWebData1.0", &[("raw", b"tail")]);
    // Signature+NUL (16), header length (4), then offset, size, name length.
    let at = base.iter().position(|b| *b == 0).unwrap() + 5;
    for kind in [ReaderKind::Memory, ReaderKind::Stream] {
        for n in [-12i32, -2, -1, 0, 2000] {
            let mut data = base.clone();
            data[at + 8..at + 12].copy_from_slice(&n.to_le_bytes());
            // One row is sufficient, even when the chosen name consumes no bytes.
            data[at - 4..at].copy_from_slice(&i32::try_from(at + 12).unwrap().to_le_bytes());
            let (ctx, m) = setup();
            let got = unityrs_core::files::parse_file_with_reader(
                RawStr::from("x"),
                data.into(),
                unityrs_core::detect::FileType::WebFile,
                None,
                &m,
                &ctx,
                kind,
            );
            if n < -1 {
                assert!(matches!(
                    got,
                    Err(Error::Unsupported(
                        unityrs_core::Unsupported::WebFileNameLength
                    ))
                ));
            } else {
                let File::Web(w) = got.unwrap() else { panic!() };
                assert_eq!(w.directory().len(), 1);
                if n == -1 {
                    assert_eq!(w.directory()[0].path.as_bytes(), b"rawtail");
                }
            }
        }
    }
    for (offset, size, want) in [
        (-4i32, 4i32, b"".as_slice()),
        (-4, -1, b"tail".as_slice()),
        (1000, 10, b"".as_slice()),
        (0, -2, &base[..base.len() - 2]),
    ] {
        let mut data = base.clone();
        data[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        data[at + 4..at + 8].copy_from_slice(&size.to_le_bytes());
        let (ctx, m) = setup();
        let w = WebFile::open(RawStr::from("x"), data.into(), None, &m, &ctx).unwrap();
        assert_eq!(
            w.entry_bytes(0).unwrap().as_ref(),
            if offset == 0 && size == -2 {
                &w.data()[..w.data().len() - 2]
            } else {
                want
            }
        );
    }
}

#[test]
fn web_inside_a_lazy_bundle_is_reached_and_lists_objects() {
    use unityrs_core::env::LoadMode;
    let asset = unityrs_testkit::env::serialized(
        &[],
        &[unityrs_testkit::env::Obj::named(7, "nested", &[])],
    );
    let w = web::plain("UnityWebData1.0", &[("CAB-A", &asset)]);
    for bytes in [w.clone(), web::gzip(&w), web::brotli(&w)] {
        let bundle = unityrs_testkit::env::bundle(&[("inner", bytes)]);
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let mut opts = EnvOptions::unitypy();
            opts.load = mode;
            let env = Environment::new(opts);
            env.load_file(Source::Bytes {
                name: RawStr::from("outer"),
                data: bundle.clone().into(),
            })
            .unwrap();
            let objects = env.objects().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(objects.len(), 1);
            let _ = env
                .get_cab(&RawStr::from("inner"), objects[0].file().meter())
                .unwrap();
            assert!(
                env.get_cab(&RawStr::from("CAB-A"), objects[0].file().meter())
                    .unwrap()
                    .is_some()
            );
        }
    }
}

#[test]
fn compressed_work_and_allocator_refusals_release_every_charge() {
    let p = web::plain("UnityWebData1.0", &[("raw", &[42; 12000])]);
    for data in [
        web::gzip(&p),
        web::brotli(&p),
        web::legacy(b"UnityWeb", 5, &[("raw", &[42; 12000])]),
    ] {
        for memory in [0, 32, 1024, 8000, 20000, 50000, 100_000] {
            let mut ctx = FileContext::unitypy();
            ctx.limits.max_input_memory = memory;
            let ctx = Arc::new(ctx);
            let m = Meter::for_input(&ctx.limits, Arc::new(Unbounded), Reservation::Input);
            let ty = if data.starts_with(b"UnityWeb\0") {
                unityrs_core::detect::FileType::BundleFile
            } else {
                unityrs_core::detect::FileType::WebFile
            };
            let got = unityrs_core::files::parse_file(
                RawStr::from("x"),
                data.clone().into(),
                ty,
                None,
                &m,
                &ctx,
            );
            drop(got);
            assert_eq!(m.used(), 0);
        }
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_decode_work = 100;
        let ctx = Arc::new(ctx);
        let m = Meter::for_input(&ctx.limits, Arc::new(Unbounded), Reservation::Input);
        let ty = if data.starts_with(b"UnityWeb\0") {
            unityrs_core::detect::FileType::BundleFile
        } else {
            unityrs_core::detect::FileType::WebFile
        };
        let e = unityrs_core::files::parse_file(RawStr::from("x"), data.into(), ty, None, &m, &ctx)
            .unwrap_err();
        assert!(
            matches!(e,Error::Limit(l) if l.kind()==LimitKind::DecodeWork),
            "{e:?}"
        );
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn ten_thousand_mutated_headers_and_streams_never_panic() {
    let p = web::plain("UnityWebData1.0", &[("raw", b"payload")]);
    let mut seeds = vec![p.clone(), web::gzip(&p), web::brotli(&p), web::huffman()];
    for f in 1..=6 {
        for sig in [b"UnityRaw".as_slice(), b"UnityWeb".as_slice()] {
            seeds.push(web::legacy(sig, f, &[("raw", b"payload")]));
        }
    }
    let mut rng = unityrs_testkit::Rng::new(0xc2);
    for i in 0..10000 {
        let mut data = seeds[i % seeds.len()].clone();
        if i % 3 == 0 {
            data.truncate(usize::try_from(rng.next_u64()).unwrap_or(0) % data.len());
        } else {
            let at = usize::try_from(rng.next_u64()).unwrap_or(0) % data.len();
            data[at] ^= rng.next_u64().to_le_bytes()[0];
        }
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_input_memory = 2 << 20;
        ctx.limits.max_inflate = 65536;
        ctx.limits.max_decode_work = 1 << 20;
        let ctx = Arc::new(ctx);
        let m = Meter::for_input(&ctx.limits, Arc::new(Unbounded), Reservation::Input);
        let ty = if i % seeds.len() < 4 {
            unityrs_core::detect::FileType::WebFile
        } else {
            unityrs_core::detect::FileType::BundleFile
        };
        let result =
            unityrs_core::files::parse_file(RawStr::from("x"), data.into(), ty, None, &m, &ctx);
        if let Ok(file) = &result {
            for slot in file.container_files() {
                let _ = file.container_entry(slot.dir_index);
            }
        }
        drop(result);
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn huffman_brotli_is_fallible_at_every_small_memory_bound() {
    let bytes = web::huffman();
    let (ctx, m) = setup();
    let w = WebFile::open(RawStr::from("x"), bytes.clone().into(), None, &m, &ctx).unwrap();
    assert_eq!(w.entry_bytes(0).unwrap().as_ref(), b"ab12".repeat(3000));
    drop(w);
    assert_eq!(m.used(), 0);
    for memory in (0..512 * 1024).step_by(521) {
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_input_memory = memory;
        let ctx = Arc::new(ctx);
        let m = Meter::for_input(&ctx.limits, Arc::new(Unbounded), Reservation::Input);
        drop(WebFile::open(
            RawStr::from("x"),
            bytes.clone().into(),
            None,
            &m,
            &ctx,
        ));
        assert_eq!(m.used(), 0);
    }
}

#[test]
fn a_shadowed_lazy_brotli_container_registers_its_members() {
    use unityrs_core::env::LoadMode;
    let asset = unityrs_testkit::env::serialized(
        &[],
        &[unityrs_testkit::env::Obj::named(7, "nested", &[])],
    );
    let web = web::brotli(&web::plain("UnityWebData1.0", &[("CAB-A", &asset)]));
    let bundle = unityrs_testkit::env::bundle(&[("inner", web)]);
    let mut opts = EnvOptions::unitypy();
    opts.load = LoadMode::Lazy;
    let env = Environment::new(opts);
    let file = env
        .load_file(Source::Bytes {
            name: RawStr::from("outer"),
            data: bundle.into(),
        })
        .unwrap()
        .unwrap();
    env.load_file(Source::Bytes {
        name: RawStr::from("inner"),
        data: Bytes::from_static(b"replacement"),
    })
    .unwrap();
    assert!(
        env.get_cab(&RawStr::from("CAB-A"), file.meter())
            .unwrap()
            .is_some()
    );
}

#[test]
fn resource_reads_reach_mixed_lazy_container_ancestors() {
    use unityrs_core::env::LoadMode;
    let asset = unityrs_testkit::env::serialized(
        &[],
        &[unityrs_testkit::env::Obj::named(7, "nested", &[])],
    );
    let inside = web::plain(
        "UnityWebData1.0",
        &[("CAB-A", &asset), ("raw.resS", b"payload")],
    );
    let bundle =
        unityrs_testkit::env::bundle(&[("CAB-A", asset), ("raw.resS", b"payload".to_vec())]);
    for bytes in [
        unityrs_testkit::env::bundle(&[("inner", inside)]),
        web::plain("UnityWebData1.0", &[("inner", &bundle)]),
    ] {
        let mut opts = EnvOptions::unitypy();
        opts.load = LoadMode::Lazy;
        let env = Environment::new(opts);
        let _top = env
            .load_file(Source::Bytes {
                name: RawStr::from("outer"),
                data: bytes.into(),
            })
            .unwrap();
        let obj = env.objects().next().unwrap().unwrap();
        assert_eq!(
            env.resource(obj.file(), &RawStr::from("raw.resS"), 0, 7)
                .unwrap()
                .as_ref(),
            b"payload"
        );
    }
}

#[test]
fn only_the_final_legacy_level_sizes_control_the_payload() {
    for sig in [b"UnityRaw".as_slice(), b"UnityWeb".as_slice()] {
        for levels in [2, 3] {
            let data = web::legacy_levels(sig, 5, &[("raw", b"payload")], levels);
            let (ctx, m) = setup();
            let b = BundleFile::open(RawStr::from("x"), data.into(), None, &m, &ctx).unwrap();
            assert_eq!(b.entry_bytes(0).unwrap().as_ref(), b"payload");
        }
    }
}

#[test]
fn legacy_web_inflation_ignores_declared_sizes_but_obeys_the_inflate_cap() {
    let mut data = web::legacy(b"UnityWeb", 5, &[("raw", &[42; 12000])]);
    let header = u32::from_be_bytes(data[54..58].try_into().unwrap()) as usize;
    data[70..74].copy_from_slice(&u32::MAX.to_be_bytes());
    data[header + 5..header + 13].copy_from_slice(&u64::MAX.to_le_bytes());
    let (ctx, m) = setup();
    let b = BundleFile::open(RawStr::from("x"), data.clone().into(), None, &m, &ctx).unwrap();
    assert_eq!(b.entry_bytes(0).unwrap().len(), 12000);
    drop(b);
    assert_eq!(m.used(), 0);
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_inflate = 100;
    let ctx = Arc::new(ctx);
    let m = Meter::for_input(&ctx.limits, Arc::new(Unbounded), Reservation::Input);
    let error = BundleFile::open(RawStr::from("x"), data.into(), None, &m, &ctx).unwrap_err();
    assert!(matches!(error,Error::Limit(l) if l.kind()==LimitKind::Inflate));
    assert_eq!(m.used(), 0);
}
