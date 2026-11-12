//! The construction model's unit tests (B3's review round 3, T2): numbers
//! as `==` compares them, a tag's truth and first item, how an item
//! unpacks, a walk over a map that is no list, and a `PPtr`'s construction,
//! width and truth.

use std::sync::Arc;

use unityrs_core::env::EnvOptions;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed, PrimArray, RawStr, Value};

use super::{
    Ids, Num, Ptr, Tag, Unpacked, attaches, first_char, pointer, render, tag, truthy, unpack, walk,
};

const ALIGN: i32 = 0x4000;

/// `rows` (level, type, name, aligned) read from `data`.
fn parse(rows: &[(u8, &str, &str, bool)], data: &[u8]) -> Parsed {
    let rows = rows
        .iter()
        .map(|(level, ty, name, aligned)| {
            NodeRow::new(i32::from(*level), *ty, *name).with_meta_flag(if *aligned {
                ALIGN
            } else {
                0
            })
        })
        .collect();
    let list = NodeList::trusted(rows).unwrap();
    let tree = Tree::compile(Arc::new(list), &Limits::default()).unwrap();
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
    let data = Bytes::from(data.to_vec());
    let size = u32::try_from(data.len()).unwrap();
    walk::read_typetree(&data, 0, size, &tree, &ctx, false).unwrap()
}

/// A root `Base` of one pointer child `m_Ptr` of type `ty`, its children
/// `kids` (an `int` each, or a `UInt64` named `m_PathID`) of `values`.
fn one_pointer(ty: &str, kids: &[(&str, &str)], values: &[u64]) -> Parsed {
    let mut rows = vec![(0, "Base", "Base", false), (1, ty, "m_Ptr", false)];
    let mut data = Vec::new();
    for ((kind, name), v) in kids.iter().zip(values) {
        rows.push((2, kind, name, false));
        match *kind {
            "UInt64" => data.extend(v.to_le_bytes()),
            _ => data.extend(u32::try_from(*v).unwrap().to_le_bytes()),
        }
    }
    if kids.is_empty() {
        // A pointer of no children: a value of its own, an `int`'s.
        data.extend([0; 4]);
    }
    parse(&rows, &data)
}

fn child(parsed: &Parsed) -> &Value {
    let Value::Dict(root) = parsed.value() else {
        panic!("no root");
    };
    parsed.attr(root, b"m_Ptr").unwrap()
}

#[test]
fn numbers_equal_by_value_whatever_their_type() {
    let five = Num::of(&Value::Int(5));
    assert_eq!(Num::of(&Value::F64(5.0f64.to_bits())), five);
    assert_eq!(Num::of(&Value::F32(5.0f32.to_bits())), five);
    assert_eq!(Num::of(&Value::Bool(true)), Num::of(&Value::Int(1)));
    assert_eq!(Num::of(&Value::Bool(false)), Num::of(&Value::Int(0)));
    assert_eq!(Num::of(&Value::F64(f64::NAN.to_bits())), None);
    assert_eq!(
        Num::of(&Value::UInt(u64::MAX)),
        Some(Num::Int(i128::from(u64::MAX)))
    );
    assert!(matches!(
        Num::of(&Value::F64(2.5f64.to_bits())),
        Some(Num::Float(_))
    ));
    assert_eq!(Num::of(&Value::Str(RawStr::from("5"))), None);
}

#[test]
fn a_tag_is_its_first_item_where_true_and_indexable() {
    let p = one_pointer("int", &[], &[]);
    let meter = Meter::unlimited();
    let tag_of = |v: &Value| format!("{:?}", tag(&p, Some(v), &meter).unwrap());
    let is = |t: Tag<'_>| format!("{t:?}");
    // Not indexable, and true: `m_AtlasTags[0]` raises.
    for v in [
        Value::Int(3),
        Value::Bool(true),
        Value::F32(0.5f32.to_bits()),
    ] {
        assert_eq!(tag_of(&v), is(Tag::Raises), "{v:?}");
    }
    // False: no lookup.
    for v in [
        Value::Int(0),
        Value::Bool(false),
        Value::F64(0.0f64.to_bits()),
    ] {
        assert_eq!(tag_of(&v), is(Tag::None), "{v:?}");
    }
    // A NaN is true.
    assert_eq!(tag_of(&Value::F64(f64::NAN.to_bits())), is(Tag::Raises));
    // A string's first item is its first character, which a `str` holds
    // as one code point, or one byte that is none.
    let s = |b: &[u8]| Value::Str(RawStr::from(b.to_vec()));
    assert_eq!(
        tag_of(&s("\u{e9}x".as_bytes())),
        is(Tag::Name("\u{e9}".as_bytes()))
    );
    assert_eq!(tag_of(&s(b"\xffab")), is(Tag::Name(b"\xff")));
    assert_eq!(tag_of(&s(b"")), is(Tag::None));
    assert_eq!(first_char(b"ab"), b"a");
    // A list's first item, a string or not.
    let list = Value::List(vec![s(b"atlas"), Value::Int(1)]);
    assert_eq!(tag_of(&list), is(Tag::Name(b"atlas")));
    assert_eq!(tag_of(&Value::List(vec![Value::Int(1)])), is(Tag::NoMatch));
    assert_eq!(tag_of(&Value::List(Vec::new())), is(Tag::None));
    let bytes = Value::Bytes(Bytes::from(vec![1u8]));
    assert_eq!(tag_of(&bytes), is(Tag::NoMatch));
}

