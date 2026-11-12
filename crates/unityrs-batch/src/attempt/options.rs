//! Compile validated export templates once, outside worker extraction.
use crate::spec::model::{MemberKind, Spec};
use unityrs_export::ngui::{MemberTemplate, Options, Template, TemplateError, TemplateKind};
#[derive(Clone, Debug)]
pub struct Prepared {
    pub options: Options,
    pub members: Vec<MemberTemplate>,
}
impl Prepared {
    pub fn from_spec(spec: &Spec) -> Result<Self, TemplateError> {
        let atlases = spec.outputs.atlases.as_ref();
        let options = Options {
            sprite_key: Template::parse(
                atlases.map_or("unused", |a| a.sprite_key.as_str()),
                TemplateKind::AtlasSprite,
            )?,
            json_key: Template::parse(
                atlases.map_or("unused", |a| a.json_key.as_str()),
                TemplateKind::AtlasJson,
            )?,
            texture_key: spec
                .outputs
                .textures
                .as_deref()
                .map(|s| Template::parse(s, TemplateKind::Texture))
                .transpose()?,
            sprites_key: spec
                .outputs
                .sprites
                .as_deref()
                .map(|s| Template::parse(s, TemplateKind::Sprite))
                .transpose()?,
            script_classes: atlases.map_or_else(Vec::new, |a| {
                a.script_classes
                    .iter()
                    .map(|s| s.as_bytes().to_vec())
                    .collect()
            }),
            skip_name_suffixes: atlases.map_or_else(Vec::new, |a| {
                a.skip_name_suffixes
                    .iter()
                    .map(|s| s.as_bytes().to_vec())
                    .collect()
            }),
            atlases: atlases.is_some(),
        };
        let mut members = Vec::new();
        for (archive, a) in spec.archives.iter().enumerate() {
            for m in &a.members {
                let kind = match m.kind {
                    MemberKind::Texture => TemplateKind::Texture,
                    MemberKind::Sprite => TemplateKind::Sprite,
                    MemberKind::AtlasSprite => TemplateKind::AtlasSprite,
                    MemberKind::AtlasJson => TemplateKind::AtlasJson,
                };
                members.push(MemberTemplate {
                    archive,
                    kind,
                    path: Template::parse(&m.path, kind)?,
                });
            }
        }
        Ok(Self { options, members })
    }
}
