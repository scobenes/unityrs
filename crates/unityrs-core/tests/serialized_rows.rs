//! `PARITY.md` rows on a `SerializedFile`'s object table, its version chain,
//! a bundle's signature and the reader an object's size picks, pinned
//! against UnityPy 1.25.2's rules (`files/ObjectReader.py:67-130`,
//! `files/SerializedFile.py:291-295` and `:351-360`, `files/BundleFile.py:41-51`,
//! `helpers/TypeTreeHelper.py:146-153`). UnityPy's side of each is pinned by
//! the self-test the row names and by G3's dump of the testkit matrix.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::env::{Cab, EnvOptions, Environment, RawCab, Source};
use unityrs_core::files::{BundleFile, File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::version::{VersionError, VersionSource};
use unityrs_core::walk::ParseError;
use unityrs_core::{Bytes, Error, FileContext, ObjectRef, RawStr, ReadBy, Unsupported, Value};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, build as bundle, build_with_signature};
use unityrs_testkit::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec, build};

fn meter(c: &FileContext) -> Arc<Meter> {
    Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input)
}

fn parse_in(data: &[u8], c: &Arc<FileContext>) -> Result<Arc<SerializedFile>, Error> {
    SerializedFile::parse(
        RawStr::from("CAB-t"),
        Bytes::copy_from_slice(data),
        None,
        &meter(c),
        c,
    )
}

fn parse(data: &[u8]) -> Result<Arc<SerializedFile>, Error> {
    parse_in(data, &Arc::new(FileContext::unitypy()))
}

fn tree(root: &str) -> Vec<NodeSpec> {
    vec![
        NodeSpec::new(0, root, "Base"),
        NodeSpec::new(1, "int", "m_A").size(4),
    ]
}

/// One object row as the test writes it, and what UnityPy reads from it.
struct Row {
    path_id: i64,
    /// The index of its type in `types`.
    ty: usize,
    /// The `u16` stored below format 16.
    stored_class: u16,
    data: Vec<u8>,
    script: i16,
    stripped: i8,
}

const CLASSES: [i32; 2] = [1, 4];

fn rows() -> Vec<Row> {
    let row = |path_id, ty, stored_class, data: &[u8], script, stripped| Row {
        path_id,
        ty,
        stored_class,
        data: data.to_vec(),
        script,
        stripped,
    };
    vec![
        row(10, 0, 40_000, &[1, 0, 0, 0], 3, -2),
        row(20, 1, 4, &[2, 0, 0, 0, 9, 9, 9, 9], 5, 1),
        // Path id 10 again: it keeps the first position and the last row.
        row(10, 1, 7, &[3, 0, 0, 0], 7, 0),
        row(-30, 0, 1, &[4, 0, 0, 0, 5], -1, 127),
    ]
}

fn file(format: u32, big_endian: bool) -> Vec<u8> {
    let types = CLASSES
        .iter()
        .map(|&class_id| TypeSpec {
            class_id,
            nodes: tree("GameObject"),
            ..TypeSpec::default()
        })
        .collect();
    let objects = rows()
        .into_iter()
        .map(|r| ObjectSpec {
            path_id: r.path_id,
            // From 16 an index into `types`; below it a class id, which
            // the type scan matches (`files/ObjectReader.py:89-101`).
            type_id: if format >= 16 {
                i32::try_from(r.ty).unwrap()
            } else {
                CLASSES[r.ty]
            },
            class_id: r.stored_class,
            data: r.data,
            script_type_index: Some(r.script),
            is_stripped: Some(r.stripped),
            ..ObjectSpec::default()
        })
        .collect();
    build(&SerializedSpec {
        format,
        big_endian,
        types,
        objects,
        ..SerializedSpec::default()
    })
}

