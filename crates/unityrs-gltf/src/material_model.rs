//! Editable glTF core PBR and the material extensions used by collection.
use crate::{Extras, Text};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// Unity texture ST expressed after collection's V flip. Uses `TEXCOORD_0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureInfo {
    pub texture: usize,
    pub scale: [f64; 2],
    pub offset: [f64; 2],
}
const _: () = crate::send_sync::<TextureInfo>();
/// Core glTF alpha coverage policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    Opaque,
    Mask,
    Blend,
}
const _: () = crate::send_sync::<AlphaMode>();
/// Core glTF material, optionally unlit. Values must meet glTF's ranges.
#[derive(Debug)]
pub struct Material {
    pub name: Option<Text>,
    pub extras: Extras,
    pub base_color: [f64; 4],
    pub base_color_texture: Option<TextureInfo>,
    pub metallic: f64,
    pub roughness: f64,
    pub metallic_roughness_texture: Option<TextureInfo>,
    pub normal_texture: Option<TextureInfo>,
    pub normal_scale: f64,
    pub occlusion_texture: Option<TextureInfo>,
    pub occlusion_strength: f64,
    pub emissive: [f64; 3],
    pub emissive_strength: f64,
    pub emissive_texture: Option<TextureInfo>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f64,
    pub double_sided: bool,
    pub unlit: bool,
    /// Whether the source shader consumes vertex colours. Unknown shaders retain them.
    pub vertex_colors: bool,
}
const _: () = crate::send_sync::<Material>();
impl Material {
    /// glTF core defaults, without allocating.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            name: None,
            extras: MeteredVec::new(meter),
            base_color: [1.; 4],
            base_color_texture: None,
            metallic: 1.,
            roughness: 1.,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.,
            occlusion_texture: None,
            occlusion_strength: 1.,
            emissive: [0.; 3],
            emissive_strength: 1.,
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            unlit: false,
            vertex_colors: true,
        }
    }
    pub(crate) fn textures(&self) -> [Option<TextureInfo>; 5] {
        [
            self.base_color_texture,
            self.metallic_roughness_texture,
            self.normal_texture,
            self.occlusion_texture,
            self.emissive_texture,
        ]
    }
}
