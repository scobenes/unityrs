use super::*;
use unityrs_core::{
    ValueBuilder,
    typetree::{NodeList, NodeRow},
};
#[test]
fn oversized_header_refuses_before_copying_or_rendering() {
    let meter = Meter::unlimited();
    let nodes = NodeList::trusted(vec![NodeRow::new(0, "int", "Base")])
        .unwrap()
        .into_shared()
        .unwrap();
    let builder = ValueBuilder::new(&meter, nodes).unwrap();
    let scalar = builder.scalar(Value::Int(1)).unwrap();
    let parsed = builder.finish(scalar, 0);
    let raw = [
        b"input".to_vec(),
        b"--path-id".to_vec(),
        b"1".to_vec(),
        b"--max-bytes".to_vec(),
        b"512".to_vec(),
    ];
    let args = Args::parse("show", &raw).unwrap();
    let header = format!("{{\"name\":\"{}\"}}", "x".repeat(4 << 20));
    let before = meter.decoded();
    crate::test_alloc::LARGEST.set(Some(0));
    let result = show(
        &args,
        &parsed,
        header.len(),
        |out| {
            out.push_str(&header);
            Ok(())
        },
        &meter,
    );
    let largest = crate::test_alloc::LARGEST.replace(None).unwrap();
    assert_eq!(result.unwrap_err().code, "budget");
    assert!(
        largest < 16_384,
        "small cap copied a huge header: {largest} bytes"
    );
    assert_eq!(meter.decoded(), before, "refuse before rendering the value");
}
#[test]
fn duplicate_and_raw_field_names_preserve_pairs() {
    use unityrs_core::typetree::NodeId;
    let meter = Meter::unlimited();
    for (second_name, expected) in [
        (
            unityrs_core::RawStr::from("key"),
            serde_json::json!({"t":"dict","v":[["key",1],["key",2]]}),
        ),
        (
            unityrs_core::RawStr::from(&b"\xff"[..]),
            serde_json::json!({"t":"dict","v":[["key",1],[{"t":"str","hex":"ff"},2]]}),
        ),
    ] {
        let nodes = NodeList::trusted(vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "int", "key"),
            NodeRow::new(1, "int", second_name),
        ])
        .unwrap()
        .into_shared()
        .unwrap();
        let builder = ValueBuilder::new(&meter, nodes).unwrap();
        let mut entries = builder.vec();
        entries
            .push_entry(NodeId(1), builder.scalar(Value::Int(1)).unwrap())
            .unwrap();
        entries
            .push_entry(NodeId(2), builder.scalar(Value::Int(2)).unwrap())
            .unwrap();
        let value = builder.map(0, entries);
        let parsed = builder.finish(value, 0);
        let mut view = View {
            parsed: &parsed,
            meter: &meter,
            max_items: 100,
            max_depth: 32,
            full: false,
            canonical: false,
            cuts: Cuts::default(),
        };
        let rendered = view.render(parsed.value(), 0, 10000).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rendered.json).unwrap(),
            expected
        );
    }
}

fn parsed_scalar(value: Value) -> Parsed {
    let meter = Meter::unlimited();
    let nodes = NodeList::trusted(vec![NodeRow::new(0, "int", "Base")])
        .unwrap()
        .into_shared()
        .unwrap();
    let builder = ValueBuilder::new(&meter, nodes).unwrap();
    let built = builder.scalar(value).unwrap();
    builder.finish(built, 0)
}
fn work_meter(work: u64) -> Arc<Meter> {
    use unityrs_core::budget::{Reservation, Unbounded};
    Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input)
}
fn view<'a>(parsed: &'a Parsed, meter: &'a Arc<Meter>, canonical: bool) -> View<'a> {
    View {
        parsed,
        meter,
        max_items: 256,
        max_depth: 32,
        full: true,
        canonical,
        cuts: Cuts::default(),
    }
}

#[test]
fn numeric_formatting_and_markers_refuse_work_before_writing() {
    for value in [
        Value::Int(i64::MIN),
        Value::UInt(u64::MAX),
        Value::F32(1.5_f32.to_bits()),
        Value::F64(1.5_f64.to_bits()),
    ] {
        let parsed = parsed_scalar(value);
        for canonical in [false, true] {
            let meter = work_meter(64);
            assert_eq!(
                view(&parsed, &meter, canonical)
                    .render(parsed.value(), 0, 1024)
                    .unwrap_err()
                    .code,
                "budget"
            );
        }
    }
    let meter = work_meter(64);
    let mut out = Text::on(&meter);
    marker(&mut out, "items", usize::MAX);
    assert!(
        out.as_str().is_empty(),
        "refuse before constructing the marker"
    );
    assert_eq!(document(out).unwrap_err().code, "budget");
}