/// Every object row at formats 9 to 22, both endians, as UnityPy reads it
/// (`files/ObjectReader.py:67-130`): the path id (from 14 after a 4-byte
/// alignment), `byte_start` from the stored offset plus the header's
/// `data_offset` (an `i64` from 22) and `byte_size` (the object's bytes are
/// what they name), the class id (below 16 the stored `u16`, from 16 the
/// type's), the script index at 11 to 16, written back into the object's
/// type so the last row of a type wins, and `is_stripped`, a signed byte, at
/// 15 and 16; and the table collapsed by path id, a repeated id keeping its
/// first position and its last row (`files/SerializedFile.py:291-295`).
#[test]
fn object_rows_read_by_format_as_unitypy_reads_them() {
    let want = rows();
    for format in 9..=22 {
        for big_endian in [false, true] {
            let at = format!("format {format}, big endian {big_endian}");
            let f = parse(&file(format, big_endian)).unwrap_or_else(|e| panic!("{at}: {e}"));
            let infos = f.object_infos();
            assert_eq!(infos.len(), want.len(), "{at}");
            for (i, (o, w)) in infos.iter().zip(&want).enumerate() {
                let at = format!("{at}, row {i}");
                assert_eq!(o.path_id, w.path_id, "{at}");
                assert_eq!(o.byte_size as usize, w.data.len(), "{at}");
                let index = u32::try_from(i).unwrap();
                let raw = ObjectRef::new(f.clone(), index).unwrap().raw_data();
                assert_eq!(raw.unwrap().as_ref(), w.data.as_slice(), "{at}");
                assert_eq!(o.type_index, Some(w.ty), "{at}");
                let class = if format >= 16 {
                    CLASSES[w.ty]
                } else {
                    i32::from(w.stored_class)
                };
                assert_eq!(o.class_id, class, "{at}");
                let script = (11..=16).contains(&format).then_some(w.script);
                assert_eq!(o.script_type_index, script, "{at}");
                let stripped = (15..=16).contains(&format).then_some(w.stripped);
                assert_eq!(o.is_stripped, stripped, "{at}");
            }
            if (11..=16).contains(&format) {
                let scripts: Vec<i16> = f.types().iter().map(|t| t.script_type_index).collect();
                assert_eq!(scripts, [-1, 7], "{at}: the last row of each type");
            }
            let order: Vec<i64> = f.objects().map(|o| o.path_id()).collect();
            assert_eq!(order, [10, 20, -30], "{at}");
            assert_eq!(f.object(10).unwrap().index(), 2, "{at}: the last row");
            let first = f.objects().next().unwrap();
            assert_eq!(first.index(), 2, "{at}: at the first position");
        }
    }
}

fn versioned(format: u32, version: &[u8]) -> Vec<u8> {
    build(&SerializedSpec {
        format,
        unity_version: version.to_vec(),
        types: vec![TypeSpec {
            class_id: 1,
            nodes: tree("GameObject"),
            ..TypeSpec::default()
        }],
        objects: vec![ObjectSpec {
            path_id: 1,
            data: vec![0; 4],
            ..ObjectSpec::default()
        }],
        ..SerializedSpec::default()
    })
}

fn with_fallback(fallback: Option<&str>) -> Arc<FileContext> {
    let mut c = FileContext::unitypy();
    c.version.fallback = fallback.map(RawStr::from);
    Arc::new(c)
}

