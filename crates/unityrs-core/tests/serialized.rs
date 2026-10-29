//! `SerializedFile` and the typetree row parsers against UnityPy's rules
//! (plan sections 3.7, 3.8 and 7.4): counts, type ids, raw metadata strings,
//! blob rows, and seeded no-panic runs.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::detect::check_file_type;
use unityrs_core::files::{File, SerializedFile, parse_file};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::provider::{ProviderChain, ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::stream::{Endian, Reader, py_read};
use unityrs_core::typetree::commonstring::CommonStrings;
use unityrs_core::typetree::parse::{parse_blob, parse_old};
use unityrs_core::typetree::{NodeList, Tree};
use unityrs_core::{Bytes, Error, FileContext, LimitKind, RawStr};
use unityrs_testkit::Rng;
use unityrs_testkit::blob::{NodeSpec, Strings, blob_tree, old_tree};
use unityrs_testkit::bundle::matrix;
use unityrs_testkit::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec, build};

fn ctx() -> Arc<FileContext> {
    Arc::new(FileContext::unitypy())
}

fn parse(data: &[u8]) -> Result<Arc<SerializedFile>, Error> {
    let c = ctx();
    let m = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
    SerializedFile::parse(
        RawStr::from("CAB-t"),
        Bytes::copy_from_slice(data),
        None,
        &m,
        &c,
    )
}

fn tree() -> Vec<NodeSpec> {
    vec![
        NodeSpec::new(0, "GameObject", "Base"),
        NodeSpec::new(1, "int", "m_A").size(4),
    ]
}

/// An empty format-22 little-endian file: the counts sit at 64 (types),
/// 68 (objects), 72 (scripts), 76 (externals) and 80 (ref types).
fn empty() -> Vec<u8> {
    build(&SerializedSpec::default())
}

#[test]
fn every_table_counts_as_range_does() {
    let base = empty();
    let ok = parse(&base).unwrap();
    assert!(ok.types().is_empty() && ok.object_infos().is_empty());
    for at in [64usize, 68, 72, 76, 80] {
        // A negative count is zero records.
        let mut neg = base.clone();
        neg[at..at + 4].copy_from_slice(&(-5i32).to_le_bytes());
        let f = parse(&neg).unwrap_or_else(|e| panic!("count at {at}: {e}"));
        assert_eq!(f.object_order().len(), 0);
        // A huge count fails at the end of the bytes, allocating nothing
        // from the count.
        let mut huge = base.clone();
        huge[at..at + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        let c = ctx();
        let m = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
        let got = SerializedFile::parse(
            RawStr::from("CAB-t"),
            Bytes::copy_from_slice(&huge),
            None,
            &m,
            &c,
        );
        assert!(got.is_err(), "count at {at}: {got:?}");
        assert_eq!(m.used(), 0, "everything returned");
        assert!(m.granted() < 4096, "count at {at}: {} granted", m.granted());
    }
}

#[test]
fn a_table_of_one_byte_externals_stops_at_the_meter() {
    // Format 5 externals are a 16-byte guid, a type and a path; format 6 adds
    // the empty `temp_empty`, so each costs a few bytes of input and a row.
    let spec = SerializedSpec {
        externals: (0..20_000)
            .map(|_| ExternalSpec {
                path: b"x".to_vec(),
                ..ExternalSpec::default()
            })
            .collect(),
        ..SerializedSpec::default()
    };
    let data = build(&spec);
    let mut c = FileContext::unitypy();
    c.limits.max_input_memory = 256 << 10;
    let c = Arc::new(c);
    let m = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
    let got = SerializedFile::parse(
        RawStr::from("CAB-t"),
        Bytes::from(data.clone()),
        None,
        &m,
        &c,
    );
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{got:?}"
    );
    assert_eq!(parse(&data).unwrap().externals().len(), 20_000);
}

/// UnityPy files/ObjectReader.py:82-86: a format-22 row's `byte_start` is a
/// signed 64-bit read plus `data_offset`, a Python int, so it may pass
/// `i64::MAX` (the oracle parses this matrix input; its L1 has no rows).
#[test]
fn a_byte_start_past_i64_is_kept_whole() {
    let (_, data) = matrix()
        .into_iter()
        .find(|(n, _)| n == "CAB-f22-byte-start-past-i64")
        .unwrap();
    let f = parse(&data).unwrap();
    let offset = i128::from(f.header().data_offset);
    let first = f.object_infos().first().unwrap();
    assert_eq!(first.path_id, 1);
    assert_eq!(first.byte_start, i128::from(i64::MAX) + offset);
    assert!(first.byte_start > i128::from(i64::MAX));
}

