#![allow(dead_code)]
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_gltf::*;

pub fn floats(m: &Arc<Meter>, kind: Kind, values: &[f64]) -> Result<Accessor, GltfError> {
    let width = match kind {
        Kind::Position
        | Kind::Normal
        | Kind::MorphPosition
        | Kind::MorphNormal
        | Kind::MorphTangent => 3,
        Kind::TexCoord => 2,
        Kind::Indices => 1,
        Kind::InverseBind => 16,
        _ => 4,
    };
    Ok(Accessor {
        kind,
        count: values.len() / width,
        data: Data::Float(array(m, values)?),
        sparse_indices: None,
    })
}
pub fn primitive(m: &Arc<Meter>) -> Result<Primitive, GltfError> {
    Ok(Primitive {
        attributes: array(m, &[(Attribute::Position, 0)])?,
        indices: Some(1),
        material: None,
        targets: MeteredVec::new(m),
        extras: MeteredVec::new(m),
    })
}
pub fn triangle(m: &Arc<Meter>) -> Result<Scene, GltfError> {
    let mut s = Scene::new(m);
    s.accessors.push(floats(
        m,
        Kind::Position,
        &[-0., 0., 0., 1., 0., 0., 0., 1., 0.],
    )?)?;
    s.accessors.push(Accessor {
        kind: Kind::Indices,
        count: 3,
        data: Data::U8(array(m, &[0, 1, 2])?),
        sparse_indices: None,
    })?;
    let mut mesh = Mesh {
        name: Some(text(m, "triangle")?),
        primitives: MeteredVec::new(m),
        target_names: MeteredVec::new(m),
        extras: MeteredVec::new(m),
    };
    mesh.primitives.push(primitive(m)?)?;
    s.meshes.push(mesh)?;
    let mut n = Node::new(m);
    n.mesh = Some(0);
    s.nodes.push(n)?;
    s.roots.push(0)?;
    Ok(s)
}
/// One shared geometry, two material bindings, two per-instance morph weights.
/// Includes all G8 rules, sparse morphs, two index widths, an image and a skin.
#[allow(clippy::too_many_lines)]
pub fn rich(m: &Arc<Meter>) -> Result<Scene, GltfError> {
    let mut s = triangle(m)?;
    s.accessors.push(floats(
        m,
        Kind::Normal,
        &[0., 0., 2., 0., 0., 2., 0., 0., 2.],
    )?)?; // 2
    s.accessors.push(floats(
        m,
        Kind::Tangent,
        &[2., 0., 0., -2., 2., 0., 0., -2., 2., 0., 0., -2.],
    )?)?; // 3
    s.accessors
        .push(floats(m, Kind::TexCoord, &[0., 0., 1., 0., 0., 1.])?)?; // 4
    s.accessors.push(Accessor {
        kind: Kind::Joints,
        count: 3,
        data: Data::U16(array(m, &[0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0])?),
        sparse_indices: None,
    })?; //5
    s.accessors.push(floats(
        m,
        Kind::Weights,
        &[-1., 2., 0., 0., 0., 0., 0., 0., 2., 2., 0., 0.],
    )?)?; //6
    s.accessors.push(Accessor {
        kind: Kind::MorphPosition,
        count: 3,
        data: Data::Float(array(m, &[0., 0., 0.25])?),
        sparse_indices: Some(array(m, &[1])?),
    })?; //7
    s.accessors.push(floats(
        m,
        Kind::InverseBind,
        &[
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 1., 0., 0., 0., 0., 1.,
            0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ],
    )?)?; //8
    for id in 0..2 {
        m.count_decode(1)?;
        s.materials.push(Material {
            name: Some(text(m, if id == 0 { "first" } else { "second" })?),
            extras: MeteredVec::new(m),
        })?;
        let mut p = primitive(m)?;
        p.attributes = array(
            m,
            &[
                (Attribute::Position, 0),
                (Attribute::Normal, 2),
                (Attribute::Tangent, 3),
                (Attribute::TexCoord(0), 4),
                (Attribute::Joints, 5),
                (Attribute::Weights, 6),
            ],
        )?;
        p.material = Some(id);
        p.targets.push([Some(7), None, None])?;
        let mut mesh = Mesh {
            name: None,
            primitives: MeteredVec::new(m),
            target_names: MeteredVec::new(m),
            extras: MeteredVec::new(m),
        };
        mesh.primitives.push(p)?;
        mesh.target_names.push(text(m, "bend")?)?;
        if id == 0 {
            *s.meshes.as_mut_slice().first_mut().unwrap() = mesh;
        } else {
            s.meshes.push(mesh)?;
        }
    }
    let n = s.nodes.as_mut_slice().first_mut().unwrap();
    n.skin = Some(0);
    n.weights.push(0.25)?;
    n.name = Some(text(m, "escaped\"\\\n\0é")?);
    let mut n = Node::new(m);
    n.mesh = Some(1);
    n.skin = Some(0);
    n.weights.push(0.75)?;
    s.nodes.push(n)?;
    s.roots.push(1)?;
    let mut bone = Node::new(m);
    bone.children.push(3)?;
    s.nodes.push(bone)?;
    s.roots.push(2)?;
    s.nodes.push(Node::new(m))?;
    s.skins.push(Skin {
        name: Some(text(m, "skin")?),
        joints: array(m, &[2, 3])?,
        inverse_bind_matrices: Some(8),
        skeleton: Some(2),
    })?;
    s.images.push(Image {
        name: Some(text(m, "pixel")?),
        mime: Mime::Png,
        bytes: array(m, include_bytes!("../goldens/pixel.png"))?,
    })?;
    s.textures.push(Texture { image: 0 })?;
    s.extras.push((text(m, "source")?, text(m, "synthetic")?))?;
    Ok(s)
}
pub fn write(s: &Scene, opts: Options) -> (Vec<u8>, Report) {
    let mut out = Vec::new();
    let report = write_glb(s, &opts, &Meter::unlimited(), &mut out).unwrap();
    (out, report)
}
pub fn json(bytes: &[u8]) -> serde_json::Value {
    let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + n]).unwrap()
}
pub fn bin(bytes: &[u8]) -> &[u8] {
    let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    &bytes[28 + n..]
}
pub fn f32s(bytes: &[u8], j: &serde_json::Value, id: usize) -> Vec<f32> {
    let a = &j["accessors"][id];
    let view = &j["bufferViews"][usize::try_from(a["bufferView"].as_u64().unwrap()).unwrap()];
    let start = usize::try_from(view["byteOffset"].as_u64().unwrap()).unwrap();
    let len = usize::try_from(view["byteLength"].as_u64().unwrap()).unwrap();
    bin(bytes)[start..start + len]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

pub fn morph_directions(m: &Arc<Meter>) -> Result<Scene, GltfError> {
    let mut s = rich(m)?;
    for (kind, delta, index) in [
        (Kind::MorphNormal, [0., 0., 0.1], 1),
        (Kind::MorphTangent, [0.2, 0., 0.], 2),
    ] {
        m.count_decode(1)?;
        s.accessors.push(Accessor {
            kind,
            count: 3,
            data: Data::Float(array(m, &delta)?),
            sparse_indices: Some(array(m, &[index])?),
        })?;
    }
    for mesh in s.meshes.as_mut_slice() {
        for p in mesh.primitives.as_mut_slice() {
            m.count_decode(1)?;
            *p.targets.as_mut_slice().first_mut().unwrap() = [Some(7), Some(9), Some(10)];
        }
    }
    Ok(s)
}

pub fn zero_joints(m: &Arc<Meter>) -> Scene {
    let mut s = rich(m).unwrap();
    s.accessors.as_mut_slice()[5].data =
        Data::U16(array(m, &[1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1]).unwrap());
    s
}
pub fn empty_morph(m: &Arc<Meter>) -> Scene {
    let mut s = morph_directions(m).unwrap();
    for id in [7, 9, 10] {
        s.accessors.as_mut_slice()[id].data = Data::Float(array(m, &[]).unwrap());
        s.accessors.as_mut_slice()[id].sparse_indices = Some(array(m, &[]).unwrap());
    }
    s
}
pub fn texcoords(m: &Arc<Meter>, indices: &[u8]) -> Scene {
    let mut s = triangle(m).unwrap();
    s.accessors
        .push(floats(m, Kind::TexCoord, &[0., 0., 1., 0., 0., 1.]).unwrap())
        .unwrap();
    for &index in indices {
        s.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0]
            .attributes
            .push((Attribute::TexCoord(index), 2))
            .unwrap();
    }
    s
}