#[test]
fn a_pointer_tag_is_true_by_its_path_id() {
    let meter = Meter::unlimited();
    let kids = [("int", "m_FileID"), ("int", "m_PathID")];
    let zero = one_pointer("PPtr<SpriteAtlas>", &kids, &[3, 0]);
    assert_eq!(tag(&zero, Some(child(&zero)), &meter).unwrap(), Tag::None);
    let set = one_pointer("PPtr<SpriteAtlas>", &kids, &[0, 7]);
    assert_eq!(tag(&set, Some(child(&set)), &meter).unwrap(), Tag::Raises);
    // Not a `PPtr` (an `UnknownObject`): true, whatever its path id.
    let other = one_pointer("NotAPointer", &kids, &[0, 0]);
    assert_eq!(
        tag(&other, Some(child(&other)), &meter).unwrap(),
        Tag::Raises
    );
}

#[test]
fn a_pointer_is_a_constructed_pptr_of_full_width_ids() {
    let meter = Meter::unlimited();
    let ptr = |p: &Parsed| pointer(p, child(p), &meter).unwrap();
    let kids = [("int", "m_FileID"), ("int", "m_PathID")];
    let p = one_pointer("PPtr<Texture2D>", &kids, &[1, 9]);
    let ids = Ids {
        file_id: 1,
        path_id: 9,
    };
    assert_eq!(ptr(&p), Ptr::Ids(ids));
    // `parse_class` sets `assetsfile` itself: a child of that name changes
    // nothing.
    let with_file = [
        ("int", "m_FileID"),
        ("int", "m_PathID"),
        ("int", "assetsfile"),
    ];
    let p = one_pointer("PPtr<Texture2D>", &with_file, &[1, 9, 4]);
    assert_eq!(ptr(&p), Ptr::Ids(ids));
    // Another field, a field missing or another type: no `PPtr`, but true.
    let extra = [("int", "m_FileID"), ("int", "m_PathID"), ("int", "more")];
    let p = one_pointer("PPtr<Texture2D>", &extra, &[1, 9, 4]);
    assert_eq!(ptr(&p), Ptr::Other(true));
    let p = one_pointer("PPtr<Texture2D>", &kids[..1], &[1]);
    assert_eq!(ptr(&p), Ptr::Other(true));
    let p = one_pointer("PPtr", &kids, &[1, 9]);
    assert_eq!(ptr(&p), Ptr::Other(true));
    assert_eq!(Ptr::Other(true).ids(), None);
    // A `UInt64` path id past `2^63` is kept.
    let wide = [("int", "m_FileID"), ("UInt64", "m_PathID")];
    let big = 1u64 << 63 | 5;
    let p = one_pointer("PPtr<Texture2D>", &wide, &[0, big]);
    let got = ptr(&p);
    assert_eq!(got.ids().map(|i| i.path_id), Some(i128::from(big)));
    assert!(got.is_set());
    // A number is no pointer, true as it is.
    let p = one_pointer("int", &[], &[]);
    assert_eq!(
        pointer(&p, &Value::Int(0), &meter).unwrap(),
        Ptr::Other(false)
    );
    assert!(truthy(&p, &Value::Int(2), &meter).unwrap());
}

