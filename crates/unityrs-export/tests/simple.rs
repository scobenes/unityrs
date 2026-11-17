use unityrs_core::{
    Bytes, File, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_export::simple::{self, Fallback};
#[path = "support/simple_cases.rs"]
mod cases;
fn load(input: Vec<u8>) -> (Environment, unityrs_core::ObjectRef) {
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("synthetic.assets"),
            data: Bytes::from(input),
        })
        .unwrap()
    else {
        panic!("file")
    };
    (env, file.object(1).unwrap())
}
#[test]
fn text_and_font_extensions_and_raw_bytes() {
    for case in cases::cases().into_iter().filter(|c| c.class_id != 114) {
        let (env, obj) = load(case.input);
        let result = simple::export(&obj, &env, case.extension, &Meter::unlimited()).unwrap();
        if case.name == "font_empty" {
            assert!(result.is_none());
            continue;
        }
        let result = result.unwrap();
        let (ext, bytes): (&str, &[u8]) = match case.name {
            "text_unicode" => (".txt", "héllo 😀\n".as_bytes()),
            "text_surrogateescape" => (".bytes", b"a\xff\xe4\xb8z\0"),
            "text_empty" => (".json", b""),
            "text_multiple_dots" => (".a.b", b"multi"),
            "font_otto" => (".ttf", b"OTTO\0\xff"),
            "font_bytes_otto" => (".otf", b"OTTO\0\xff"),
            "font_bytes_ttf" | "font_ttf" => (".ttf", b"\0\x01\0\0"),
            _ => unreachable!(),
        };
        assert_eq!(result.extension, ext, "{}", case.name);
        assert_eq!(result.bytes.as_slice(), bytes, "{}", case.name);
    }
}
#[test]
fn mono_json_and_fallbacks() {
    for case in cases::cases().into_iter().filter(|c| c.class_id == 114) {
        let (env, obj) = load(case.input);
        let result = simple::export(&obj, &env, case.extension, &Meter::unlimited())
            .unwrap()
            .unwrap();
        if case.name == "mono_json" {
            assert_eq!(result.extension, ".json");
            assert_eq!(result.fallback, None);
            let mut expected = b"{\n    \"z\": \"".to_vec();
            expected.extend_from_slice("é😀\u{7f}".as_bytes());
            expected.extend_from_slice(b"\\n\\u0001\",\n    \"bad\": \"\xff\xed\xa0\x80\",\n    ");
            expected.extend_from_slice("\"clé\": [\n        -0.0,\n        NaN,\n        Infinity,\n        -Infinity,\n        1e-05,\n        1551563132437320.2,\n        5e-324\n    ],\n    \"float\": 0.10000000149011612,\n    \"pair\": [\n        1,\n        \"x\"\n    ],\n    \"empty\": []\n}".as_bytes());
            assert_eq!(result.bytes.as_slice(), expected);
        } else if matches!(
            case.name,
            "mono_bytes" | "mono_empty_bytes" | "mono_no_tree" | "mono_truncated" | "mono_trailing"
        ) {
            assert_eq!(result.extension, ".bin");
            assert_eq!(result.bytes.as_slice(), obj.raw_data().unwrap());
            assert_eq!(
                result.fallback,
                Some(if case.name == "mono_no_tree" {
                    Fallback::NoTypeTree
                } else if matches!(case.name, "mono_truncated" | "mono_trailing") {
                    Fallback::ReadFailed
                } else {
                    Fallback::JsonUnwritable
                })
            );
        } else {
            assert_eq!(result.extension, ".json", "{}", case.name);
            assert_eq!(result.fallback, None, "{}", case.name);
        }
    }
}

