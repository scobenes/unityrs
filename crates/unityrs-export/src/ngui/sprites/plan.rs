// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's get_image_from_sprite and get_image (a Sprite's checks before its pixels, its pointers' truth and path ids, and its sheet's dereference and read), and export/Texture2DConverter.py's parse_image_data for a side of 0. See THIRD_PARTY_LICENSES.
//! The sprites kind's plan (4.4; module docs of [`super`]): every Sprite of
//! the input's own files, in object order, through steps 1 to 3, each
//! planned Sprite joined to its group. A sheet's answers (its object read
//! and, with no side of 0, section 3's answers 2 and 3) are asked once per
//! texture and kept by its identity; the groups are found by their keys.
//! Both tables are hash tables charged to `M_X` before they grow (B3's
//! review round 1, R3), and a failure they keep is a [`Kept`].

use std::sync::Arc;

use unityrs_core::files::SerializedFile;
use unityrs_core::{ObjectRef, RawStr};
use unityrs_texture::sprite::SpriteData;

use super::super::attempt::{Attempt, Site, UNKNOWN_PLATFORM, Vars};
use super::super::classify::{self, Class, Reason};
use super::super::kept::Kept;
use super::super::keys::{Template, TemplateKind};
use super::super::read::{self, Texture, top_bundle};
use super::super::table::{self, Map, Set};
use super::super::{Failure, SkipOutput};
use super::atlas::Atlases;
use super::object::{self, Alpha};
use super::{GroupKey, GroupSheet, Resolved, SPRITE, SheetId, SpritePlan, Sprites, follow};

/// The plan's scratch: the names taken, the sheets' answers (none, or the
/// skip, kept) and the atlas tables.
struct Scratch {
    names: Set<RawStr>,
    answers: Map<SheetId, Option<Kept>>,
    atlases: Atlases,
}

/// Section 3's answers for a Sprite's sheet at the plan: none for a side
/// of 0 (UnityPy's `parse_image_data` returns its 0 x 0 image before it
/// reads the format, `export/Texture2DConverter.py`), else answers 2 and 3.
pub(super) fn sheet_answers(t: &Texture, obj: &ObjectRef) -> Result<(), Class> {
    if t.width == 0 || t.height == 0 {
        return Ok(());
    }
    let platform = obj.file().target_platform().unwrap_or(UNKNOWN_PLATFORM);
    read::formats(t, platform).map(|_| ())
}