/// `SerializedFile.set_version` (`files/SerializedFile.py:351-360`): a
/// string its pattern does not match raises whatever the fallback, at
/// formats 9, 15 and 22 (`2017.4` and `abc`), and the file does not load;
/// an empty string or `0.0.0` takes the parent bundle's engine string, and
/// where that is unusable too, or there is no bundle,
/// `config.FALLBACK_UNITY_VERSION`, whose absence raises.
#[test]
fn set_version_falls_back_only_from_an_empty_string_or_0_0_0() {
    for format in [9, 15, 22] {
        for fallback in [None, Some("2019.4.15f1")] {
            let c = with_fallback(fallback);
            for bad in [&b"2017.4"[..], b"abc"] {
                let got = parse_in(&versioned(format, bad), &c);
                assert!(
                    matches!(&got, Err(Error::Version(VersionError::NoMatch(s))) if s.as_bytes() == bad),
                    "{format} {fallback:?}: {got:?}"
                );
            }
            for unusable in [&b""[..], b"0.0.0"] {
                let got = parse_in(&versioned(format, unusable), &c);
                match fallback {
                    None => assert!(
                        matches!(got, Err(Error::Version(VersionError::NoFallback))),
                        "{format}: {got:?}"
                    ),
                    Some(v) => {
                        let f = got.unwrap();
                        let r = f.version().unwrap();
                        assert_eq!(r.source, VersionSource::Fallback, "{format}");
                        assert_eq!(r.version.raw().as_bytes(), v.as_bytes(), "{format}");
                    }
                }
            }
        }
    }
    // In a bundle an unusable string takes the bundle's engine string, an
    // unusable engine string then the fallback, and an unmatched string
    // still raises.
    for (engine, fallback) in [
        ("2019.4.15f1", None),
        ("2019.4.15f1", Some("2018.4.36f1")),
        ("0.0.0", Some("2018.4.36f1")),
        ("0.0.0", None),
    ] {
        let c = with_fallback(fallback);
        for version in [&b"0.0.0"[..], b"", b"2017.4"] {
            let at = format!("engine {engine}, fallback {fallback:?}, {version:?}");
            let data = bundle(&BundleSpec {
                version_engine: engine.as_bytes().to_vec(),
                entries: vec![EntrySpec {
                    name: b"CAB-v".to_vec(),
                    data: versioned(22, version),
                    flags: 4,
                }],
                ..BundleSpec::default()
            });
            let opened = BundleFile::open(
                RawStr::from("v.bundle"),
                Bytes::from(data),
                None,
                &meter(&c),
                &c,
            );
            // An unusable engine string with no fallback raises as the
            // bundle opens (`files/BundleFile.py:523-536`), before any entry.
            if (engine, fallback) == ("0.0.0", None) {
                assert!(
                    matches!(opened, Err(Error::Version(VersionError::NoFallback))),
                    "{at}: {opened:?}"
                );
                continue;
            }
            let entry = opened.and_then(|b| b.entry(0));
            let source = |e: &Error| match e {
                Error::Entry { source, .. } => (**source).clone(),
                e => e.clone(),
            };
            match (entry, version, engine, fallback) {
                (Err(e), b"2017.4", ..) => assert!(
                    matches!(source(&e), Error::Version(VersionError::NoMatch(_))),
                    "{at}: {e:?}"
                ),
                (Ok(File::Serialized(f)), _, "2019.4.15f1", _) => {
                    let r = f.version().unwrap();
                    assert_eq!(r.source, VersionSource::ParentBundle, "{at}");
                    assert_eq!(r.version.raw().as_bytes(), engine.as_bytes(), "{at}");
                }
                (Ok(File::Serialized(f)), _, "0.0.0", Some(v)) => {
                    let r = f.version().unwrap();
                    assert_eq!(r.source, VersionSource::Fallback, "{at}");
                    assert_eq!(r.version.raw().as_bytes(), v.as_bytes(), "{at}");
                }
                (got, ..) => panic!("{at}: {got:?}"),
            }
        }
    }
}

/// `BundleFile.__init__` (`files/BundleFile.py:41-51`): a signature other
/// than `UnityFS`, `UnityWeb`, `UnityRaw` and `UnityArchive` raises "Unknown
/// Bundle signature", the U+00FA signature detection takes for a bundle
/// among them (its eight characters are UTF-8 bytes that no signature is).
#[test]
fn an_unknown_bundle_signature_is_malformed() {
    let c = Arc::new(FileContext::unitypy());
    let spec = BundleSpec {
        entries: vec![EntrySpec {
            name: b"a".to_vec(),
            data: vec![1; 40],
            flags: 0,
        }],
        ..BundleSpec::default()
    };
    for signature in [
        "UnityXYZ",
        "\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}",
        "",
    ] {
        let data = build_with_signature(&spec, signature.as_bytes());
        let got = BundleFile::open(
            RawStr::from("x.bundle"),
            Bytes::from(data),
            None,
            &meter(&c),
            &c,
        );
        assert!(
            matches!(&got, Err(Error::Malformed { what: "bundle signature", detail })
                if &**detail == "Unknown Bundle signature"),
            "{signature:?}: {got:?}"
        );
    }
}

/// `TypeTreeHelper.read_typetree` (`helpers/TypeTreeHelper.py:146-153`): an
/// object whose `byte_size` is 0 is read by the pure-Python reader from its
/// `byte_start` in the whole file, which reads past the size, and
/// `check_read` then raises "Expected to read 0 bytes"; with `check_read`
/// off the value is returned. With its own size UnityPyBoost reads it.
#[test]
fn a_zero_size_object_is_read_by_the_pure_python_reader() {
    let data = |byte_size| {
        build(&SerializedSpec {
            types: vec![TypeSpec {
                class_id: 1,
                nodes: tree("GameObject"),
                ..TypeSpec::default()
            }],
            objects: vec![ObjectSpec {
                path_id: 1,
                data: 5_i32.to_le_bytes().to_vec(),
                byte_size,
                ..ObjectSpec::default()
            }],
            ..SerializedSpec::default()
        })
    };
    let m_a = |p: &unityrs_core::Parsed| p.get(&[b"m_A"]).and_then(Value::as_i64);
    let f = parse(&data(Some(0))).unwrap();
    let o = f.object(1).unwrap();
    let Err(ParseError::CheckRead { expected, parsed }) = o.parse_as_dict(true) else {
        panic!("no check_read error")
    };
    assert_eq!(expected, 0);
    assert_eq!((parsed.bytes_read, parsed.read_by()), (4, ReadBy::Python));
    assert_eq!(m_a(&parsed), Some(5));
    let unchecked = o.parse_as_dict(false).unwrap();
    assert_eq!(unchecked.read_by(), ReadBy::Python);
    assert_eq!(m_a(&unchecked), Some(5));
    let f = parse(&data(None)).unwrap();
    let sized = f.object(1).unwrap().parse_as_dict(true).unwrap();
    assert_eq!(sized.read_by(), ReadBy::Boost);
    assert_eq!(m_a(&sized), Some(5));
}