#[test]
fn type_ids_index_types_as_python_lists() {
    let types = vec![
        TypeSpec {
            class_id: 1,
            nodes: tree(),
            ..TypeSpec::default()
        },
        TypeSpec {
            class_id: 4,
            nodes: tree(),
            ..TypeSpec::default()
        },
    ];
    let with = |type_id: i32| {
        build(&SerializedSpec {
            types: types.clone(),
            objects: vec![ObjectSpec {
                path_id: 1,
                type_id,
                data: vec![0; 4],
                ..ObjectSpec::default()
            }],
            ..SerializedSpec::default()
        })
    };
    for (type_id, want) in [
        (0, Some(1)),
        (1, Some(4)),
        (-1, Some(4)),
        (-2, Some(1)),
        (2, None),
        (-3, None),
        (i32::MIN, None),
    ] {
        match (parse(&with(type_id)), want) {
            (Ok(f), Some(class)) => assert_eq!(f.object_infos()[0].class_id, class, "{type_id}"),
            (Err(Error::TypeIndex { index, len: 2 }), None) => assert_eq!(index, type_id),
            (got, _) => panic!("{type_id}: {got:?}"),
        }
    }
    // Below 16 the type is the first whose class equals the stored type id,
    // and none is no error.
    let old = build(&SerializedSpec {
        format: 15,
        types: types.clone(),
        objects: vec![ObjectSpec {
            path_id: 1,
            type_id: 9,
            class_id: 9,
            data: vec![0; 4],
            ..ObjectSpec::default()
        }],
        ..SerializedSpec::default()
    });
    let f = parse(&old).unwrap();
    assert_eq!(f.object_infos()[0].type_index, None);
    assert_eq!(f.object_infos()[0].class_id, 9);
}

#[test]
fn metadata_strings_are_kept_raw() {
    let spec = SerializedSpec {
        user_information: vec![0xff, 0xfe, b'a'],
        externals: vec![ExternalSpec {
            temp_empty: vec![0xc3],
            path: vec![b'p', 0x80],
            ..ExternalSpec::default()
        }],
        types: vec![TypeSpec {
            class_id: 1,
            nodes: vec![NodeSpec {
                ty: vec![0xe9],
                ..NodeSpec::new(0, "x", "Base")
            }],
            ..TypeSpec::default()
        }],
        ..SerializedSpec::default()
    };
    let f = parse(&build(&spec)).unwrap();
    assert_eq!(f.user_information().unwrap().as_bytes(), [0xff, 0xfe, b'a']);
    assert_eq!(f.externals()[0].path.as_bytes(), [b'p', 0x80]);
    assert_eq!(
        f.externals()[0].temp_empty.as_ref().unwrap().as_bytes(),
        [0xc3]
    );
    let nodes = f.types()[0].nodes.as_ref().unwrap();
    assert_eq!(nodes.rows()[0].ty.as_bytes(), [0xe9]);
    // A version string that does not parse fails the file (UnityPy packs it
    // directly), and an empty one wants the fallback.
    for v in [&b"\xff"[..], b"", b"0.0.0"] {
        let bad = build(&SerializedSpec {
            unity_version: v.to_vec(),
            ..SerializedSpec::default()
        });
        assert!(matches!(parse(&bad), Err(Error::Version(_))), "{v:?}");
    }
}

fn blob_rows(
    nodes: &[NodeSpec],
    format: u32,
    big: bool,
) -> Result<Vec<unityrs_core::typetree::NodeRow>, Error> {
    let data = Bytes::from(blob_tree(nodes, format, big, Strings::Local, &[]));
    let mut r = Reader::new(&data, if big { Endian::Big } else { Endian::Little });
    let m = Meter::unlimited();
    Ok(
        parse_blob(&mut r, &data, format, &m, &Limits::default(), None)?
            .as_slice()
            .to_vec(),
    )
}

