use unityrs_anim::{AnimError, BindingKind, SlotCounts, TransformProperty, read_bindings};
use unityrs_core::env::{EnvOptions, Environment, PPtr, Source};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, ObjectRef, RawStr};
use unityrs_testkit::ngui::{Object, V, vector};
use unityrs_testkit::scene3d::{self as s, Layout, anim as a};

fn clip(layout: Layout, records: &[a::Binding]) -> ObjectRef {
    load(layout, a::bound_clip(1, layout, records, &[(1, -9)]))
}

fn load(layout: Layout, object: Object) -> ObjectRef {
    let env = Environment::new(EnvOptions::unitypy());
    let data = s::file(layout, &[], &[object]).unwrap();
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"clip"),
            data: Bytes::from(data),
        })
        .unwrap()
    else {
        panic!("serialized fixture");
    };
    file.object(1).unwrap()
}

#[test]
fn reads_class_and_flag_layouts_with_multiple_records() {
    for (layout, width, int_flag, managed_flag) in [
        (Layout::Unity55, 2, false, false),
        (Layout::Unity2019, 4, false, false),
        (Layout::Unity20221, 4, true, false),
        (Layout::Unity2022, 4, true, true),
        (Layout::Unity6000, 4, true, true),
    ] {
        let object = clip(
            layout,
            &[
                a::Binding {
                    path: 7,
                    class: 4,
                    attribute: 1,
                    ..Default::default()
                },
                a::Binding {
                    path: 13,
                    class: if width == 2 { 65_535 } else { -7 },
                    attribute: 9,
                    script: (1, -8),
                    pptr: 1,
                    integer: 0,
                    managed: u8::from(managed_flag),
                    ..Default::default()
                },
            ],
        );
        let meter = Meter::unlimited();
        let table = read_bindings(&object, &meter).unwrap();
        assert_eq!(table.layout.class_bytes, width);
        assert_eq!(table.layout.aligns_record, width == 4);
        assert_eq!(table.layout.has_int_flag, int_flag);
        assert_eq!(table.layout.has_managed_reference_flag, managed_flag);
        assert_eq!(table.bindings()[1].path, 13);
        assert_eq!(
            table.bindings()[1].class_id,
            if width == 2 { 65_535 } else { -7 }
        );
        assert_eq!(
            table.bindings()[1].script,
            PPtr {
                file_id: 1,
                path_id: -8
            }
        );
        assert!(table.bindings()[1].flags.is_pptr);
        assert_eq!(table.bindings()[1].flags.is_managed_reference, managed_flag);
        assert_eq!(
            table.pptr_mapping(),
            &[PPtr {
                file_id: 1,
                path_id: -9
            }]
        );
        assert!(meter.used() > 0);
        drop(table);
        assert_eq!(meter.used(), 0);
    }
}

fn mixed_records() -> Vec<a::Binding> {
    vec![
        a::Binding {
            class: 4,
            attribute: 1,
            ..Default::default()
        },
        a::Binding {
            class: 137,
            custom: 22,
            ..Default::default()
        },
        a::Binding {
            class: 114,
            custom: 24,
            integer: 1,
            ..Default::default()
        },
        a::Binding {
            class: 1_818_360_608,
            custom: 31,
            pptr: 1,
            ..Default::default()
        },
        a::Binding {
            class: 4,
            attribute: 2,
            ..Default::default()
        },
        a::Binding {
            class: 4,
            attribute: 3,
            ..Default::default()
        },
        a::Binding {
            class: 4,
            attribute: 4,
            ..Default::default()
        },
        a::Binding {
            class: 137,
            custom: 20,
            ..Default::default()
        },
    ]
}

#[test]
fn empty_mapping_still_requires_a_pointer_element() {
    let layout = Layout::Unity2022;
    let mut object = a::bound_clip(1, layout, &[], &[]);
    let V::Struct(_, root) = &mut object.value else {
        panic!("clip fixture")
    };
    let V::Struct(_, fields) = &mut root[0].1 else {
        panic!("binding fixture")
    };
    fields[1].1 = vector(V::Int(0), vec![]);
    let meter = Meter::unlimited();
    assert!(matches!(
        read_bindings(&load(layout, object), &meter),
        Err(AnimError::UnsupportedBindingLayout)
    ));
    assert_eq!(meter.used(), 0);
}

#[test]
fn slot_widths_keep_metadata_in_place_and_refuse_unknowns_and_count_errors() {
    let mut source = mixed_records();
    let meter = Meter::unlimited();
    let table = read_bindings(&clip(Layout::Unity2022, &source), &meter).unwrap();
    let slots = table
        .slots(
            SlotCounts {
                streamed: 4,
                dense: 2,
                constant: 11,
            },
            &meter,
        )
        .unwrap();
    assert_eq!(
        slots
            .as_slice()
            .iter()
            .map(|x| (x.first, x.width))
            .collect::<Vec<_>>(),
        [
            (0, 3),
            (3, 1),
            (4, 1),
            (5, 1),
            (6, 4),
            (10, 3),
            (13, 3),
            (16, 1)
        ]
    );
    assert_eq!(slots.as_slice()[1].kind, BindingKind::Material);
    assert_eq!(slots.as_slice()[2].kind, BindingKind::Integer);
    assert_eq!(slots.as_slice()[3].kind, BindingKind::PPtr);
    assert_eq!(
        slots.as_slice()[4].kind,
        BindingKind::Transform(TransformProperty::Quaternion)
    );
    assert_eq!(slots.as_slice()[7].kind, BindingKind::Morph);
    assert!(matches!(
        table.slots(SlotCounts::default(), &meter),
        Err(AnimError::SlotCount {
            bindings: 17,
            storage: 0
        })
    ));
    assert!(matches!(
        table.slots(
            SlotCounts {
                streamed: usize::MAX,
                dense: 1,
                constant: 0
            },
            &meter
        ),
        Err(AnimError::SlotOverflow)
    ));
    source[1].custom = 255;
    let table = read_bindings(&clip(Layout::Unity2022, &source), &meter).unwrap();
    assert!(matches!(
        table.slots(SlotCounts::default(), &meter),
        Err(AnimError::UnknownBindingWidth { binding: 1, .. })
    ));
    source[1].custom = 22;
    source[2].pptr = 1;
    assert!(matches!(
        read_bindings(&clip(Layout::Unity2022, &source), &meter),
        Err(AnimError::ContradictoryFlags(2))
    ));
}