/// From format 21 a type's dependencies, and a ref type's class, namespace
/// and assembly, sit inside the tree flag (`files/SerializedFile.py:137-149`):
/// with the flag false the file holds neither, the type's dependencies and
/// the ref type's names stay unset, and every table after the types reads
/// where it lies.
#[test]
fn type_dependencies_are_read_only_under_the_tree_flag() {
    for format in [21, 22] {
        for type_trees in [false, true] {
            let data = build(&SerializedSpec {
                format,
                type_trees,
                types: vec![TypeSpec {
                    class_id: 1,
                    nodes: tree("GameObject"),
                    dependencies: vec![1, 2],
                    ..TypeSpec::default()
                }],
                objects: vec![ObjectSpec {
                    path_id: 7,
                    data: vec![0; 4],
                    ..ObjectSpec::default()
                }],
                externals: vec![ExternalSpec {
                    path: b"library/unity default resources".to_vec(),
                    ..ExternalSpec::default()
                }],
                ref_types: vec![TypeSpec {
                    class_id: 114,
                    nodes: tree("Ref"),
                    ref_names: Some((b"Foo".to_vec(), b"Ns".to_vec(), b"Asm.dll".to_vec())),
                    ..TypeSpec::default()
                }],
                user_information: b"ui".to_vec(),
                ..SerializedSpec::default()
            });
            let at = format!("format {format}, tree flag {type_trees}");
            let refs = f_ref_names(&data);
            let want =
                type_trees.then(|| ["Foo", "Ns", "Asm.dll"].map(|n| Some(n.as_bytes().to_vec())));
            assert_eq!(refs, want.unwrap_or([None, None, None]), "{at}");
            let f = parse(&data).unwrap_or_else(|e| panic!("{at}: {e}"));
            let t = &f.types()[0];
            let deps = t.type_dependencies.as_ref().map(|d| d.to_vec());
            assert_eq!(deps, type_trees.then(|| vec![1, 2]), "{at}");
            assert_eq!(t.nodes.is_some(), type_trees, "{at}");
            assert_eq!(f.object_infos()[0].path_id, 7, "{at}");
            let path = f.externals()[0].path.as_bytes();
            assert_eq!(path, b"library/unity default resources", "{at}");
            assert_eq!(f.user_information().unwrap().as_bytes(), b"ui", "{at}");
        }
    }
}

/// The class, namespace and assembly of `data`'s first ref type.
fn f_ref_names(data: &[u8]) -> [Option<Vec<u8>>; 3] {
    let f = parse(data).unwrap();
    let t = &f.ref_types().unwrap()[0];
    [&t.class_name, &t.namespace, &t.assembly].map(|n| n.as_ref().map(|s| s.as_bytes().to_vec()))
}

/// Format 22's large header (`files/SerializedFile.py:260-264`), read
/// big-endian before the file's own endian applies: a `u32` metadata size,
/// then the file size, the data offset and `unknown`, each a signed 64-bit
/// read whose upper half counts, `unknown` negative here.
#[test]
fn the_large_header_reads_three_signed_64_bit_fields() {
    let file_size: i64 = 0x0001_2345_6789_abcd;
    let data_offset: i64 = 0x0000_0002_0000_0010;
    let unknown: i64 = -0x0102_0304_0506_0708;
    for big_endian in [false, true] {
        let mut data = build(&SerializedSpec {
            big_endian,
            file_size: Some(file_size),
            data_offset: Some(data_offset),
            ..SerializedSpec::default()
        });
        // The header: four `u32`s (16 bytes), the endian byte and three
        // reserved (20), the metadata size (24), the file size (32), the
        // data offset (40), then `unknown`.
        data[40..48].copy_from_slice(&unknown.to_be_bytes());
        let f = parse(&data).unwrap_or_else(|e| panic!("{big_endian}: {e}"));
        let h = f.header();
        assert_eq!(h.version, 22);
        assert_eq!(h.file_size, file_size, "{big_endian}");
        assert_eq!(h.data_offset, data_offset, "{big_endian}");
        assert_eq!(h.unknown, unknown, "{big_endian}");
    }
}