// Synthetic 1x1 grayscale JPEG header with a complete SOF0 segment and EOI.
pub const MINIMAL_JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0, 1, 0, 1, 1, 1, 0x11, 0, 0xff, 0xd9,
];
pub fn image(m: &Arc<Meter>, mime: Mime, bytes: &[u8]) -> Scene {
    let mut s = Scene::new(m);
    s.images
        .push(Image {
            name: None,
            mime,
            bytes: array(m, bytes).unwrap(),
        })
        .unwrap();
    s
}

pub fn skinned_triangle(m: &Arc<Meter>, joints: [u16; 4]) -> Scene {
    let mut s = triangle(m).unwrap();
    s.accessors
        .push(Accessor {
            kind: Kind::Joints,
            count: 3,
            data: Data::U16(array(m, &joints.repeat(3)).unwrap()),
            sparse_indices: None,
        })
        .unwrap();
    s.accessors
        .push(floats(m, Kind::Weights, &[1., 0., 0., 0.].repeat(3)).unwrap())
        .unwrap();
    let p = &mut s.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0];
    p.attributes.push((Attribute::Joints, 2)).unwrap();
    p.attributes.push((Attribute::Weights, 3)).unwrap();
    s.nodes.as_mut_slice()[0].skin = Some(0);
    for _ in 0..4 {
        s.nodes.push(Node::new(m)).unwrap();
    }
    s.nodes.as_mut_slice()[1].children = array(m, &[2, 3, 4]).unwrap();
    s.roots.push(1).unwrap();
    s.skins
        .push(Skin {
            name: None,
            joints: array(m, &[1, 2, 3, 4]).unwrap(),
            inverse_bind_matrices: None,
            skeleton: Some(1),
        })
        .unwrap();
    s
}

