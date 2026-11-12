//! The cross-field rules `template` and `key(<rule>)` (`docs/BATCH_SPEC.md`
//! 1.1, 2.1, 2.3; section 10's order), through `unityrs_export::keys`, the
//! one copy of the grammar and the key rules an attempt expands and checks
//! its keys with.
//!
//! - `template`: every template parses for its kind and its literals pass
//!   2.1's checks: the outputs' keys, then the archives' member paths, in
//!   document order, a member path by every rule but `reserved_prefix` and
//!   `reserved_name` (4.7).
//! - `key(<rule>)`: every literal key passes 2.3's rules but
//!   `prefix_conflict` and `archive_conflict`, which phase 6 decides:
//!   `prefer`'s keys and the archives' keys, each checked as given and then
//!   after the sink's prefix (2.3), and a non-empty prefix itself without
//!   its final `/`, in document order.
//!
//! What only an expansion can break (a variable's value) is each
//! attempt's, an object-level skip `unwritable_key(<rule>)`.

use unityrs_export::keys::{self, TemplateKind};

use super::super::build::{KeySite, Sites, TemplateSite};
use super::super::model::{MemberKind, Sink, Spec};
use super::super::{BadSpec, Rule, pointer};
use crate::reason::KeyRule;

/// The first template in document order that `template` refuses.
pub(super) fn template(spec: &Spec, sites: &Sites) -> Result<(), BadSpec> {
    for &site in &sites.templates {
        let Some((text, kind, member, at)) = located(spec, site) else {
            continue;
        };
        let checked = if member {
            keys::check_member_template(text, kind)
        } else {
            keys::check_template(text, kind)
        };
        if checked.is_err() {
            return Err(BadSpec::new(Rule::Template, at));
        }
    }
    Ok(())
}

/// A template's text, kind, whether it is a member path, and pointer.
fn located(spec: &Spec, site: TemplateSite) -> Option<(&str, TemplateKind, bool, String)> {
    let o = &spec.outputs;
    let out = |key: &str| pointer(&["outputs", key, "key"]);
    Some(match site {
        TemplateSite::Textures => (
            o.textures.as_deref()?,
            TemplateKind::Texture,
            false,
            out("textures"),
        ),
        TemplateSite::Sprites => (
            o.sprites.as_deref()?,
            TemplateKind::Sprite,
            false,
            out("sprites"),
        ),
        TemplateSite::SpriteKey => (
            o.atlases.as_ref()?.sprite_key.as_str(),
            TemplateKind::AtlasSprite,
            false,
            pointer(&["outputs", "atlases", "sprite_key"]),
        ),
        TemplateSite::JsonKey => (
            o.atlases.as_ref()?.json_key.as_str(),
            TemplateKind::AtlasJson,
            false,
            pointer(&["outputs", "atlases", "json_key"]),
        ),
        TemplateSite::Member { archive, member } => {
            let m = spec.archives.get(archive)?.members.get(member)?;
            let kind = match m.kind {
                MemberKind::Texture => TemplateKind::Texture,
                MemberKind::Sprite => TemplateKind::Sprite,
                MemberKind::AtlasSprite => TemplateKind::AtlasSprite,
                MemberKind::AtlasJson => TemplateKind::AtlasJson,
            };
            let at = format!("/archives/{archive}/members/{member}/path");
            (m.path.as_str(), kind, true, at)
        }
    })
}

/// The first literal key that `key(<rule>)` refuses: the sink's prefix
/// first, wherever it stands, since every other key is checked with it,
/// then the others in document order (`docs/BATCH_SPEC.md` section 10).
pub(super) fn key(spec: &Spec, sites: &Sites) -> Result<(), BadSpec> {
    let prefix = match &spec.sink {
        Sink::ObjectStore(s) => s.prefix.as_str(),
        Sink::Directory { .. } => "",
    };
    // Room for a key and its prefix as far as the rules read them, so no
    // check allocates (`keys::check_prefixed`).
    let mut scratch = Vec::with_capacity(keys::STOP);
    let refused = |rule: keys::KeyRule, at: String| BadSpec::new(Rule::Key(rule.into()), at);
    let prefix_first = sites
        .keys
        .iter()
        .filter(|s| matches!(s, KeySite::Prefix))
        .chain(sites.keys.iter().filter(|s| !matches!(s, KeySite::Prefix)));
    for &site in prefix_first {
        match site {
            KeySite::Prefer => {
                for (key, _) in spec.prefer() {
                    if let Err(r) = keys::check_prefixed(key.as_bytes(), prefix, &mut scratch) {
                        return Err(refused(r, pointer(&["prefer", key])));
                    }
                }
            }
            KeySite::Archive(i) => {
                let Some(a) = spec.archives.get(i) else {
                    continue;
                };
                if let Err(r) = keys::check_prefixed(a.key.as_bytes(), prefix, &mut scratch) {
                    return Err(refused(r, format!("/archives/{i}/key")));
                }
            }
            KeySite::Prefix => {
                // The form (`format`) has made a non-empty prefix end in
                // `/`; it passes the rules without it (1.1).
                let Some(bare) = prefix.strip_suffix('/') else {
                    continue;
                };
                if let Err(r) = keys::check(bare.as_bytes()) {
                    return Err(refused(r, "/sink/prefix".to_owned()));
                }
            }
        }
    }
    Ok(())
}

impl From<keys::KeyRule> for KeyRule {
    fn from(rule: keys::KeyRule) -> Self {
        match rule {
            keys::KeyRule::Empty => Self::Empty,
            keys::KeyRule::NotUtf8 => Self::NotUtf8,
            keys::KeyRule::ControlChar => Self::ControlChar,
            keys::KeyRule::LeadingSlash => Self::LeadingSlash,
            keys::KeyRule::TrailingSlash => Self::TrailingSlash,
            keys::KeyRule::EmptySegment => Self::EmptySegment,
            keys::KeyRule::DotSegment => Self::DotSegment,
            keys::KeyRule::SegmentTooLong => Self::SegmentTooLong,
            keys::KeyRule::TooLong => Self::TooLong,
            keys::KeyRule::ReservedPrefix => Self::ReservedPrefix,
            keys::KeyRule::ReservedName => Self::ReservedName,
        }
    }
}

#[cfg(test)]
mod tests {
    use unityrs_export::keys;

    use crate::reason::KeyRule;

    /// The export crate's rules are 2.3's first eleven, by name and order.
    #[test]
    fn the_key_rules_map_by_name() {
        let all = [
            keys::KeyRule::Empty,
            keys::KeyRule::NotUtf8,
            keys::KeyRule::ControlChar,
            keys::KeyRule::LeadingSlash,
            keys::KeyRule::TrailingSlash,
            keys::KeyRule::EmptySegment,
            keys::KeyRule::DotSegment,
            keys::KeyRule::SegmentTooLong,
            keys::KeyRule::TooLong,
            keys::KeyRule::ReservedPrefix,
            keys::KeyRule::ReservedName,
        ];
        for (r, want) in all.into_iter().zip(KeyRule::ALL) {
            assert_eq!(KeyRule::from(r), want);
            assert_eq!(r.name(), want.name());
        }
    }
}
