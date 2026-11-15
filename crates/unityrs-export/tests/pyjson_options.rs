//! Encoding modes, scalar boundaries and arbitrary byte strings.
use unityrs_core::{
    RawStr, Value, ValueBuilder,
    limits::Meter,
    typetree::{NodeId, NodeList, NodeRow},
};
use unityrs_export::pyjson::{self, Options};
#[test]
fn non_ascii_and_surrogateescaped_keys_preserve_insertion_order() {
    let nodes = NodeList::trusted(vec![
        NodeRow::new(0, "Root", "Base"),
        NodeRow::new(1, "string", RawStr::from_static(b"z\xff")),
        NodeRow::new(1, "string", "é"),
    ])
    .unwrap()
    .into_shared()
    .unwrap();
    let b = ValueBuilder::new(&Meter::unlimited(), nodes).unwrap();
    let mut entries = b.vec();
    entries
        .push((
            NodeId(1),
            Value::Str(RawStr::from_static(b"\xff\xe2\x80\xa8")),
        ))
        .unwrap();
    entries.push((NodeId(2), Value::Int(7))).unwrap();
    let map = b.map(0, entries);
    let parsed = b.finish(map, 0);
    let mut out = Vec::new();
    pyjson::value_with(
        &parsed,
        parsed.value(),
        0,
        Options::MONO_BEHAVIOUR,
        &mut out,
        &Meter::unlimited(),
    )
    .unwrap();
    assert_eq!(
        out,
        b"{\n    \"z\xff\": \"\xff\xe2\x80\xa8\",\n    \"\xc3\xa9\": 7\n}"
    );
    let mut out = Vec::new();
    pyjson::value(&parsed, parsed.value(), 0, &mut out, &Meter::unlimited()).unwrap();
    assert_eq!(
        out,
        b"{\n  \"z\\udcff\": \"\\udcff\\u2028\",\n  \"\\u00e9\": 7\n}"
    );
}
#[test]
fn stack_numeric_formatting_matches_the_existing_python_repr_and_integer_boundaries() {
    let mut seed = 0xe1_f10a7u64;
    for _ in 0..10_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let x = f64::from_bits(seed);
        let mut out = Vec::new();
        pyjson::float(x, &mut out);
        assert_eq!(out, pyjson::float_repr(x).as_bytes());
    }
    for x in [i128::MIN, i128::MAX, 0, -1, i128::from(u64::MAX)] {
        let mut out = Vec::new();
        pyjson::int(x, &mut out);
        assert_eq!(out, x.to_string().as_bytes());
    }
}