pub fn parented_skin(m: &Arc<Meter>) -> Scene {
    let mut s = skinned_triangle(m, [1, 0, 0, 0]);
    let mut parent = Node::new(m);
    parent.children.push(0).unwrap();
    s.roots.as_mut_slice()[0] = s.nodes.len();
    s.nodes.push(parent).unwrap();
    s
}
pub fn transformed_skin(m: &Arc<Meter>) -> Scene {
    let mut s = skinned_triangle(m, [1, 0, 0, 0]);
    s.nodes.as_mut_slice()[0].translation = [1., 0., 0.];
    s
}

// The omitted primitive shares JOINTS but would preserve its stale second column.
pub fn omitted_joint_influence(m: &Arc<Meter>, dropped_first: bool) -> Scene {
    let mut s = skinned_triangle(m, [1, 9, 0, 0]);
    s.accessors
        .push(
            floats(
                m,
                Kind::Position,
                &[f64::NAN, 0., 0., 1., 0., 0., 0., 1., 0.],
            )
            .unwrap(),
        )
        .unwrap();
    s.accessors
        .push(floats(m, Kind::Weights, &[0., 1., 0., 0.].repeat(3)).unwrap())
        .unwrap();
    let mut dropped = primitive(m).unwrap();
    dropped.attributes = array(
        m,
        &[
            (Attribute::Position, 4),
            (Attribute::Joints, 2),
            (Attribute::Weights, 5),
        ],
    )
    .unwrap();
    let primitives = &mut s.meshes.as_mut_slice()[0].primitives;
    if dropped_first {
        let surviving = std::mem::replace(&mut primitives.as_mut_slice()[0], dropped);
        primitives.push(surviving).unwrap();
    } else {
        primitives.push(dropped).unwrap();
    }
    s
}

mod images;
pub fn image_review_cases() -> Vec<images::Case> {
    images::review_cases()
}