#[test]
fn canonical_key_escaping_is_charged_and_cuts_cannot_hide_refusals() {
    use unityrs_core::typetree::NodeId;
    let build_meter = Meter::unlimited();
    let nodes = NodeList::trusted(vec![
        NodeRow::new(0, "Base", "Base"),
        NodeRow::new(1, "int", "\n".repeat(10_000).as_str()),
    ])
    .unwrap()
    .into_shared()
    .unwrap();
    let builder = ValueBuilder::new(&build_meter, nodes).unwrap();
    let mut entries = builder.vec();
    entries
        .push_entry(NodeId(1), builder.scalar(Value::Int(1)).unwrap())
        .unwrap();
    let built = builder.map(0, entries);
    let parsed = builder.finish(built, 0);
    for cap in [1, 100_000] {
        let meter = work_meter(4096);
        assert_eq!(
            view(&parsed, &meter, true)
                .render(parsed.value(), 0, cap)
                .unwrap_err()
                .code,
            "budget"
        );
    }
}

#[test]
fn every_primitive_run_and_float_list_matches_the_canonical_encoder() {
    use unityrs_core::PrimRun;
    let build_meter = Meter::unlimited();
    let nodes = NodeList::trusted(vec![NodeRow::new(0, "vector", "Base")])
        .unwrap()
        .into_shared()
        .unwrap();
    let mut cases = Vec::new();
    macro_rules! run {
        ($variant:ident, $values:expr) => {
            for values in [&$values[..], &[][..]] {
                let builder = ValueBuilder::new(&build_meter, nodes.clone()).unwrap();
                let mut items = builder.vec();
                items.extend_from_slice(values).unwrap();
                let built = builder.prims(PrimRun::$variant(items)).unwrap();
                cases.push(builder.finish(built, 0));
            }
        };
    }
    run!(Bool, [false, true]);
    run!(I8, [i8::MIN, i8::MAX]);
    run!(I16, [i16::MIN, i16::MAX]);
    run!(U16, [0, u16::MAX]);
    run!(I32, [i32::MIN, i32::MAX]);
    run!(U32, [0, u32::MAX]);
    run!(I64, [i64::MIN, i64::MAX]);
    run!(U64, [0, u64::MAX]);
    run!(F32, [0x3fc0_0000, 0x4020_0000]);
    run!(F64, [0x3ff8_0000_0000_0000, 0x4004_0000_0000_0000]);
    for values in [
        vec![Value::F32(0x3fc0_0000), Value::F32(0x4020_0000)],
        vec![
            Value::F64(0x3ff8_0000_0000_0000),
            Value::F64(0x4004_0000_0000_0000),
        ],
        vec![Value::F32(0), Value::Int(1)],
        vec![],
    ] {
        let builder = ValueBuilder::new(&build_meter, nodes.clone()).unwrap();
        let mut items = builder.vec();
        for value in values {
            items.push_built(builder.scalar(value).unwrap()).unwrap();
        }
        let built = builder.list(items);
        cases.push(builder.finish(built, 0));
    }
    for parsed in cases {
        let mut expected = String::new();
        encode_value_into(&mut expected, &parsed, parsed.value()).unwrap();
        let meter = Meter::unlimited();
        let actual = view(&parsed, &meter, true)
            .render(parsed.value(), 0, 10000)
            .unwrap();
        assert_eq!(actual.json, expected);
        let mut clipped = view(&parsed, &meter, true);
        clipped.max_items = 1;
        let output = clipped.render(parsed.value(), 0, 10000).unwrap();
        let _: serde_json::Value = serde_json::from_str(&output.json).unwrap();
        if !matches!(parsed.value(), Value::List(items) if items.is_empty()) {
            let tight = work_meter(64);
            assert_eq!(
                view(&parsed, &tight, true)
                    .render(parsed.value(), 0, 10000)
                    .unwrap_err()
                    .code,
                "budget"
            );
        }
    }
}