#[test]
fn a_pointer_whose_children_share_a_name_takes_the_last() {
    // B3's review round 4, R8: `parse_class`'s keyword dict keeps a name's
    // first place and its last value, so a `PPtr` whose first child's name
    // comes again is still a `PPtr`, of the later id.
    let meter = Meter::unlimited();
    let kids = [
        ("int", "m_FileID"),
        ("int", "m_FileID"),
        ("int", "m_PathID"),
    ];
    let p = one_pointer("PPtr<Texture2D>", &kids, &[7, 1, 9]);
    let ids = Ids {
        file_id: 1,
        path_id: 9,
    };
    assert_eq!(pointer(&p, child(&p), &meter).unwrap(), Ptr::Ids(ids));
    let kids = [
        ("int", "m_FileID"),
        ("int", "m_PathID"),
        ("int", "m_PathID"),
    ];
    let p = one_pointer("PPtr<Texture2D>", &kids, &[1, 3, 9]);
    assert_eq!(pointer(&p, child(&p), &meter).unwrap(), Ptr::Ids(ids));
}

#[test]
fn a_root_of_a_class_of_no_set_object_reader_does_not_attach() {
    let meter = Meter::unlimited();
    let root = |ty: &str, kids: &[&str]| {
        let mut rows = vec![(0, ty, "Base", false)];
        rows.extend(kids.iter().map(|k| (1, "int", *k, false)));
        parse(&rows, &vec![0u8; 4 * kids.len()])
    };
    let words = ["data_0_", "data_1_", "data_2_", "data_3_"];
    assert!(!attaches(&root("GUID", &words), &meter).unwrap());
    assert!(!attaches(&root("PPtr<Object>", &["m_FileID", "m_PathID"]), &meter).unwrap());
    // An `UnknownObject` (a `GUID` of a fifth word) attaches.
    let five = ["data_0_", "data_1_", "data_2_", "data_3_", "data_4_"];
    assert!(attaches(&root("GUID", &five), &meter).unwrap());
    assert!(attaches(&root("SpriteAtlas", &["m_Name"]), &meter).unwrap());
    // A child that shadows the method fails any read.
    assert!(!attaches(&root("SpriteAtlas", &["set_object_reader"]), &meter).unwrap());
}

#[test]
fn an_item_unpacks_into_two_names() {
    let s = |b: &str| Value::Str(RawStr::from(b));
    let pair = Value::Tuple(Box::new([Value::Int(1), Value::Int(2)]));
    assert!(matches!(unpack(&pair), Some(Unpacked::Pair(..))));
    // Two characters, numbers or bytes: two names, neither a key.
    for v in [
        s("ab"),
        s("\u{e9}!"),
        Value::Bytes(Bytes::from(vec![1u8, 2])),
        Value::Prims(PrimArray::I8(Box::new([1, 2]))),
    ] {
        assert_eq!(unpack(&v), Some(Unpacked::Opaque), "{v:?}");
    }
    // Any other count, or a value that is no sequence, does not unpack.
    for v in [
        s("a"),
        s("abc"),
        s(""),
        Value::Bytes(Bytes::from(vec![1u8, 2, 3])),
        Value::List(vec![Value::Int(1)]),
        Value::Int(12),
    ] {
        assert_eq!(unpack(&v), None, "{v:?}");
    }
}

#[test]
fn a_walk_of_a_map_that_is_no_list_is_render_or_empty() {
    let meter = Meter::unlimited();
    let string = |s: &[u8]| {
        let mut data = i32::try_from(s.len()).unwrap().to_le_bytes().to_vec();
        data.extend(s);
        let rows = [
            (0, "SpriteAtlas", "Base", false),
            (1, "string", "m_RenderDataMap", false),
            (2, "Array", "Array", true),
            (3, "int", "size", false),
            (3, "char", "data", false),
        ];
        parse(&rows, &data)
    };
    // Each walk's items visited, and its failure.
    let walked = |p: &Parsed| {
        let mut visited = 0;
        let failed = walk(p, &meter, |_| {
            visited += 1;
            Ok(true)
        })
        .unwrap();
        (failed, visited)
    };
    // A string's items are characters, which do not unpack into two.
    assert_eq!(walked(&string(b"ab")), (Some(render()), 0));
    // An empty one has none: no match, the walk's own `StopIteration`.
    assert_eq!(walked(&string(b"")), (None, 0));
    // A number is no sequence.
    let number = parse(
        &[
            (0, "SpriteAtlas", "Base", false),
            (1, "int", "m_RenderDataMap", false),
        ],
        &[1, 0, 0, 0],
    );
    assert_eq!(walked(&number), (Some(render()), 0));
    // None: the attribute's `AttributeError`.
    let none = parse(
        &[
            (0, "SpriteAtlas", "Base", false),
            (1, "int", "m_Name", false),
        ],
        &[0; 4],
    );
    assert_eq!(walked(&none), (Some(render()), 0));
}