#[test]
fn blob_rows_are_unpacked_as_unitypy_does() {
    let mut nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(2, "int", "jump").size(4),
        NodeSpec::new(1, "int", "back").size(4),
    ];
    nodes[1].version = 70_000; // written as i16: 70000 - 65536
    nodes[2].ref_type_hash = u64::MAX;
    nodes[2].index = Some(99);
    for big in [false, true] {
        let rows = blob_rows(&nodes, 19, big).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].level, 2);
        assert_eq!(rows[1].version, 70_000 - 65_536);
        assert_eq!(rows[2].ref_type_hash, Some(u64::MAX));
        assert_eq!(rows[2].index, Some(99), "kept as stored");
        assert_eq!(rows[0].variable_count, None);
        let rows = blob_rows(&nodes, 17, big).unwrap();
        assert_eq!(rows[2].ref_type_hash, None);
        // The level jump nests under the previous row; the list keeps it.
        let list = NodeList::trusted(rows).unwrap();
        assert_eq!(list.len(), 3);
    }
    // A common offset the table lacks is its decimal string.
    let common: &[(u32, &[u8])] = &[(0x7FFF_FFF0, b"Base")];
    let data = Bytes::from(blob_tree(&nodes, 17, false, Strings::Common, common));
    let mut r = Reader::new(&data, Endian::Little);
    let rows = parse_blob(
        &mut r,
        &data,
        17,
        &Meter::unlimited(),
        &Limits::default(),
        None,
    )
    .unwrap();
    if unityrs_core::typetree::commonstring::common_string(0x7FFF_FFF0).is_none() {
        assert_eq!(rows.as_slice()[0].ty.as_bytes(), b"2147483632");
    }
    // `str(offset)` at one digit, two, and the largest 31-bit offset.
    for (offset, text) in [(1, &b"1"[..]), (10, b"10"), (0x7FFF_FFFF, b"2147483647")] {
        assert!(unityrs_core::typetree::commonstring::common_string(offset).is_none());
        let common: &[(u32, &[u8])] = &[(offset, b"Base")];
        let data = Bytes::from(blob_tree(&nodes, 17, false, Strings::Common, common));
        let mut r = Reader::new(&data, Endian::Little);
        let rows = parse_blob(
            &mut r,
            &data,
            17,
            &Meter::unlimited(),
            &Limits::default(),
            None,
        )
        .unwrap();
        assert_eq!(rows.as_slice()[0].ty.as_bytes(), text);
    }
}

/// With the caller's TPK table, a high-bit offset reads that table alone, as
/// UnityPy reads its TPK's (`get_common_strings()`): an offset the TPK's
/// table lacks is its decimal string even where UnityPy's MIT table, the one
/// read without a TPK, has a string.
#[test]
fn a_blob_reads_the_callers_common_strings_in_place_of_the_mit_table() {
    let table = CommonStrings::new(["Thing", "é", "m_X"].map(RawStr::from));
    // Thing at 0, é at 6, m_X at 8 (code points, not bytes).
    let nodes = vec![
        NodeSpec::new(0, "Thing", "Base"),
        NodeSpec::new(1, "m_X", "AABB"),
    ];
    let offsets: &[(u32, &[u8])] = &[(0, b"Thing"), (8, b"m_X"), (5, b"AABB")];
    let data = Bytes::from(blob_tree(&nodes, 17, false, Strings::Common, offsets));
    let read = |common: Option<&CommonStrings>| -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut r = Reader::new(&data, Endian::Little);
        let rows = parse_blob(
            &mut r,
            &data,
            17,
            &Meter::unlimited(),
            &Limits::default(),
            common,
        );
        rows.unwrap()
            .as_slice()
            .iter()
            .map(|row| (row.ty.as_bytes().to_vec(), row.name.as_bytes().to_vec()))
            .collect()
    };
    let with = read(Some(&table));
    assert_eq!(with[0], (b"Thing".to_vec(), b"Base".to_vec()));
    // 5 is `AnimationClip` in the MIT table, absent from this one.
    assert_eq!(with[1], (b"m_X".to_vec(), b"5".to_vec()));
    let without = read(None);
    assert_eq!(without[0], (b"AABB".to_vec(), b"Base".to_vec()));
    assert_eq!(without[1], (b"8".to_vec(), b"AnimationClip".to_vec()));
}

/// A provider with common strings only.
#[derive(Debug)]
struct Common(CommonStrings);

impl TypeTreeProvider for Common {
    fn name(&self) -> &'static str {
        "common"
    }
    fn tree(&self, _: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        Ok(None)
    }
    fn common_strings(&self) -> Option<&CommonStrings> {
        Some(&self.0)
    }
}

