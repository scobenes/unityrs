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
                    pptr: u8::from(!int_flag),
                    integer: u8::from(int_flag && !managed_flag),
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
        assert_eq!(table.bindings()[1].flags.is_pptr, !int_flag);
        assert_eq!(table.bindings()[1].flags.is_int, int_flag && !managed_flag);
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
fn slot_widths_keep_metadata_in_place_and_refuse_count_errors() {
    let source = mixed_records();
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
}

#[test]
fn ordinary_scalar_curves_and_rect_transform_trs_preserve_later_slots() {
    use BindingKind::{Transform, UnsupportedScalar};
    use TransformProperty::{Euler, Quaternion, Scale, Translation};

    // Class/attribute pairs follow AssetStudio FindBinding and AssetRipper IsTransform.
    let rows = [
        (212, 23, UnsupportedScalar, 0, 1), // SpriteRenderer color
        (108, 25, UnsupportedScalar, 1, 1), // Light
        (224, 28, UnsupportedScalar, 2, 1), // RectTransform anchor
        (225, 0, UnsupportedScalar, 3, 1),  // CanvasGroup
        (224, 1, Transform(Translation), 4, 3),
        (224, 2, Transform(Quaternion), 7, 4),
        (224, 3, Transform(Scale), 11, 3),
        (224, 4, Transform(Euler), 14, 3),
        (4, 1, Transform(Translation), 17, 3),
    ];
    let source = rows.map(|(class, attribute, ..)| a::Binding {
        class,
        attribute,
        ..Default::default()
    });
    let meter = Meter::unlimited();
    let table = read_bindings(&clip(Layout::Unity2022, &source), &meter).unwrap();
    let slots = table
        .slots(
            SlotCounts {
                constant: 20,
                ..Default::default()
            },
            &meter,
        )
        .unwrap();
    assert_eq!(
        slots
            .as_slice()
            .iter()
            .map(|slot| (slot.kind, slot.first, slot.width))
            .collect::<Vec<_>>(),
        rows.map(|(_, _, kind, first, width)| (kind, first, width))
    );
}

#[test]
fn unknown_transform_attribute_refuses_slot_assignment() {
    let meter = Meter::unlimited();
    let source = [a::Binding {
        class: 4,
        attribute: 255,
        ..Default::default()
    }];
    let table = read_bindings(&clip(Layout::Unity2022, &source), &meter).unwrap();
    assert!(matches!(
        table.slots(SlotCounts::default(), &meter),
        Err(AnimError::UnknownBindingWidth {
            binding: 0,
            class_id: 4,
            attribute: 255,
            ..
        })
    ));
}

#[test]
fn unknown_custom_transform_refuses_before_assigning_trs_slots() {
    let meter = Meter::unlimited();
    let ordinary = a::Binding {
        class: 4,
        attribute: 1,
        ..Default::default()
    };
    let source = [
        a::Binding {
            custom: 250,
            ..ordinary
        },
        ordinary,
    ];
    let table = read_bindings(&clip(Layout::Unity2022, &source), &meter).unwrap();
    let counts = SlotCounts {
        constant: 6,
        ..Default::default()
    };
    assert!(matches!(
        table.slots(counts, &meter),
        Err(AnimError::UnknownBindingWidth {
            binding: 0,
            class_id: 4,
            custom_type: 250,
            attribute: 1,
        })
    ));
}

#[test]
fn conflicting_value_flags_are_refused_in_every_pair() {
    let meter = Meter::unlimited();
    for (pptr, integer, managed) in [(1, 1, 0), (1, 0, 1), (0, 1, 1)] {
        let source = [a::Binding {
            class: 114,
            pptr,
            integer,
            managed,
            ..Default::default()
        }];
        assert!(
            matches!(
                read_bindings(&clip(Layout::Unity2022, &source), &meter),
                Err(AnimError::ContradictoryFlags(0))
            ),
            "flags {pptr}/{integer}/{managed}"
        );
    }
}

#[test]
fn pre_56_binding_records_accept_an_aligned_final_flag() {
    let layout = Layout::Unity55;
    let rows = [
        a::Binding {
            class: 4,
            attribute: 1,
            ..Default::default()
        },
        a::Binding {
            class: 114,
            path: 42,
            pptr: 1,
            ..Default::default()
        },
    ];
    let mut object = a::bound_clip(1, layout, &rows, &[(1, -9)]);
    let V::Struct(_, root) = &mut object.value else {
        panic!("clip fixture")
    };
    let V::Struct(_, fields) = &mut root[0].1 else {
        panic!("binding fixture")
    };
    let V::Vector(_, element, records) = &mut fields[0].1 else {
        panic!("record array")
    };
    for record in std::iter::once(element.as_mut()).chain(records) {
        let V::Struct(_, fields) = record else {
            panic!("binding record")
        };
        let (_, flag) = fields
            .iter_mut()
            .find(|(name, _)| name == "isPPtrCurve")
            .unwrap();
        let V::UInt8(value) = *flag else {
            panic!("PPtr flag")
        };
        *flag = V::AlignedUInt8(value);
    }
    let table = read_bindings(&load(layout, object), &Meter::unlimited()).unwrap();
    assert!(table.layout.aligns_record);
    assert_eq!(table.bindings()[1].path, 42);
    assert!(table.bindings()[1].flags.is_pptr);
    assert_eq!(
        table.pptr_mapping(),
        &[PPtr {
            file_id: 1,
            path_id: -9
        }]
    );
}
