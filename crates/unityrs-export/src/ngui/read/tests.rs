//! The reads' unit tests (module docs): a texture's answer from its fields,
//! the integers as read, a platform blob, a stream's span and slice, and
//! the head's rows, their flags, their reads and their source.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::env::EnvOptions;
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, FileContext, PrimArray, RawStr, Value};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{self, SerializedSpec, TypeSpec};

use super::{Blob, Span, Stream, Texture, answer, field, head_rows, integer, pptr, python_slice};
use crate::ngui::classify::{Class, Reason};

#[test]
fn the_head_rows_set_only_the_align_bit() {
    // Review round 6's P1 and round 7's F1: UnityPy's align bit, written
    // here as the literal it tests (`m_MetaFlag & 0x4000`), on `m_Enabled`
    // and the string's `Array` and on no other row, every other flag 0; no
    // size.
    for wide in [true, false] {
        let rows = head_rows(wide);
        let flags: Vec<(&[u8], Option<i64>)> = rows
            .iter()
            .inspect(|r| assert_eq!(r.byte_size, 0, "{r:?}"))
            .map(|r| (r.name.as_bytes(), r.meta_flag))
            .collect();
        let want: Vec<(&[u8], Option<i64>)> = [
            ("Base", 0),
            ("m_GameObject", 0),
            ("m_FileID", 0),
            ("m_PathID", 0),
            ("m_Enabled", 0x4000),
            ("m_Script", 0),
            ("m_FileID", 0),
            ("m_PathID", 0),
            ("m_Name", 0),
            ("Array", 0x4000),
            ("size", 0),
            ("data", 0),
        ]
        .map(|(name, flag)| (name.as_bytes(), Some(flag)))
        .into();
        assert_eq!(flags, want, "{wide}");
    }
}

#[test]
fn the_head_rows_are_those_a_file_stores_by_the_public_layout() {
    // Review round 7's F3: no UnityPy sample holds a MonoBehaviour, so the
    // file's stored typetree is written from the format facts the module
    // docs cite (AssetStudio v0.16.47's MIT readers, `Component.cs`,
    // `Behaviour.cs`, `MonoBehaviour.cs` and `PPtr.cs`, and its
    // `SerializedTypeHelper.cs`, whose align flag is `m_MetaFlag` 0x4000):
    // the base's `m_GameObject`, `UInt8 m_Enabled` aligned, `m_Script`,
    // the aligned string `m_Name`, then the script's own fields. The core
    // parses the file; the head's rows are the stored tree's first rows,
    // level, type, name and align bit, in order, the narrow pointer in a
    // format 9 file of 4.7.2f1 and the wide one in a format 22 file.
    let pptr = |ty: &str, name: &str, id: &str| {
        [
            NodeSpec::new(1, ty, name),
            NodeSpec::new(2, "int", "m_FileID"),
            NodeSpec::new(2, id, "m_PathID"),
        ]
    };
    for (format, version, wide) in [(9, "4.7.2f1", false), (22, "2022.3.0f1", true)] {
        let id = if wide { "SInt64" } else { "int" };
        let mut stored = vec![NodeSpec::new(0, "MonoBehaviour", "Base")];
        stored.extend(pptr("PPtr<GameObject>", "m_GameObject", id));
        stored.push(NodeSpec::new(1, "UInt8", "m_Enabled").aligned());
        stored.extend(pptr("PPtr<MonoScript>", "m_Script", id));
        stored.extend([
            NodeSpec::new(1, "string", "m_Name"),
            NodeSpec::new(2, "Array", "Array").aligned(),
            NodeSpec::new(3, "int", "size"),
            NodeSpec::new(3, "char", "data"),
            NodeSpec::new(1, "int", "mine"),
        ]);
        let data = serialized::build(&SerializedSpec {
            format,
            unity_version: version.as_bytes().to_vec(),
            types: vec![TypeSpec {
                class_id: 114,
                nodes: stored,
                ..TypeSpec::default()
            }],
            ..SerializedSpec::default()
        });
        let c = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
        let file =
            SerializedFile::parse(RawStr::from("CAB-h"), Bytes::from(data), None, &meter, &c)
                .unwrap();
        let shape = |r: &NodeRow| {
            (
                r.level,
                r.ty.as_bytes().to_vec(),
                r.name.as_bytes().to_vec(),
                r.meta_flag.map(|m| m & 0x4000),
            )
        };
        let rows = file.types()[0].nodes.as_ref().unwrap().rows();
        let head = head_rows(wide);
        let got: Vec<_> = rows.iter().take(head.len()).map(shape).collect();
        let want: Vec<_> = head.iter().map(shape).collect();
        assert_eq!(got, want, "format {format}");
        // The head ends where the script's own fields begin.
        assert_eq!(shape(&rows[head.len()]).2, b"mine", "format {format}");
    }
}