/// A file's blob trees read the common strings of the first provider in the
/// context's chain that has them; with none, UnityPy's MIT table.
#[test]
fn a_files_blob_trees_read_the_chains_common_strings() {
    let spec = SerializedSpec {
        types: vec![TypeSpec {
            class_id: 1,
            nodes: tree(),
            ..TypeSpec::default()
        }],
        ..SerializedSpec::default()
    };
    let mut data = build(&spec);
    // The blob's counts (2 rows, the 24-byte buffer "GameObject\0Base\0int\0m_A\0"),
    // then row 0, whose type offset (local 0) becomes common offset 0.
    let counts = [2_i32.to_le_bytes(), 24_i32.to_le_bytes()].concat();
    let at = data.windows(8).position(|w| w == counts).unwrap() + 8 + 4;
    assert_eq!(data[at..at + 4], 0_u32.to_le_bytes());
    data[at..at + 4].copy_from_slice(&0x8000_0000_u32.to_le_bytes());
    let parse_with = |providers: Vec<Arc<dyn TypeTreeProvider>>| {
        let mut c = FileContext::unitypy();
        c.providers = Arc::new(ProviderChain::new(providers));
        let c = Arc::new(c);
        let m = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
        let f = SerializedFile::parse(
            RawStr::from("CAB-t"),
            Bytes::copy_from_slice(&data),
            None,
            &m,
            &c,
        )
        .unwrap();
        let nodes = f.types()[0].nodes.clone().unwrap();
        nodes.rows()[0].ty.as_bytes().to_vec()
    };
    assert_eq!(parse_with(Vec::new()), b"AABB");
    let table = CommonStrings::new([RawStr::from("FromTheTpk")]);
    let empty = CommonStrings::default();
    assert_eq!(
        parse_with(vec![Arc::new(Common(table.clone()))]),
        b"FromTheTpk"
    );
    // The first provider with a table decides, even an empty one.
    assert_eq!(
        parse_with(vec![Arc::new(Common(empty)), Arc::new(Common(table))]),
        b"0"
    );
}

#[test]
fn negative_blob_counts_are_python_slices() {
    // `read(node_count * 24)` and `read(stringbuffer_size)` with negative
    // sizes are Python slices: `view[pos:pos + n]` counts a negative end from
    // the end of the data, and the position moves by `n` either way.
    for (count, size) in [(-1i32, 0i32), (0, -2), (-1, -2), (-100, -100), (-3, 4)] {
        let mut data = vec![0u8; 64];
        data[..4].copy_from_slice(&count.to_le_bytes());
        data[4..8].copy_from_slice(&size.to_le_bytes());
        let data = Bytes::from(data);
        let mut r = Reader::new(&data, Endian::Little);
        let got = parse_blob(
            &mut r,
            &data,
            17,
            &Meter::unlimited(),
            &Limits::default(),
            None,
        );
        let (rows, pos) = py_read(data.len(), 8, i64::from(count) * 24);
        let (_, end) = py_read(data.len(), pos, i64::from(size));
        if rows.len() % 24 == 0 {
            let got = got.unwrap();
            assert_eq!(got.len(), rows.len() / 24, "{count} {size}");
            assert_eq!(r.position(), end, "{count} {size}");
        } else {
            assert!(
                matches!(got, Err(Error::Malformed { .. })),
                "{count} {size}: {got:?}"
            );
        }
    }
    // Zero rows is UnityPy's IndexError when the tree is built.
    assert!(NodeList::trusted(Vec::new()).is_err());
}

#[test]
fn old_trees_read_child_counts_with_a_stack() {
    let nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "Sub", "s"),
        NodeSpec::new(2, "int", "a").size(4),
        NodeSpec::new(1, "int", "b").size(4),
    ];
    for format in [2u32, 3, 5, 9, 11] {
        for big in [false, true] {
            let data = Bytes::from(old_tree(&nodes, format, big));
            let mut r = Reader::new(&data, if big { Endian::Big } else { Endian::Little });
            let rows = parse_old(
                &mut r,
                &data,
                format,
                &Meter::unlimited(),
                &Limits::default(),
            )
            .unwrap();
            let levels: Vec<i32> = rows.as_slice().iter().map(|r| r.level).collect();
            assert_eq!(levels, [0, 1, 2, 1], "{format}");
            assert_eq!(r.position(), i128::try_from(data.len()).unwrap());
            assert_eq!(rows.as_slice()[0].variable_count.is_some(), format == 2);
            assert_eq!(rows.as_slice()[0].meta_flag.is_some(), format != 3);
        }
    }
    // A deep chain (each node one child) needs no stack beyond one entry.
    let chain: Vec<NodeSpec> = (0..250u8).map(|i| NodeSpec::new(i, "T", "n")).collect();
    let data = Bytes::from(old_tree(&chain, 9, false));
    let mut r = Reader::new(&data, Endian::Little);
    assert_eq!(
        parse_old(&mut r, &data, 9, &Meter::unlimited(), &Limits::default())
            .unwrap()
            .len(),
        250
    );
}