/// A type's `is_stripped_type` byte (`files/SerializedFile.py:122-123`):
/// read from format 16, each type's own value; below 16 it is not read.
#[test]
fn is_stripped_type_is_read_from_format_16() {
    for format in [15, 16, 17, 22] {
        let types = [true, false]
            .into_iter()
            .map(|stripped| TypeSpec {
                class_id: 1,
                stripped,
                nodes: tree("GameObject"),
                ..TypeSpec::default()
            })
            .collect();
        let f = parse(&build(&SerializedSpec {
            format,
            types,
            ..SerializedSpec::default()
        }))
        .unwrap_or_else(|e| panic!("{format}: {e}"));
        let got: Vec<Option<bool>> = f.types().iter().map(|t| t.is_stripped_type).collect();
        let want = if format >= 16 {
            vec![Some(true), Some(false)]
        } else {
            vec![None, None]
        };
        assert_eq!(got, want, "{format}");
    }
}

/// A codec id UnityPy has no decompressor for raises (`files/BundleFile.py:515-521`),
/// in a data block when it is read and in the blocks-info when the bundle
/// opens: `Unsupported::Codec` naming the id (the matrix's
/// `unknown-codec-block.bundle` holds 9 in block 1, `unknown-codec-info.bundle`
/// 7 in the blocks-info).
#[test]
fn an_unknown_codec_is_unsupported_naming_its_id() {
    let by_name = |n: &str| {
        unityrs_testkit::bundle::matrix()
            .into_iter()
            .find(|(m, _)| m == n)
            .unwrap()
            .1
    };
    let c = Arc::new(FileContext::unitypy());
    let open = |data: Vec<u8>| {
        BundleFile::open(
            RawStr::from("u.bundle"),
            Bytes::from(data),
            None,
            &meter(&c),
            &c,
        )
    };
    let codec = |e: &Error| match e {
        Error::Unsupported(Unsupported::Codec(id)) => Some(*id),
        Error::Entry { source, .. } => match &**source {
            Error::Unsupported(Unsupported::Codec(id)) => Some(*id),
            _ => None,
        },
        _ => None,
    };
    let b = open(by_name("unknown-codec-block.bundle")).unwrap();
    let e = b.load_all().unwrap_err();
    assert_eq!(codec(&e), Some(9), "{e:?}");
    let e = open(by_name("unknown-codec-info.bundle")).unwrap_err();
    assert_eq!(codec(&e), Some(7), "{e:?}");
}

/// A `UnityArchive` file (`helpers/ImportHelper.py:60-66`) is no bundle to
/// detection: through loading it is a `ResourceFile` kept as a raw reader,
/// registered as a CAB under its simplified name, with no error.
#[test]
fn a_unity_archive_loads_as_a_raw_file_and_a_cab() {
    let data = build_with_signature(
        &BundleSpec {
            entries: vec![EntrySpec {
                name: b"a".to_vec(),
                data: vec![1; 200],
                flags: 0,
            }],
            ..BundleSpec::default()
        },
        b"UnityArchive",
    );
    let env = Environment::new(EnvOptions::unitypy());
    let loaded = env
        .load(Source::Bytes {
            name: RawStr::from("Data/Archive.BIN"),
            data: Bytes::from(data.clone()),
        })
        .unwrap();
    let File::Raw(raw) = loaded else {
        panic!("not raw: {loaded:?}")
    };
    assert_eq!(raw.data.as_ref(), data.as_slice());
    let keys: Vec<Vec<u8>> = env
        .registry()
        .keys()
        .map(|k| k.as_bytes().to_vec())
        .collect();
    assert_eq!(keys, [b"archive.bin".to_vec()]);
    let cab = env.get_cab(&RawStr::from("archive.bin"), &Meter::unlimited());
    let Ok(Some(Cab::Raw(RawCab::File(cab)))) = cab else {
        panic!("{cab:?}")
    };
    assert_eq!(cab.data.as_ref(), data.as_slice());
}
