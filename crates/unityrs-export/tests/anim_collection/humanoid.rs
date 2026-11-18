//! The collector returns the typed humanoid refusal for a selected clip.
use crate::{ALL, animated, input};
use unityrs_anim::{AnimError, BodyMotion};
use unityrs_export::gltf::ExportError;
use unityrs_testkit::{
    ngui::V,
    scene3d::{
        self as s, Layout,
        anim::humanoid::{self as h, Rig},
    },
};

fn collect(rig: &Rig, settings: &[(&str, V)]) -> ExportError {
    let mut objects = h::prefab(rig);
    objects.push(h::clip(&[42, 43, 44, 42 + 42], settings));
    let (env, root, graph, meter) = input(s::file(Layout::Unity2022, &[], &objects).unwrap(), 0);
    animated(&env, &root, &graph, &meter, ALL).unwrap_err()
}

#[test]
fn a_selected_humanoid_clip_refuses_with_its_typed_cause() {
    // The forearm twist redistributes over the clip's Left Forearm Stretch.
    let twist = Rig {
        twist: [0., 0.25, 0., 0.],
        ..Rig::default()
    };
    assert!(matches!(
        collect(&twist, &[]),
        ExportError::Animation(AnimError::UnsupportedHumanoidTwist("m_ForeArmTwist"))
    ));
    assert!(matches!(
        collect(&Rig::default(), &[("m_Level", V::Float(0.125))]),
        ExportError::Animation(AnimError::UnsupportedHumanoidClipSettings("m_Level"))
    ));
    let no_avatar = Rig {
        avatar: false,
        ..Rig::default()
    };
    assert!(matches!(
        collect(&no_avatar, &[]),
        ExportError::Animation(AnimError::MissingAvatar)
    ));
    assert!(matches!(
        collect(&Rig::default(), &[]),
        ExportError::Animation(AnimError::UnsupportedHumanoidPlayback)
    ));
    let offset = Rig {
        body_offset: [0., 0.9, 0.],
        ..Rig::default()
    };
    assert!(matches!(
        collect(&offset, &[]),
        ExportError::Animation(AnimError::UnsupportedBodyMotion(BodyMotion::HipsCorrection))
    ));
}