/// UnityPy builds `[dummy_node] * children_count` before it reads a child
/// (`helpers/TypeTreeNode.py:107`), so a huge count runs it out of memory
/// (`PARITY.md` section 19.3, G3b's `memory_bound`); unityrs reads children
/// until the bytes run out, allocating nothing from the count.
#[test]
fn a_huge_old_child_count_fails_at_the_end_allocating_nothing() {
    let nodes = vec![NodeSpec::new(0, "Base", "Base")];
    for format in [2u32, 3, 5, 9, 11] {
        let mut data = old_tree(&nodes, format, false);
        // The root's child count is the tree's last field.
        let end = data.len();
        data[end - 4..].copy_from_slice(&i32::MAX.to_le_bytes());
        let data = Bytes::from(data);
        let c = ctx();
        let m = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
        let mut r = Reader::new(&data, Endian::Little);
        let got = parse_old(&mut r, &data, format, &m, &Limits::default());
        assert!(got.is_err(), "{format}: {got:?}");
        drop(got);
        assert_eq!(m.used(), 0, "{format}: everything returned");
        assert!(m.granted() < 4096, "{format}: {} granted", m.granted());
    }
}

#[test]
fn mutated_files_and_trees_never_panic() {
    let inputs: Vec<(String, Vec<u8>)> = matrix()
        .into_iter()
        .filter(|(n, _)| !n.ends_with(".bundle"))
        .collect();
    let c = {
        let mut c = FileContext::unitypy();
        c.limits.max_input_memory = 64 << 20;
        Arc::new(c)
    };
    let mut rng = Rng::new(0x00C0_FFEE);
    for round in 0..10_000 {
        let (name, data) = &inputs[round % inputs.len()];
        let mut data = data.clone();
        let n = 1 + rng.below(6);
        rng.flip_bits(&mut data, n);
        if rng.below(4) == 0 {
            rng.truncate(&mut data);
        }
        let m = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
        let _ = SerializedFile::parse(
            RawStr::from(name.as_str()),
            Bytes::from(data.clone()),
            None,
            &m,
            &c,
        );
        if let Ok(ty) = check_file_type(&data, &c.limits) {
            let _ = parse_file(
                RawStr::from(name.as_str()),
                Bytes::from(data),
                ty,
                None,
                &m,
                &c,
            )
            .map(|f| matches!(f, File::Raw(_)));
        }
    }
    let nodes = tree();
    for round in 0..10_000u32 {
        let format = [9, 11, 15, 17, 19, 22][round as usize % 6];
        let mut data = if round % 2 == 0 {
            blob_tree(&nodes, format, round % 4 == 0, Strings::Local, &[])
        } else {
            old_tree(&nodes, format, round % 3 == 0)
        };
        let n = 1 + rng.below(4);
        rng.flip_bits(&mut data, n);
        let data = Bytes::from(data);
        let mut r = Reader::new(&data, Endian::Little);
        let m = Meter::new(
            1 << 20,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        let _ = if round % 2 == 0 {
            parse_blob(&mut r, &data, format, &m, &Limits::default(), None)
        } else {
            parse_old(&mut r, &data, format, &m, &Limits::default())
        };
    }
}

#[test]
fn mutated_codec_inputs_never_panic() {
    use unityrs_core::budget::DecodeBuf;
    use unityrs_core::codec::{BlockCodec, Lz4, Lzma};
    use unityrs_testkit::bundle::{TestCodec, compress};
    let plain: Vec<u8> = (0..3000u32).map(|i| (i % 13) as u8).collect();
    let lz4 = compress(TestCodec::Lz4, &plain);
    let lzma = compress(TestCodec::Lzma, &plain);
    let mut rng = Rng::new(42);
    for round in 0..10_000 {
        let (codec, src): (&dyn BlockCodec, &Vec<u8>) = if round % 2 == 0 {
            (&Lz4, &lz4)
        } else {
            (&Lzma, &lzma)
        };
        let mut data = src.clone();
        let n = 1 + rng.below(6);
        rng.flip_bits(&mut data, n);
        if rng.below(3) == 0 {
            rng.truncate(&mut data);
        }
        let m = Meter::new(
            1 << 20,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        let mut out = DecodeBuf::new(&m, 0, 1 << 16, LimitKind::BlockOutput).unwrap();
        let declared = [0u32, 1, 3000, 2999, 70_000, u32::MAX][rng.below(6)];
        let _ = codec.decode(&Bytes::from(data), declared, &mut out);
    }
}
