//! Explicit and compiled `AnimationClip` layouts with embedded typetrees.
use super::{Binding, Ptr, bound_clip, offset};
use crate::ngui::{Object, V, pptr, s, st, vector};
use crate::scene3d::{Layout, set};

#[derive(Clone, Copy, Debug)]
pub struct Key<const N: usize> {
    pub time: f32,
    pub value: [f32; N],
    pub incoming: [f32; N],
    pub outgoing: [f32; N],
    pub weighted: i32,
    pub in_weight: [f32; N],
    pub out_weight: [f32; N],
}

impl<const N: usize> Key<N> {
    #[must_use]
    pub fn new(time: f32, value: [f32; N]) -> Self {
        Self {
            time,
            value,
            incoming: [0.; N],
            outgoing: [0.; N],
            weighted: 0,
            in_weight: [1. / 3.; N],
            out_weight: [1. / 3.; N],
        }
    }
}

fn components<const N: usize>(row: [f32; N]) -> V {
    if N == 1 {
        return V::Float(row[0]);
    }
    let name = if N == 4 { "Quaternionf" } else { "Vector3f" };
    st(
        name,
        ["x", "y", "z", "w"]
            .into_iter()
            .zip(row.map(V::Float))
            .collect(),
    )
}

/// A runtime or authoring curve row. Pass `None` for the pre-5.3 order
/// layout and `false` for the pre-2018 key layout.
#[must_use]
pub fn curve<const N: usize>(
    path: &str,
    keys: &[Key<N>],
    wrap: [i32; 2],
    order: Option<i32>,
    weighted_layout: bool,
    attribute: Option<(&str, i32)>,
) -> V {
    let key = |k: Key<N>| {
        let mut fields = vec![
            ("time", V::Float(k.time)),
            ("value", components(k.value)),
            ("inSlope", components(k.incoming)),
            ("outSlope", components(k.outgoing)),
        ];
        if weighted_layout {
            fields.extend([
                ("weightedMode", V::Int(k.weighted)),
                ("inWeight", components(k.in_weight)),
                ("outWeight", components(k.out_weight)),
            ]);
        }
        st("Keyframe", fields)
    };
    let mut data = vec![
        (
            "m_Curve",
            vector(
                key(Key::new(0., [0.; N])),
                keys.iter().copied().map(key).collect(),
            ),
        ),
        ("m_PreInfinity", V::Int(wrap[0])),
        ("m_PostInfinity", V::Int(wrap[1])),
    ];
    if let Some(order) = order {
        data.push(("m_RotationOrder", V::Int(order)));
    }
    let mut row = vec![("curve", st("AnimationCurve", data))];
    if let Some((attribute, _)) = attribute {
        row.push(("attribute", s(attribute)));
    }
    row.push(("path", s(path)));
    if let Some((_, class)) = attribute {
        row.extend([
            ("classID", V::Int(class)),
            ("script", pptr("MonoScript", (0, 0))),
        ]);
    }
    st(
        match N {
            1 => "FloatCurve",
            4 => "QuaternionCurve",
            _ => "Vector3Curve",
        },
        row,
    )
}

