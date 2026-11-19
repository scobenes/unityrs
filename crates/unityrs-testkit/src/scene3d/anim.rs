//! Minimal embedded trees for animation discovery and binding tests.
use super::pointers;
use crate::ngui::{Object, V, pair, pptr, s, st, vector};

type Ptr = (i32, i64);

#[must_use]
pub fn clip(id: i64) -> Object {
    Object::new(
        id,
        74,
        st("AnimationClip", vec![("m_Name", s("same clip"))]),
    )
}

#[must_use]
pub fn legacy(id: i64, default: Ptr, clips: &[Ptr]) -> Object {
    Object::new(
        id,
        111,
        st(
            "Animation",
            vec![
                ("m_Animation", pptr("AnimationClip", default)),
                ("m_Animations", pointers("AnimationClip", clips)),
            ],
        ),
    )
}

#[must_use]
pub fn animator(id: i64, controller: Ptr) -> Object {
    Object::new(
        id,
        95,
        st(
            "Animator",
            vec![(
                "m_Controller",
                pptr("RuntimeAnimatorController", controller),
            )],
        ),
    )
}

#[must_use]
pub fn overrides(id: i64, controller: Ptr, rows: &[(Ptr, Ptr)]) -> Object {
    let row = |original, replacement| {
        st(
            "AnimationClipOverride",
            vec![
                ("m_OriginalClip", pptr("AnimationClip", original)),
                ("m_OverrideClip", pptr("AnimationClip", replacement)),
            ],
        )
    };
    Object::new(
        id,
        221,
        st(
            "AnimatorOverrideController",
            vec![
                (
                    "m_Controller",
                    pptr("RuntimeAnimatorController", controller),
                ),
                (
                    "m_Clips",
                    vector(
                        row((0, 0), (0, 0)),
                        rows.iter().map(|&(a, b)| row(a, b)).collect(),
                    ),
                ),
            ],
        ),
    )
}

/// Unity 5.2+ controller table with one machine, state and tree.
#[must_use]
pub fn controller(id: i64, clips: &[Ptr], leaves: &[u32]) -> Object {
    controller_with_leaf_info(id, clips, leaves, None)
}

/// A pre-5.2 controller whose leaves refer to IDs in a state-local table.
#[must_use]
pub fn controller_with_leaf_info(
    id: i64,
    clips: &[Ptr],
    leaves: &[u32],
    info: Option<(&[u32], u32)>,
) -> Object {
    let node = |clip| {
        offset(st(
            "BlendTreeNodeConstant",
            vec![
                ("m_ChildIndices", vector(V::UInt(0), vec![])),
                ("m_ClipID", V::UInt(clip)),
            ],
        ))
    };
    let tree = st(
        "BlendTreeConstant",
        vec![(
            "m_NodeArray",
            vector(node(0), leaves.iter().copied().map(node).collect()),
        )],
    );
    let mut fields = vec![("m_BlendTreeConstantArray", one(offset(tree)))];
    if let Some((ids, base)) = info {
        fields.push((
            "m_LeafInfoArray",
            one(st(
                "LeafInfoConstant",
                vec![
                    (
                        "m_IDArray",
                        vector(V::UInt(0), ids.iter().copied().map(V::UInt).collect()),
                    ),
                    ("m_IndexOffset", V::UInt(base)),
                ],
            )),
        ));
    }
    let state = offset(st("StateConstant", fields));
    let machine = offset(st(
        "StateMachineConstant",
        vec![("m_StateConstantArray", one(state))],
    ));
    let constant = st(
        "ControllerConstant",
        vec![("m_StateMachineArray", one(machine))],
    );
    Object::new(
        id,
        91,
        st(
            "AnimatorController",
            vec![
                ("m_Controller", constant),
                (
                    "m_TOS",
                    vector(pair(V::UInt(0), s("")), vec![pair(V::UInt(0), s(""))]),
                ),
                ("m_AnimationClips", pointers("AnimationClip", clips)),
            ],
        ),
    )
}

fn offset(value: V) -> V {
    st("OffsetPtr", vec![("data", value)])
}

fn one(value: V) -> V {
    vector(value.clone(), vec![value])
}

/// Stored values for one `GenericBinding` test row.
#[derive(Clone, Copy, Debug, Default)]
pub struct Binding {
    pub path: u32,
    pub attribute: u32,
    pub script: Ptr,
    pub class: i32,
    pub custom: u8,
    pub pptr: u8,
    pub integer: u8,
    pub managed: u8,
}

/// A clip with only its embedded binding constant. Consecutive records
/// exercise alignment as well as field reads.
#[must_use]
pub fn bound_clip(id: i64, layout: super::Layout, bindings: &[Binding], mapping: &[Ptr]) -> Object {
    use super::Layout;
    let record = |b: &Binding| {
        let (class_name, class) = if matches!(layout, Layout::Unity51 | Layout::Unity55) {
            (
                "classID",
                V::UInt16(u16::try_from(b.class).unwrap_or_default()),
            )
        } else {
            ("typeID", V::Int(b.class))
        };
        let mut fields = vec![
            ("path", V::UInt(b.path)),
            ("attribute", V::UInt(b.attribute)),
            ("script", pptr("Object", b.script)),
            (class_name, class),
            ("customType", V::UInt8(b.custom)),
            ("isPPtrCurve", V::UInt8(b.pptr)),
        ];
        if matches!(
            layout,
            Layout::Unity20221 | Layout::Unity2022 | Layout::Unity6000
        ) {
            fields.push(("isIntCurve", V::UInt8(b.integer)));
        }
        if matches!(layout, Layout::Unity2022 | Layout::Unity6000) {
            fields.push(("isSerializeReferenceCurve", V::UInt8(b.managed)));
        }
        if !matches!(layout, Layout::Unity51 | Layout::Unity55)
            && let Some((_, last)) = fields.last_mut()
            && let V::UInt8(value) = last
        {
            *last = V::AlignedUInt8(*value);
        }
        st("GenericBinding", fields)
    };
    let table = st(
        "AnimationClipBindingConstant",
        vec![
            (
                "genericBindings",
                vector(
                    record(&Binding::default()),
                    bindings.iter().map(record).collect(),
                ),
            ),
            ("pptrCurveMapping", pointers("Object", mapping)),
        ],
    );
    Object::new(
        id,
        74,
        st("AnimationClip", vec![("m_ClipBindingConstant", table)]),
    )
}

pub mod humanoid;
pub mod playback;
