//! Platform transforms retain every decoded format and the selected Crunch fork.
use serde_json::Value as J;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_texture::{
    Format, Formats, Selection, TextureFields, XBOX_SWAP_FORMATS, XBOX360, decode, decode_in_as,
    plan, plan_in_as, select_in,
};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn fields(format: Format, width: i32, height: i32) -> TextureFields<'static> {
    TextureFields {
        texture_format: format.texture_format(),
        width,
        height,
        platform: XBOX360,
        platform_blob: None,
    }
}

#[test]
fn xbox_applies_its_swap_table_to_every_plain_format_including_closed_forms() {
    for format in Format::ALL.into_iter().filter(|f| !f.is_crunched()) {
        let fields = fields(format, 16, 8);
        let base = plan(format, 1 << 20, fields.width, fields.height).unwrap();
        let mut data: Vec<_> = (0..=base.input_bytes)
            .map(|n| u8::try_from(n % 251).unwrap())
            .collect();
        let expected = decode(
            format,
            &data,
            fields.width,
            fields.height,
            &Meter::unlimited(),
        )
        .unwrap();
        let swap = XBOX_SWAP_FORMATS.contains(&fields.texture_format);
        if swap {
            for pair in data.as_chunks_mut::<2>().0 {
                pair.swap(0, 1);
            }
        }
        let meter = Meter::unlimited();
        let p = plan_in_as(format, &fields, data.len()).unwrap();
        assert_eq!(
            p.scratch,
            base.scratch
                + if swap {
                    heap_cost(data.len() as u64)
                } else {
                    0
                }
        );
        let actual = decode_in_as(format, &fields, &data, &meter).unwrap();
        assert_eq!(actual.mode(), expected.mode(), "{format:?}");
        assert_eq!(actual.pixels(), expected.pixels(), "{format:?}");
        assert_eq!(meter.used(), p.charge(), "{format:?}");
        drop(actual);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn xbox_transforms_both_crunch_forks_before_unpacking_and_keeps_etc_unchanged() {
    for (fixture, version) in [
        (include_str!("fixtures/crunch_native.json"), (2017, 2)),
        (include_str!("fixtures/unitycrunch_native.json"), (2017, 3)),
    ] {
        let doc: J = serde_json::from_str(fixture).unwrap();
        for case in doc["cases"].as_array().unwrap() {
            let value = i32::try_from(case["format"].as_i64().unwrap()).unwrap();
            let fields = fields(
                Format::from_texture_format(value).unwrap(),
                i32::try_from(case["width"].as_i64().unwrap()).unwrap(),
                i32::try_from(case["height"].as_i64().unwrap()).unwrap(),
            );
            let Selection::Export(format) = select_in(&fields, Formats::DECODED, version) else {
                panic!("not selected: {}", case["id"]);
            };
            let mut data = unhex(case["data"]["hex"].as_str().unwrap());
            let expected = decode(
                format,
                &data,
                fields.width,
                fields.height,
                &Meter::unlimited(),
            )
            .unwrap();
            if XBOX_SWAP_FORMATS.contains(&value) {
                for pair in data.as_chunks_mut::<2>().0 {
                    pair.swap(0, 1);
                }
            }
            let meter = Meter::unlimited();
            let p = plan_in_as(format, &fields, data.len()).unwrap();
            let actual = decode_in_as(format, &fields, &data, &meter)
                .unwrap_or_else(|e| panic!("{} {format:?}: {e}", case["id"]));
            assert_eq!(actual.pixels(), expected.pixels(), "{}", case["id"]);
            assert_eq!(meter.used(), p.charge());
            drop(actual);
            assert_eq!(meter.used(), 0);
        }
    }
}