#[must_use]
pub fn explicit(id: i64, rate: f32, fields: &[(&str, Vec<V>)]) -> Object {
    let mut value = st(
        "AnimationClip",
        vec![
            ("m_Name", s("explicit fixture")),
            ("m_Legacy", V::Bool(true)),
            ("m_Compressed", V::Bool(false)),
            ("m_SampleRate", V::Float(rate)),
            ("m_WrapMode", V::Int(2)),
        ],
    );
    for (name, rows) in fields {
        if let Some(shape) = rows.first() {
            set(&mut value, name, vector(shape.clone(), rows.clone()));
        }
    }
    Object::new(id, 74, value)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Dense<'a> {
    pub frames: i32,
    pub curves: u32,
    pub rate: f32,
    pub begin: f32,
    pub bits: &'a [u32],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Storage<'a> {
    pub interval: [f32; 2],
    pub streamed_count: u32,
    pub words: &'a [u32],
    pub dense: Dense<'a>,
    pub constants: &'a [u32],
    /// The 2022.3 split count of discrete streamed slots, omitted when `None`.
    pub discrete_count: Option<u32>,
}

fn uints(items: &[u32]) -> V {
    vector(V::UInt(0), items.iter().copied().map(V::UInt).collect())
}

fn floats(items: &[u32]) -> V {
    vector(
        V::Float(0.),
        items
            .iter()
            .copied()
            .map(|bits| V::Float(f32::from_bits(bits)))
            .collect(),
    )
}

/// Append a streamed frame to the serialized words.
///
/// # Panics
/// Panics if there are more than `u32::MAX` keys in this fixture frame.
pub fn frame(words: &mut Vec<u32>, time: f32, keys: &[(u32, [u32; 4])]) {
    words.extend([time.to_bits(), u32::try_from(keys.len()).unwrap()]);
    for (slot, coefficients) in keys {
        words.push(*slot);
        words.extend(coefficients);
    }
}

pub fn add_compiled(
    object: &mut Object,
    layout: Layout,
    bindings: &[Binding],
    mapping: &[Ptr],
    storage: Storage<'_>,
) {
    let V::Struct(_, table) = bound_clip(object.path_id, layout, bindings, mapping).value else {
        unreachable!()
    };
    for (name, value) in table {
        set(&mut object.value, &name, value);
    }
    let dense = storage.dense;
    let data = st(
        "Clip",
        vec![
            (
                "m_StreamedClip",
                st(
                    "StreamedClip",
                    [
                        ("data", uints(storage.words)),
                        ("curveCount", V::UInt(storage.streamed_count)),
                    ]
                    .into_iter()
                    .chain(
                        storage
                            .discrete_count
                            .map(|count| ("discreteCurveCount", V::UInt(count))),
                    )
                    .collect(),
                ),
            ),
            (
                "m_DenseClip",
                st(
                    "DenseClip",
                    vec![
                        ("m_FrameCount", V::Int(dense.frames)),
                        ("m_CurveCount", V::UInt(dense.curves)),
                        ("m_SampleRate", V::Float(dense.rate)),
                        ("m_BeginTime", V::Float(dense.begin)),
                        ("m_SampleArray", floats(dense.bits)),
                    ],
                ),
            ),
            (
                "m_ConstantClip",
                st("ConstantClip", vec![("data", floats(storage.constants))]),
            ),
        ],
    );
    set(&mut object.value, "m_Legacy", V::Bool(false));
    set(
        &mut object.value,
        "m_MuscleClip",
        st(
            "ClipMuscleConstant",
            vec![
                ("m_Clip", offset(data)),
                ("m_StartTime", V::Float(storage.interval[0])),
                ("m_StopTime", V::Float(storage.interval[1])),
                ("m_IndexArray", vector(V::Int(-1), vec![V::Int(-1); 7])),
                ("m_LoopTime", V::Bool(true)),
            ],
        ),
    );
}

#[must_use]
pub fn explicit_fixture() -> Object {
    let mut left = Key::new(2., [2., -2., 3.]);
    left.outgoing = [6., 0., f32::INFINITY];
    let mut right = Key::new(6., [10., 8., 4.]);
    right.incoming = [-2., 0., 0.];
    explicit(
        9,
        2.,
        &[
            (
                "m_PositionCurves",
                vec![curve("", &[left, right], [2; 2], Some(4), true, None)],
            ),
            (
                "m_EditorCurves",
                vec![curve(
                    "",
                    &[Key::new(100., [-99.])],
                    [2; 2],
                    Some(4),
                    true,
                    Some(("m_LocalPosition.x", 4)),
                )],
            ),
        ],
    )
}

#[must_use]
pub fn compiled_fixture() -> Object {
    let mut object = explicit_fixture();
    let mut words = Vec::new();
    frame(&mut words, f32::MIN, &[]);
    frame(
        &mut words,
        3.,
        &[
            (0, [2_f32, -1., 3., 4.].map(f32::to_bits)),
            (1, [0_f32, 0., 0., 5.].map(f32::to_bits)),
            (2, [0_f32, 0., 0., 6.].map(f32::to_bits)),
        ],
    );
    frame(
        &mut words,
        5.,
        &[(0, [9_f32, 8., 7., -2.].map(f32::to_bits))],
    );
    frame(&mut words, f32::INFINITY, &[]);
    let bindings = [1, 3].map(|attribute| Binding {
        class: 4,
        attribute,
        ..Binding::default()
    });
    add_compiled(
        &mut object,
        Layout::Unity2022,
        &bindings,
        &[],
        Storage {
            interval: [2., 6.],
            streamed_count: 3,
            words: &words,
            constants: &[2_f32, 3., 4.].map(f32::to_bits),
            ..Storage::default()
        },
    );
    object
}

/// Off-key Euler and weighted morph fixture for the f64 transcription.
#[must_use]
pub fn explicit_replay() -> Object {
    let mut object = explicit_fixture();
    let euler = curve(
        "",
        &[
            Key::new(2., [10., 20., 30.]),
            Key::new(6., [370., -80., 60.]),
        ],
        [2; 2],
        Some(5),
        true,
        None,
    );
    set(&mut object.value, "m_EulerCurves", one_row(euler));
    let mut left = Key::new(2., [4.]);
    left.outgoing = [20.];
    left.out_weight = [0.25];
    left.weighted = 2;
    let mut right = Key::new(6., [60.]);
    right.incoming = [-8.];
    let weights = curve(
        "",
        &[left, right],
        [2; 2],
        Some(4),
        true,
        Some(("blendShape.Lift", 137)),
    );
    set(&mut object.value, "m_FloatCurves", one_row(weights));
    let stale_euler = curve(
        "",
        &[Key::new(100., [0., 0., 0.])],
        [2; 2],
        Some(0),
        true,
        None,
    );
    set(
        &mut object.value,
        "m_EulerEditorCurves",
        one_row(stale_euler),
    );
    let event = st(
        "AnimationEvent",
        vec![
            ("time", V::Float(3.)),
            ("functionName", s("fixture_marker")),
            ("data", s("inert")),
            ("objectReferenceParameter", pptr("Object", (0, -10))),
            ("floatParameter", V::Float(6.)),
            ("intParameter", V::Int(-9)),
            ("messageOptions", V::Int(0)),
        ],
    );
    set(&mut object.value, "m_Events", one_row(event));
    let key = |time, value| {
        st(
            "PPtrKeyframe",
            vec![("time", V::Float(time)), ("value", pptr("Object", value))],
        )
    };
    let pointer = st(
        "PPtrCurve",
        vec![
            (
                "curve",
                vector(key(0., (0, 0)), vec![key(2., (0, -7)), key(6., (1, -8))]),
            ),
            ("attribute", s("texture")),
            ("path", s("")),
            ("classID", V::Int(137)),
            ("script", pptr("MonoScript", (0, 0))),
        ],
    );
    set(&mut object.value, "m_PPtrCurves", one_row(pointer));
    object
}

#[must_use]
pub fn one_row(row: V) -> V {
    vector(row.clone(), vec![row])
}

/// Compiled XYZW and morph values on a dense grid inside the clip interval.
#[must_use]
pub fn dense_replay() -> Object {
    let mut object = explicit(10, 4., &[]);
    add_compiled(
        &mut object,
        Layout::Unity2022,
        &[
            Binding {
                class: 4,
                attribute: 2,
                ..Binding::default()
            },
            Binding {
                class: 137,
                custom: 20,
                attribute: 3_545_215_282,
                ..Binding::default()
            },
        ],
        &[],
        Storage {
            interval: [2., 6.],
            dense: Dense {
                frames: 3,
                curves: 5,
                rate: 4.,
                begin: 3.,
                bits: &[
                    0_f32, 0., 0., 1., 4., 0., 0., 1., 1., 20., 0., 0., 1., 0., 84.,
                ]
                .map(f32::to_bits),
            },
            ..Storage::default()
        },
    );
    object
}

/// Serialized packed vectors for two rotations without an established
/// slope interpretation. Words use omitted W, then X:9, Y:10 and Z:10.
#[must_use]
pub fn packed_row(slopes: bool) -> V {
    let packed = 3u32 | (256 << 3) | (512 << 12) | (512 << 22);
    let values = [packed.to_le_bytes(), (packed | 4).to_le_bytes()].concat();
    let mut fields = vec![
        ("m_Path", s("")),
        (
            "m_Times",
            st(
                "PackedIntVector",
                vec![
                    ("m_NumItems", V::UInt(2)),
                    ("m_Data", V::Bytes(vec![50, 125])),
                    ("m_BitSize", V::AlignedUInt8(8)),
                ],
            ),
        ),
        (
            "m_Values",
            st(
                "PackedQuatVector",
                vec![("m_NumItems", V::UInt(2)), ("m_Data", V::Bytes(values))],
            ),
        ),
    ];
    if slopes {
        fields.push((
            "m_Slopes",
            st(
                "PackedFloatVector",
                vec![
                    ("m_NumItems", V::UInt(3)),
                    ("m_Range", V::Float(12.)),
                    ("m_Start", V::Float(-4.)),
                    ("m_Data", V::Bytes(vec![0b00_11_01_00])),
                    ("m_BitSize", V::AlignedUInt8(2)),
                ],
            ),
        ));
    }
    fields.extend([("m_PreInfinity", V::Int(2)), ("m_PostInfinity", V::Int(2))]);
    st("CompressedAnimationCurve", fields)
}

/// A release-tree 2021.3/2022.3 legacy clip with one position key, for the
/// stripped-provider gate. The empty muscle record occupies its serialized
/// fields even though the clip is legacy.
#[must_use]
pub fn stripped_position_payload(big_endian: bool) -> Vec<u8> {
    use crate::ByteWriter;
    let mut out = if big_endian {
        ByteWriter::be()
    } else {
        ByteWriter::le()
    };
    out.i32(0) // name
        .u8(1)
        .u8(0)
        .u8(0)
        .align(4) // legacy, compressed, high-quality
        .i32(0)
        .i32(0)
        .i32(0) // quaternion, compressed quaternion, Euler arrays
        .i32(1)
        .i32(1); // one position curve, one key
    for value in [2_f32, 4., 5., 6.] {
        out.u32(value.to_bits());
    }
    out.bytes(&[0; 24]).i32(0).bytes(&[0; 24]); // slopes, weighted mode, weights
    out.i32(2)
        .i32(2)
        .i32(4)
        .i32(0) // per-curve pre/post infinity, order and empty path
        .i32(0)
        .i32(0)
        .i32(0) // scale, float, PPtr arrays
        .u32(24_f32.to_bits())
        .i32(0) // clip sample rate and wrap
        .bytes(&[0; 28]) // bounds and muscle size
        .bytes(&[0; 452]) // muscle record with empty arrays
        .i32(0)
        .i32(0) // generic bindings and PPtr remapping arrays
        .u8(0)
        .u8(0)
        .align(4)
        .i32(0); // motion flags and events
    out.into_inner()
}