#[test]
fn the_head_reads_a_head_laid_out_as_a_file_stores_it() {
    // Review round 6's P1: the reduced rows read the four fields of a head
    // laid out by hand, in either pointer width, by either reader:
    // `m_Enabled` padded to 4, the name's bytes padded to 4, and whatever
    // follows (the class's own fields) left unread, the bytes read the
    // head's length (round 7's F2).
    let options = ReadOptions {
        limits: Limits::default(),
        ..EnvOptions::unitypy().context.read_options()
    };
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::NoFile,
        options: &options,
        meter: &meter,
    };
    for wide in [true, false] {
        let path_id = |out: &mut Vec<u8>, v: i64| {
            if wide {
                out.extend(v.to_le_bytes());
            } else {
                out.extend(i32::try_from(v).unwrap().to_le_bytes());
            }
        };
        let mut data = Vec::new();
        data.extend(7i32.to_le_bytes());
        path_id(&mut data, -3);
        data.push(1);
        data.resize(data.len().next_multiple_of(4), 0);
        data.extend(2i32.to_le_bytes());
        path_id(&mut data, if wide { 0x0102_0304_0506 } else { 77 });
        data.extend(5i32.to_le_bytes());
        data.extend(b"atlas");
        data.resize(data.len().next_multiple_of(4), 0);
        let head = i64::try_from(data.len()).unwrap();
        data.extend([0xEE; 8]);
        let data = Bytes::from(data);
        let list = NodeList::trusted(head_rows(wide)).unwrap();
        let tree = Tree::compile(Arc::new(list), &Limits::default()).unwrap();
        let size = u32::try_from(data.len()).unwrap();
        for byte_size in [size, 0] {
            let parsed = walk::read_typetree(&data, 0, byte_size, &tree, &ctx, false).unwrap();
            let root = parsed.value();
            let script = field(&parsed, root, b"m_Script")
                .and_then(|v| pptr(&parsed, v))
                .unwrap();
            assert_eq!(script.file_id, 2, "{wide} {byte_size}");
            let want = if wide { 0x0102_0304_0506 } else { 77 };
            assert_eq!(script.path_id, want, "{wide} {byte_size}");
            let Some(Value::Str(name)) = field(&parsed, root, b"m_Name") else {
                panic!("{wide} {byte_size}: no name");
            };
            assert_eq!(name.as_bytes(), b"atlas", "{wide} {byte_size}");
            let game_object = field(&parsed, root, b"m_GameObject")
                .and_then(|v| pptr(&parsed, v))
                .unwrap();
            assert_eq!((game_object.file_id, game_object.path_id), (7, -3));
            // Round 7's F2: the read ends at the head, the tail unread.
            assert_eq!(parsed.bytes_read, head, "{wide} {byte_size}");
        }
    }
}

fn tex(width: i128, height: i128, format: i128) -> Texture {
    Texture {
        width,
        height,
        format,
        image_data: Bytes::new(),
        stream: None,
        platform_blob: None,
    }
}

fn skip(reason: &str) -> Result<unityrs_texture::Format, Class> {
    Err(Class::Skip(Reason::plain(reason)))
}

#[test]
fn any_integer_is_kept_wide() {
    assert_eq!(integer(&Value::Int(-1)), Some(-1));
    assert_eq!(integer(&Value::UInt(u64::MAX)), Some(i128::from(u64::MAX)));
    assert_eq!(integer(&Value::Bool(true)), None);
}

#[test]
fn the_answers_narrow_only_where_they_use_a_value() {
    // A side of 0 is `empty_image` whatever the other side and the
    // format, however wide.
    assert_eq!(
        answer(&tex(0, i128::from(u64::MAX), 1 << 40), 13),
        skip("empty_image")
    );
    // A format outside `i32` is its decimal.
    assert_eq!(
        answer(&tex(1, 1, 1 << 40), 13),
        skip("unsupported_format(1099511627776)")
    );
    assert_eq!(
        answer(&tex(1, 1, -(1 << 40)), 13),
        skip("unsupported_format(-1099511627776)")
    );
    // A side past `i32` reaches the decoder as `i32::MAX`, more pixels
    // than its bound; one below as `i32::MIN`, negative.
    let wide = tex(i128::from(u32::MAX), 1, 4);
    assert_eq!(wide.sides(), (i32::MAX, 1));
    assert!(answer(&wide, 13).is_ok());
    assert_eq!(tex(-(1 << 40), 1, 4).sides(), (i32::MIN, 1));
}