#[test]
fn invalid_utf8_field_names_are_written_as_raw_surrogateescaped_bytes() {
    use unityrs_testkit::{
        ngui::{V, st},
        serialized::{ObjectSpec, SerializedSpec, TypeSpec},
        writer::ByteWriter,
    };
    // UnityPyBoost aborts on this field name, so keep it out of cases::cases().
    let value = st(
        "MonoBehaviour",
        vec![("key", V::Str(b"surrogateescape key".to_vec()))],
    );
    let mut nodes = unityrs_testkit::ngui::nodes(0, "Base", &value).unwrap();
    nodes.iter_mut().find(|n| n.name == b"key").unwrap().name =
        b"key\xff\xed\xa0\x80\xe4\xb8".to_vec();
    let mut writer = ByteWriter::le();
    unityrs_testkit::ngui::write(&mut writer, &value).unwrap();
    let input = unityrs_testkit::serialized::build(&SerializedSpec {
        types: vec![TypeSpec {
            class_id: 114,
            nodes,
            ..Default::default()
        }],
        objects: vec![ObjectSpec {
            path_id: 1,
            data: writer.into_inner(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let (env, obj) = load(input);
    let result = simple::export(&obj, &env, "", &Meter::unlimited())
        .unwrap()
        .unwrap();
    assert_eq!(result.extension, ".json");
    assert_eq!(result.fallback, None);
    assert_eq!(
        result.bytes.as_slice(),
        b"{\n    \"key\xff\xed\xa0\x80\xe4\xb8\": \"surrogateescape key\"\n}"
    );
}

#[test]
fn video_reads_only_the_external_resource_range() {
    use std::sync::Arc;
    use unityrs_core::{
        budget::{Reservation, Unbounded},
        limits::ChargeError,
    };
    use unityrs_testkit::ngui::{V, st};
    // Independent reference: StreamedResource's source/offset/size layout
    // (PARITY.md section 15.7.1), applied to these known synthetic bytes.
    // This verifies a payload copy, with no UnityPy video-export claim.
    for (offset, size, expected) in [
        (3, 4, &b"\0\xffab"[..]),
        (8, 20, &b"tail"[..]),
        (50, 3, &b""[..]),
        (3, 0, &b""[..]),
    ] {
        let input = cases::build(
            329,
            st(
                "VideoClip",
                vec![(
                    "m_ExternalResources",
                    st(
                        "StreamedResource",
                        vec![
                            ("m_Source", V::Str(b"archive:/data/video.resource".to_vec())),
                            ("m_Offset", V::UInt64(offset)),
                            ("m_Size", V::UInt64(size)),
                        ],
                    ),
                )],
            ),
        );
        let (env, obj) = load(input);
        env.load(Source::Bytes {
            name: RawStr::from("video.resource"),
            data: Bytes::from_static(b"pre\0\xffab-tail"),
        })
        .unwrap();
        for (extension, want) in [(".mp4", ".mp4"), ("", ".video")] {
            let meter = Meter::unlimited();
            let result = simple::export(&obj, &env, extension, &meter)
                .unwrap()
                .unwrap();
            assert_eq!(result.extension, want);
            assert_eq!(result.bytes.as_slice(), expected);
            assert_eq!(result.fallback, None);
            assert!(meter.used() >= expected.len() as u64);
            assert_eq!(meter.decoded(), expected.len() as u64);
            drop(result);
            assert_eq!(meter.used(), 0);
        }
        if !expected.is_empty() {
            let short = expected.len() as u64 - 1;
            for meter in [
                Meter::new(short, Arc::new(Unbounded), Reservation::Input),
                Meter::with_decode_work(u64::MAX, short, Arc::new(Unbounded), Reservation::Input),
            ] {
                let error = simple::export(&obj, &env, ".mp4", &meter).unwrap_err();
                assert!(matches!(
                    error,
                    simple::Error::Charge(ChargeError::Limit(_))
                ));
                drop(error);
                assert_eq!(meter.used(), 0);
            }
        }
    }
    let (env, obj) = load(cases::build(
        329,
        st(
            "VideoClip",
            vec![(
                "m_ExternalResources",
                st(
                    "StreamedResource",
                    vec![
                        ("m_Source", V::Str(b"missing.resource".to_vec())),
                        ("m_Offset", V::UInt64(0)),
                        ("m_Size", V::UInt64(3)),
                    ],
                ),
            )],
        ),
    ));
    assert!(simple::export(&obj, &env, "", &Meter::unlimited()).is_err());
}

#[test]
fn malformed_mono_tree_falls_back_but_unsupported_classes_fail() {
    use unityrs_testkit::{
        ngui::{Object, V, st},
        scene3d::{Layout, file},
    };
    let mut object = Object::new(1, 114, st("MonoBehaviour", vec![("n", V::Int(7))]));
    object.cut = Some(2);
    let (env, obj) = load(file(Layout::Unity2022, &[], &[object]).unwrap());
    let result = simple::export(&obj, &env, "", &Meter::unlimited())
        .unwrap()
        .unwrap();
    assert_eq!(result.fallback, Some(Fallback::ReadFailed));
    assert_eq!(result.bytes.as_slice(), &[7, 0]);
    let (env, obj) = load(cases::build(1, st("GameObject", vec![])));
    assert!(matches!(
        simple::export(&obj, &env, "", &Meter::unlimited()),
        Err(simple::Error::UnsupportedClass(1))
    ));
}

#[derive(Debug)]
struct Refuse {
    at: u64,
    calls: std::sync::atomic::AtomicU64,
}
impl unityrs_core::budget::MemoryBudget for Refuse {
    fn try_reserve(
        &self,
        bytes: u64,
        what: unityrs_core::budget::Reservation,
    ) -> Result<unityrs_core::budget::Permit, unityrs_core::budget::BudgetExceeded> {
        use std::sync::atomic::Ordering;
        // Refuse retries too: MeteredVec may retry a smaller growth.
        if self.calls.fetch_add(1, Ordering::Relaxed) >= self.at {
            Err(unityrs_core::budget::BudgetExceeded {
                requested: bytes,
                what,
            })
        } else {
            unityrs_core::budget::MemoryBudget::try_reserve(
                &unityrs_core::budget::Unbounded,
                bytes,
                what,
            )
        }
    }
}
#[test]
fn every_output_reservation_is_fallible_and_returns_its_charge() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use unityrs_core::budget::Reservation;
    for case in cases::cases() {
        let (env, obj) = load(case.input);
        assert_eq!(obj.class_id(), case.class_id);
        let budget = Arc::new(Refuse {
            at: u64::MAX,
            calls: AtomicU64::new(0),
        });
        let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let output = simple::export(&obj, &env, case.extension, &meter).unwrap();
        if let Some(o) = &output {
            assert!(meter.used() >= o.bytes.len() as u64);
        }
        drop(output);
        assert_eq!(meter.used(), 0);
        let calls = budget.calls.load(Ordering::Relaxed);
        for at in 0..calls {
            let meter = Meter::new(
                u64::MAX,
                Arc::new(Refuse {
                    at,
                    calls: AtomicU64::new(0),
                }),
                Reservation::Input,
            );
            let result = simple::export(&obj, &env, case.extension, &meter);
            assert!(
                result.is_err(),
                "{} reservation {at}: {result:?}",
                case.name
            );
            drop(result);
            assert_eq!(meter.used(), 0);
        }
    }
}

#[test]
fn seeded_mutations_never_panic_or_outgrow_the_budget() {
    use std::sync::Arc;
    use unityrs_core::FileContext;
    let inputs = cases::cases();
    let mut seed = 0x00e1_4912_8114_u64;
    for i in 0..10_000 {
        let mut data = inputs[i % inputs.len()].input.clone();
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let at = usize::try_from(seed % u64::try_from(data.len()).unwrap()).unwrap();
        data[at] ^= seed.to_le_bytes()[4];
        if i % 3 == 0 {
            data.truncate(at);
        }
        let mut context = FileContext::unitypy();
        context.limits.max_input_memory = 256 * 1024;
        context.limits.max_decode_work = 1024 * 1024;
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(context);
        let env = Environment::new(options);
        if let Ok(File::Serialized(file)) = env.load(Source::Bytes {
            name: RawStr::from("mutation.assets"),
            data: Bytes::from(data),
        }) && let Some(obj) = file.object(1)
        {
            let meter = Meter::new(
                256 * 1024,
                Arc::new(unityrs_core::budget::Unbounded),
                unityrs_core::budget::Reservation::Input,
            );
            let _ = simple::export(&obj, &env, "", &meter);
            assert_eq!(meter.used(), 0);
        }
    }
}

#[test]
fn fonts_convert_integer_lists_to_bytes_and_reject_out_of_range_values() {
    use unityrs_testkit::ngui::{V, st, vector};
    for (items, expected) in [
        (vec![79, 84, 84, 79, 255], Some(&b"OTTO\xff"[..])),
        (vec![-1], None),
        (vec![256], None),
    ] {
        let (env, obj) = load(cases::build(
            128,
            st(
                "Font",
                vec![(
                    "m_FontData",
                    vector(V::Int(0), items.into_iter().map(V::Int).collect()),
                )],
            ),
        ));
        let result = simple::export(&obj, &env, "", &Meter::unlimited());
        if let Some(expected) = expected {
            assert_eq!(result.unwrap().unwrap().bytes.as_slice(), expected);
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn json_counting_bounds_work_even_before_a_late_unwritable_value() {
    use std::sync::Arc;
    use unityrs_core::{
        LimitKind,
        budget::{Reservation, Unbounded},
        limits::ChargeError,
    };
    use unityrs_testkit::ngui::{V, st, vector};
    let mut nested = st("Empty", vec![]);
    for _ in 0..40 {
        nested = st("Nested", vec![("child", nested)]);
    }
    for value in [
        vector(V::Double(0.), vec![V::Double(0.1); 128]),
        V::Str(vec![b'a'; 16 * 1024]),
        nested,
    ] {
        for late_bytes in [false, true] {
            let mut fields = vec![("value", value.clone())];
            if late_bytes {
                fields.push(("bytes", V::Bytes(vec![0])));
            }
            let (env, obj) = load(cases::build(114, st("MonoBehaviour", fields)));
            let meter =
                Meter::with_decode_work(u64::MAX, 1024, Arc::new(Unbounded), Reservation::Input);
            let result = simple::export(&obj, &env, "", &meter);
            assert!(
                matches!(result,
                    Err(simple::Error::Json(unityrs_export::pyjson::WriteError::Charge(ChargeError::Limit(ref e))))
                    if e.kind() == LimitKind::DecodeWork
                ),
                "{result:?}"
            );
            assert!(
                (1000..=1024).contains(&meter.decoded()),
                "counting must stop near the limit: {}",
                meter.decoded()
            );
            assert_eq!(meter.used(), 0);
        }
    }
}

#[test]
fn failed_json_traversals_keep_their_work_charge() {
    use unityrs_testkit::ngui::{V, st};
    let (env, obj) = load(cases::build(
        114,
        st(
            "MonoBehaviour",
            vec![
                ("text", V::Str(vec![b'a'; 256])),
                ("bytes", V::Bytes(vec![0])),
            ],
        ),
    ));
    let meter = Meter::unlimited();
    let result = simple::export(&obj, &env, "", &meter).unwrap().unwrap();
    assert_eq!(result.fallback, Some(Fallback::JsonUnwritable));
    assert!(meter.decoded() >= u64::from(obj.info().byte_size) + 256);
}