impl Attempt<'_> {
    /// The sprites kind (4.4), between the textures' stage and the
    /// atlases', so a Sprite takes its key before an atlas does: nothing
    /// without `outputs.sprites`.
    pub fn plan_sprites(&mut self, files: &[Arc<SerializedFile>]) -> Result<Sprites, Failure> {
        let meter = self.seam.meter;
        let mut out = Sprites::new(meter);
        let Some(template) = self.options.sprites_key.clone() else {
            return Ok(out);
        };
        let mut scratch = Scratch {
            names: Set::new(meter),
            answers: Map::new(meter),
            atlases: Atlases::new(meter),
        };
        for f in files {
            for obj in f.objects() {
                if obj.class_id() != SPRITE {
                    continue;
                }
                self.seam.stop()?;
                self.plan_sprite(&obj, &template, &mut scratch, &mut out)?;
            }
        }
        Ok(out)
    }

    /// One Sprite through steps 1 to 3: its name, then the skips that need
    /// no pixels, then its key; a planned one joins its group.
    fn plan_sprite(
        &mut self,
        obj: &ObjectRef,
        template: &Template,
        scratch: &mut Scratch,
        out: &mut Sprites,
    ) -> Result<(), Failure> {
        let site = Site {
            obj,
            output: SkipOutput::Sprite,
            sprite: None,
        };
        let seam = self.seam;
        let name = match self.reader.peek_name(&seam, obj) {
            Ok(n) => n,
            Err(c) => return self.record(c, &site),
        };
        if scratch.names.contains(&name) {
            return self.skip(&site, Reason::plain("duplicate_name"));
        }
        let names = &mut scratch.names;
        self.charged(|| names.insert(name.clone()))?;
        let (group, sheet, data) = match self.sheets(obj, scratch) {
            Ok(g) => g,
            Err(c) => return self.record(c, &site),
        };
        let vars = Vars {
            name: name.as_bytes(),
            ..Vars::default()
        };
        self.expand(template, obj, vars);
        let Some(key) = self.take(&site)? else {
            return Ok(());
        };
        let members = self.member_paths(TemplateKind::Sprite, obj, vars)?;
        let at = self.charged(|| join(out, group, sheet))?;
        let plan = SpritePlan {
            plan_index: 0,
            obj: obj.clone(),
            data,
            group: at,
            key,
            members,
        };
        let sprites = &mut out.planned;
        self.charged(|| table::push(sprites, plan))
    }

    /// Step 2 past the name: the Sprite resolved ([`Self::resolve`]), then,
    /// in `get_image`'s order (`export/SpriteHelper.py:44-59`; B3's review
    /// round 2, S5), its texture dereferenced from the file that holds the
    /// render data and its answers asked, then, where set, its alpha
    /// texture's the same; the group's key and record, and the render data;
    /// or the skip.
    fn sheets(
        &mut self,
        obj: &ObjectRef,
        scratch: &mut Scratch,
    ) -> Result<(GroupKey, GroupSheet, SpriteData), Class> {
        let seam = self.seam;
        let resolved = self.resolve(obj, &mut scratch.atlases)?;
        let rd = resolved.rd;
        // `export/SpriteHelper.py:84`: an `UnknownObject`'s render data
        // with no `alphaTexture` raises; then `get_image`'s truth test of
        // it (`export/SpriteHelper.py:47`) and its `.path_id`s, the
        // texture's first (`export/SpriteHelper.py:48`, or
        // `export/SpriteHelper.py:55` with none), which a pointer that is
        // no `PPtr` lacks, before any dereference (B3's review round 3,
        // S3a).
        let alpha = match rd.alpha {
            Alpha::Missing => return Err(object::render()),
            Alpha::None => None,
            Alpha::Set(p) => Some(p).filter(|p| p.is_set()),
        };
        let texture_ids = rd.texture.ids().ok_or_else(object::render)?;
        let alpha_ids = match alpha {
            Some(p) => Some(p.ids().ok_or_else(object::render)?),
            None => None,
        };
        let (texture, texture_ptr) = follow(&seam, &resolved.holder, texture_ids)?;
        let _first = top_bundle(texture.file());
        let sheet = self.answer(&texture, scratch)?;
        drop(texture);
        let (alpha, alpha_ptr) = match alpha_ids {
            Some(ids) => {
                let (alpha, p) = follow(&seam, &resolved.holder, ids)?;
                let _second = top_bundle(alpha.file());
                (Some(self.answer(&alpha, scratch)?), Some(p))
            }
            None => (None, None),
        };
        let record = GroupSheet {
            from: obj.file().clone(),
            atlas: resolved.atlas,
            texture: texture_ptr,
            alpha: alpha_ptr,
        };
        Ok(((sheet, alpha), record, resolved.data))
    }

    /// `obj`'s Sprite resolved: its checked read (4.4 step 2), as UnityPy's
    /// `Sprite` ([`object::sprite`]: one that is not, or a field of another
    /// shape than E17's, is `read`), and its render data
    /// ([`Self::render_data`]).
    fn resolve(&mut self, obj: &ObjectRef, atlases: &mut Atlases) -> Result<Resolved, Class> {
        let seam = self.seam;
        let meter = seam.meter;
        let parsed = seam.probe(|| {
            obj.read_typetree(None, true)
                .map_err(|e| classify::parse(&e))
        })?;
        let sprite = seam.probe(|| object::sprite(&parsed, meter))?;
        let (rd, holder, atlas, held) = self.render_data(obj, &sprite, atlases)?;
        let data = SpriteData {
            texture_rect: rd.texture_rect,
            settings_raw: rd.settings_raw,
            pixels_to_units: sprite.ppu,
        };
        Ok(Resolved {
            data,
            rd,
            holder,
            atlas,
            _held: held,
        })
    }

    /// A texture's answers at the plan (its object read, then
    /// [`sheet_answers`]), asked once per texture: its identity, or its
    /// skip. A refusal of `M_X`'s is the Sprite's, not kept.
    fn answer(&mut self, obj: &ObjectRef, scratch: &mut Scratch) -> Result<SheetId, Class> {
        let seam = self.seam;
        let meter = seam.meter;
        let atlases = &mut scratch.atlases;
        let file = seam.probe(|| atlases.files.id(obj.file(), meter))?;
        let id = SheetId {
            file,
            path_id: obj.path_id(),
        };
        if scratch.answers.get(&id).is_none() {
            let answer = seam
                .probe(|| read::texture(obj))
                .and_then(|t| sheet_answers(&t, obj));
            let kept = match answer {
                Ok(()) => None,
                // A refusal of `M_X`'s is this Sprite's, not kept.
                Err(c) => Some(Kept::keep(meter, seam.hooks, c)?),
            };
            let answers = &mut scratch.answers;
            seam.probe(|| answers.insert(id, kept))?;
        }
        match scratch.answers.get(&id) {
            Some(Some(k)) => Err(k.class()),
            Some(None) => Ok(id),
            None => Err(Class::Run(Reason::plain("internal"))),
        }
    }
}

/// The index in `out.groups` of `key`'s group: found by its key, or added
/// at the end with `sheet`, its record (so the groups run in the order of
/// each one's first Sprite).
fn join(out: &mut Sprites, key: GroupKey, sheet: GroupSheet) -> Result<u32, Class> {
    if let Some(n) = out.index.get(&key) {
        return Ok(*n);
    }
    let n = u32::try_from(out.groups.len()).map_err(|_| Class::Run(Reason::plain("internal")))?;
    out.groups.reserve(1).map_err(|e| table::refused(&e))?;
    out.index.insert(key, n)?;
    table::push(&mut out.groups, sheet)?;
    Ok(n)
}