#[test]
fn a_switch_blob_is_read_as_bytes() {
    let blob = |len, gobs| Some(Blob { len, gobs });
    let mut t = tex(1, 1, 4);
    t.platform_blob = blob(12, Some([1, 0, 0, 0]));
    assert_eq!(answer(&t, 38), skip("unsupported_platform(switch)"));
    // Off Switch the blob is never read.
    assert!(answer(&t, 13).is_ok());
    t.platform_blob = blob(12, Some([0; 4]));
    assert!(answer(&t, 38).is_ok());
    // Fewer than 12 elements: not swizzled, whatever they hold.
    t.platform_blob = blob(11, None);
    assert!(answer(&t, 38).is_ok());
    // An element 8 to 12 that is no byte: `int.from_bytes` raises.
    t.platform_blob = blob(12, None);
    assert_eq!(
        answer(&t, 38),
        Err(Class::Skip(Reason::decode_error("read")))
    );
    // ... for a texture of the five formats only: another is answer 2.
    t.format = 1;
    assert_eq!(answer(&t, 38), skip("unsupported_format(Alpha8)"));
}

#[test]
fn a_blob_of_any_list_is_read_without_a_copy() {
    let ints = |v: Vec<i32>| Value::Prims(PrimArray::I32(v.into_boxed_slice()));
    let mut twelve = vec![0; 12];
    twelve[8] = 2;
    assert_eq!(
        Blob::of(&ints(twelve.clone())),
        Some(Blob {
            len: 12,
            gobs: Some([2, 0, 0, 0])
        })
    );
    twelve[9] = 300;
    assert_eq!(
        Blob::of(&ints(twelve)),
        Some(Blob {
            len: 12,
            gobs: None
        })
    );
    let bools = Value::Prims(PrimArray::Bool(vec![true; 12].into_boxed_slice()));
    assert_eq!(
        Blob::of(&bools),
        Some(Blob {
            len: 12,
            gobs: Some([1; 4])
        })
    );
    let floats = Value::Prims(PrimArray::F32(vec![0; 12].into_boxed_slice()));
    assert_eq!(
        Blob::of(&floats),
        Some(Blob {
            len: 12,
            gobs: None
        })
    );
    let list = Value::List((0..12).map(Value::Int).collect());
    assert_eq!(
        Blob::of(&list),
        Some(Blob {
            len: 12,
            gobs: Some([8, 9, 10, 11])
        })
    );
    assert_eq!(
        Blob::of(&Value::List(Vec::new())),
        Some(Blob { len: 0, gobs: None })
    );
    assert_eq!(Blob::of(&Value::Bytes(Bytes::new())), None);
}

#[test]
fn a_stream_outside_u64_is_sliced_as_python_slices() {
    let s = |offset, size| Stream {
        path: "r".into(),
        offset,
        size,
    };
    assert_eq!(s(5, 4).span(), Span::Range(5, 4));
    assert_eq!(s(-4, 12).span(), Span::Slice(-4, 8));
    assert_eq!(s(0, -4).span(), Span::Slice(0, -4));
    // The memoryview reader's `read(size)` (review round 2's C1): 0
    // reads nothing, -1 to the end, from a negative offset too.
    let over = |offset, size| match s(offset, size).span() {
        Span::Range(o, n) => {
            let o = usize::try_from(o).unwrap().min(8);
            o..usize::try_from(o as u64 + n).unwrap().min(8)
        }
        Span::Slice(a, b) => python_slice(8, a, b),
    };
    assert_eq!(over(0, -1), 0..8);
    assert_eq!(over(-4, -1), 4..8);
    assert_eq!(over(5, -1), 5..8);
    assert_eq!(over(12, -1), 8..8);
    assert!(over(5, 0).is_empty());
    assert!(over(-4, 0).is_empty());
    assert_eq!(over(0, -2), 0..6);
    let wide = i128::from(i64::MAX) * 4;
    assert_eq!(over(2, wide), 2..8);
    assert_eq!(over(-wide, 3), 0..0);
    assert_eq!(
        s(i128::from(u64::MAX), 1).span(),
        Span::Slice(i128::from(u64::MAX), i128::from(u64::MAX) + 1)
    );
    assert_eq!(python_slice(8, -4, 8), 4..8);
    assert_eq!(python_slice(8, 0, -4), 0..4);
    assert_eq!(python_slice(8, -1, 0), 7..7);
    assert_eq!(python_slice(8, -20, 3), 0..3);
    assert_eq!(python_slice(8, 9, 20), 8..8);
    assert_eq!(python_slice(8, 6, 2), 6..6);
}
