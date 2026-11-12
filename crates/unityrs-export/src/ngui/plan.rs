//! 4.4's plan: names (step 1), the skips that need no pixels (step 2) and
//! keys (step 3), in plan order: the textures kind, then the sprites kind
//! (E17's, [`super::sprites`]), then the atlases, each in object
//! order, an atlas's JSON before its sprites. Every kind takes its keys
//! from the engine's one table ([`super::KeyTable`]), each key checked as
//! given and after the sink's prefix (2.3), and expands its archive member
//! paths (4.7). The plan's tables are [`MeteredVec`]s on `M_X`, charged at
//! their capacity before they grow, and its strings are charged before
//! they are made (`docs/plans/batch.md` 2.6 steps 5 and 10; review round
//! 2's S3).

use std::sync::Arc;

use unityrs_core::classid::{MONO_BEHAVIOUR, MONO_SCRIPT};
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::{Charge, MeteredVec};
use unityrs_core::{ObjectRef, Parsed, RawStr, Value};
use unityrs_texture::{MAX_CROP_PIXELS, Mode};

use super::attempt::{Attempt, Site, Tex, Vars, unwritable};
use super::classify::{self, Class, Reason};
use super::keys::{KeyRule, Template, TemplateKind};
use super::read::{self, field, shape, top_bundle};
use super::recognize::has_sprite_list;
use super::table::{self, Set};
use super::{Failure, MemberPath, SheetBy, SheetRow, SkipOutput};

/// `Texture2D`'s class id.
pub const TEXTURE_2D: i32 = 28;

/// A planned texture output.
pub(super) struct TexturePlan {
    pub tex: usize,
    pub key: String,
    pub members: MeteredVec<MemberPath>,
}

/// A planned sprite: its index, rectangle and key.
pub(super) struct SpritePlan {
    pub index: usize,
    pub x: i128,
    pub y: i128,
    pub width: u64,
    pub height: u64,
    pub key: String,
    pub members: MeteredVec<MemberPath>,
}

/// A planned atlas: past every plan skip, its outputs with their keys.
pub(super) struct AtlasPlan {
    pub obj: ObjectRef,
    pub name: RawStr,
    pub parsed: Parsed,
    pub sheet: usize,
    pub json_key: Option<String>,
    pub json_members: MeteredVec<MemberPath>,
    pub sprites: MeteredVec<SpritePlan>,
}

impl AtlasPlan {
    /// Whether an output is still planned after step 3.
    pub fn has_outputs(&self) -> bool {
        self.json_key.is_some() || !self.sprites.is_empty()
    }
}

/// The plan, and its strings' charge, dropped with it.
pub(super) struct Planned {
    pub textures: MeteredVec<TexturePlan>,
    pub sprites: super::sprites::Sprites,
    pub atlases: MeteredVec<AtlasPlan>,
    _strings: Charge,
}

/// A sprite element as read: its name and rectangle.
struct Element {
    name: RawStr,
    x: i128,
    y: i128,
    width: i128,
    height: i128,
}

fn int128(v: &Value) -> Option<i128> {
    match v {
        Value::Int(i) => Some(i128::from(*i)),
        Value::UInt(u) => Some(i128::from(*u)),
        _ => None,
    }
}

/// `mSprites` as read: each element's name and rectangle, in a table
/// charged at its length before it is collected; none when a value is of
/// another shape.
fn elements(
    parsed: &Parsed,
    meter: &Arc<unityrs_core::limits::Meter>,
) -> Result<Option<MeteredVec<Element>>, Class> {
    let items = match field(parsed, parsed.value(), b"mSprites") {
        Some(Value::List(items)) => items.as_slice(),
        Some(Value::Prims(p)) if p.is_empty() => &[],
        _ => return Ok(None),
    };
    let mut out = table::with_capacity(meter, items.len())?;
    for e in items {
        let int = |n: &[u8]| field(parsed, e, n).and_then(int128);
        let Some(Value::Str(name)) = field(parsed, e, b"name") else {
            return Ok(None);
        };
        let (Some(x), Some(y), Some(width), Some(height)) =
            (int(b"x"), int(b"y"), int(b"width"), int(b"height"))
        else {
            return Ok(None);
        };
        table::push(
            &mut out,
            Element {
                name: name.clone(),
                x,
                y,
                width,
                height,
            },
        )?;
    }
    Ok(Some(out))
}

/// Whether a reason is of the budget class (exit 5), which section 5
/// records as a skip where it otherwise only counts or swallows.
fn is_budget(r: &Reason) -> bool {
    r.reason == "budget(input)"
        || ["input_memory", "input_bytes", "decode_work"]
            .iter()
            .any(|k| r.reason.starts_with(&format!("limit({k},")))
}

/// The PNG encoder's mode of an image's.
pub(super) fn png_mode(m: Mode) -> unityrs_imageio::Mode {
    match m {
        Mode::Rgb => unityrs_imageio::Mode::Rgb,
        Mode::Rgba => unityrs_imageio::Mode::Rgba,
    }
}

/// Whether Pillow 9.5.0's PNG encoder refuses an image `width` pixels wide
/// in `mode` (`unityrs_imageio::ImageError::TooWide`: wider than
/// `INT_MAX / bits - 7`), from the plan of a one-row image, which
/// allocates nothing. Three sites ask it: a crop at its plan, before its
/// pixels are made (review round 2's C5); a plain candidate from its
/// decode's plan, before the decode; and a sheet's texture output after the
/// sheet's decode, before its PNG is made (round 4's A1).
pub(super) fn too_wide(width: u32, mode: Mode) -> bool {
    matches!(
        unityrs_imageio::plan(width, 1, png_mode(mode)),
        Err(unityrs_imageio::ImageError::TooWide { .. })
    )
}

/// A sprite's plan skip, kept as a code until it is recorded, when its
/// strings are made and charged (review round 3's P2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// `empty_rect`.
    EmptyRect,
    /// `decode_error`, `decompression_bomb`.
    Bomb,
    /// `decode_error`, `crop_overflow`.
    Overflow,
    /// `decode_error`, `pillow_memory_error`: the PNG encoder's width.
    TooWide,
    /// `unwritable_key(<rule>)`.
    Key(KeyRule),
}

impl Refusal {
    /// Its reason, made as it is recorded.
    pub fn reason(self) -> Reason {
        match self {
            Self::EmptyRect => Reason::plain("empty_rect"),
            Self::Bomb => Reason::decode_error("decompression_bomb"),
            Self::Overflow => Reason::decode_error("crop_overflow"),
            Self::TooWide => Reason::decode_error("pillow_memory_error"),
            Self::Key(rule) => unwritable(rule),
        }
    }
}

/// A sprite's place in E30's table: its key's bytes in the table's buffer,
/// or its skip's code.
type Pending = Result<(usize, usize), Refusal>;

impl Attempt<'_> {
    /// The plan over the input's own files, in plan order.
    pub fn plan(&mut self, files: &[Arc<SerializedFile>]) -> Result<Planned, Failure> {
        let meter = self.seam.meter;
        let mut behaviours: MeteredVec<ObjectRef> = MeteredVec::new(meter);
        for f in files {
            for obj in f.objects() {
                match obj.class_id() {
                    TEXTURE_2D => {
                        let texs = &mut self.texs;
                        self.seam
                            .probe(|| table::push(texs, Tex::new(obj)))
                            .map_err(Failure::from)?;
                    }
                    MONO_BEHAVIOUR if self.options.atlases => {
                        self.charged(|| table::push(&mut behaviours, obj))?;
                    }
                    _ => {}
                }
            }
        }
        let textures = match self.options.texture_key.clone() {
            Some(t) => self.plan_textures(&t)?,
            None => MeteredVec::new(meter),
        };
        let sprites = self.plan_sprites(files)?;
        let mut atlases = MeteredVec::new(meter);
        let mut names: Set<RawStr> = Set::new(meter);
        for obj in behaviours.as_slice() {
            self.seam.stop()?;
            if let Some(a) = self.plan_atlas(obj, &mut names)? {
                self.charged(|| table::push(&mut atlases, a))?;
            }
        }
        drop(names);
        drop(behaviours);
        let strings = std::mem::replace(&mut self.plan_strings, Charge::zero(meter));
        Ok(Planned {
            textures,
            sprites,
            atlases,
            _strings: strings,
        })
    }

    /// The textures kind: a texture whose name an earlier one has is
    /// `duplicate_name` (its name's first owner is another).
    fn plan_textures(&mut self, template: &Template) -> Result<MeteredVec<TexturePlan>, Failure> {
        self.names()?;
        let mut plans = MeteredVec::new(self.seam.meter);
        for i in 0..self.texs.len() {
            self.seam.stop()?;
            let Some(t) = self.texs.as_slice().get(i) else {
                continue;
            };
            let obj = t.obj.clone();
            let site = Site {
                obj: &obj,
                output: SkipOutput::Texture,
                sprite: None,
            };
            // A name `names` left unread (a refusal) is read again here,
            // by its own texture (B3's review round 3, S9).
            if let Err(c) = self.name(i) {
                self.record(c, &site)?;
                continue;
            }
            let name = match self.texs.as_slice().get(i).and_then(|t| t.name.as_ref()) {
                Some(Ok(n)) => n.clone(),
                Some(Err(k)) => {
                    let c = k.class();
                    self.record(c, &site)?;
                    continue;
                }
                None => continue,
            };
            match self.owner(name.as_bytes()) {
                Ok(Some(o)) if o == i => {}
                Ok(_) => {
                    self.skip(&site, Reason::plain("duplicate_name"))?;
                    continue;
                }
                Err(c) => {
                    self.record(c, &site)?;
                    continue;
                }
            }
            if let Err(c) = self.sheet_answer(i) {
                self.record(c, &site)?;
                continue;
            }
            let vars = Vars {
                name: name.as_bytes(),
                ..Vars::default()
            };
            self.expand(template, &obj, vars);
            if let Some(key) = self.take(&site)? {
                let members = self.member_paths(TemplateKind::Texture, &obj, vars)?;
                let plan = TexturePlan {
                    tex: i,
                    key,
                    members,
                };
                self.charged(|| table::push(&mut plans, plan))?;
            }
        }
        Ok(plans)
    }

    /// Recognition 1: whether `obj`'s `m_Script` names an atlas class, and
    /// its head's name. A failed read is counted, and recorded too when
    /// of the budget class.
    fn recognise(&mut self, obj: &ObjectRef) -> Result<Option<RawStr>, Failure> {
        let site = Site {
            obj,
            output: SkipOutput::Atlas,
            sprite: None,
        };
        match self.script_class(obj) {
            Ok(Some((class, name))) => {
                let listed = self
                    .options
                    .script_classes
                    .iter()
                    .any(|c| c.as_slice() == class.as_bytes());
                Ok(listed.then_some(name))
            }
            Ok(None) => Ok(None),
            Err(Class::Skip(r)) => {
                self.unresolved();
                if is_budget(&r) {
                    self.skip(&site, r)?;
                }
                Ok(None)
            }
            Err(c) => Err(c.into()),
        }
    }

    /// The head, its `m_Script`'s MonoScript and its `m_ClassName`; none
    /// when the pointer names no MonoScript. Each core call is under the
    /// probe on its own (review round 2's S4).
    fn script_class(&mut self, obj: &ObjectRef) -> Result<Option<(RawStr, RawStr)>, Class> {
        let seam = self.seam;
        let reader = &self.reader;
        let head = seam.probe(|| reader.head(obj))?;
        let target = read::deref(&seam, obj.file(), head.script)?;
        let _top = top_bundle(target.file());
        if target.class_id() != MONO_SCRIPT {
            return Ok(None);
        }
        let script = seam.probe(|| {
            target
                .read_typetree(None, true)
                .map_err(|e| classify::parse(&e))
        })?;
        match field(&script, script.value(), b"m_ClassName") {
            Some(Value::Str(s)) => Ok(Some((s.clone(), head.name))),
            _ => Err(shape()),
        }
    }

    /// One MonoBehaviour through section 5 and 4.4 to a planned atlas.
    fn plan_atlas(
        &mut self,
        obj: &ObjectRef,
        names: &mut Set<RawStr>,
    ) -> Result<Option<AtlasPlan>, Failure> {
        let Some(name) = self.recognise(obj)? else {
            return Ok(None);
        };
        let site = Site {
            obj,
            output: SkipOutput::Atlas,
            sprite: None,
        };
        if names.contains(&name) {
            self.skip(&site, Reason::plain("duplicate_name"))?;
            return Ok(None);
        }
        self.charged(|| names.insert(name.clone()))?;
        let tree = self
            .seam
            .probe(|| obj.tree(None).map_err(|e| classify::parse(&e)));
        let tree = match tree {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.skip(&site, Reason::plain("no_typetree"))?;
                return Ok(None);
            }
            Err(c) => {
                self.record(c, &site)?;
                return Ok(None);
            }
        };
        if !has_sprite_list(tree.tree.list()) {
            self.skip(&site, Reason::plain("ngui_layout_unsupported"))?;
            return Ok(None);
        }
        if name.is_empty() {
            self.skip(&site, Reason::plain("unnamed_atlas"))?;
            return Ok(None);
        }
        let reference = self
            .options
            .skip_name_suffixes
            .iter()
            .any(|s| name.as_bytes().ends_with(s));
        if reference {
            self.skip(&site, Reason::plain("reference_atlas"))?;
            return Ok(None);
        }
        let parsed = self.seam.probe(|| {
            obj.read_typetree(Some(&tree.tree), true)
                .map_err(|e| classify::parse(&e))
        });
        drop(tree);
        let parsed = match parsed {
            Ok(p) => p,
            Err(c) => {
                self.record(c, &site)?;
                return Ok(None);
            }
        };
        let meter = self.seam.meter;
        let Some(elements) = self.charged(|| elements(&parsed, meter))? else {
            self.record(shape(), &site)?;
            return Ok(None);
        };
        let found = match self.find_sheet(obj, &name, &parsed, &site)? {
            Ok(found) => found,
            // A texture's name read late was refused: the atlas's skip.
            Err(c) => {
                self.record(c, &site)?;
                return Ok(None);
            }
        };
        let Some((sheet, by)) = found else {
            self.skip(&site, Reason::plain("sheet_not_found"))?;
            return Ok(None);
        };
        if let Some(t) = self.texs.as_slice().get(sheet) {
            let row = SheetRow {
                atlas_file: obj.file().name().clone(),
                atlas_path_id: obj.path_id(),
                by,
                texture_file: t.obj.file().name().clone(),
                texture_path_id: t.obj.path_id(),
            };
            self.sheet(row)?;
        }
        let format = match self.sheet_answer(sheet) {
            Ok(f) => f,
            Err(c) => {
                self.record(c, &site)?;
                return Ok(None);
            }
        };
        let mut atlas = AtlasPlan {
            obj: obj.clone(),
            name,
            parsed,
            sheet,
            json_key: None,
            json_members: MeteredVec::new(meter),
            sprites: MeteredVec::new(meter),
        };
        self.plan_keys(&mut atlas, elements.as_slice(), format.mode())?;
        Ok(Some(atlas))
    }

    /// Section 5 steps 1 and 2: the sheet, and how it was found.
    fn find_sheet(
        &mut self,
        obj: &ObjectRef,
        name: &RawStr,
        parsed: &Parsed,
        site: &Site<'_>,
    ) -> Result<Result<Option<(usize, SheetBy)>, Class>, Failure> {
        let seam = self.seam;
        let material = match read::material_name(&seam, &mut self.reader, obj, parsed) {
            Ok(n) => n,
            Err(Class::Skip(r)) => {
                if is_budget(&r) {
                    self.skip(site, r)?;
                }
                None
            }
            Err(c) => return Err(c.into()),
        };
        self.names()?;
        if let Some(m) = material {
            match self.owner(m.as_bytes()) {
                Ok(Some(i)) => return Ok(Ok(Some((i, SheetBy::MaterialName)))),
                Ok(None) => {}
                Err(c) => return Ok(Err(c)),
            }
        }
        Ok(self
            .owner(name.as_bytes())
            .map(|o| o.map(|i| (i, SheetBy::AtlasName))))
    }

    /// The atlas's crops' plan skips and every key (steps 2 and 3, E30).
    /// The sprites' keys go into one buffer, and E30's later-wins rule is a
    /// permutation of the keyed sprites sorted by (key, index), a sprite
    /// losing when the next in that order has its key; each table charged
    /// to `M_X` at its capacity, and dropped after.
    fn plan_keys(
        &mut self,
        atlas: &mut AtlasPlan,
        elements: &[Element],
        mode: Mode,
    ) -> Result<(), Failure> {
        let options = self.options;
        let meter = self.seam.meter;
        let obj = atlas.obj.clone();
        let atlas_name = atlas.name.clone();
        let json_site = Site {
            obj: &obj,
            output: SkipOutput::AtlasJson,
            sprite: None,
        };
        let json_vars = Vars {
            atlas: atlas_name.as_bytes(),
            ..Vars::default()
        };
        self.expand(&options.json_key, &obj, json_vars);
        atlas.json_key = self.take(&json_site)?;
        if atlas.json_key.is_some() {
            atlas.json_members = self.member_paths(TemplateKind::AtlasJson, &obj, json_vars)?;
        }
        // Step 2 for each sprite, and its key.
        let n = elements.len();
        let mut pending: MeteredVec<Pending> = self.charged(|| table::with_capacity(meter, n))?;
        let mut keys: MeteredVec<u8> = MeteredVec::new(meter);
        for e in elements {
            let row = if let Some(r) = crop_skip(e, mode) {
                Err(r)
            } else {
                let vars = Vars {
                    atlas: atlas_name.as_bytes(),
                    sprite: e.name.as_bytes(),
                    ..Vars::default()
                };
                self.expand(&options.sprite_key, &obj, vars);
                if let Err(rule) = self.check_key() {
                    Err(Refusal::Key(rule))
                } else {
                    let start = keys.len();
                    let key = self.scratch_key();
                    self.seam
                        .probe(|| keys.extend_from_slice(key).map_err(|e| table::refused(&e)))
                        .map_err(Failure::from)?;
                    Ok((start, keys.len()))
                }
            };
            self.charged(|| table::push(&mut pending, row))?;
        }
        // E30: of two sprites with one key, the later one wins.
        let lost = self.losers(&pending, &keys)?;
        let (pending, _pending) = pending.into_parts();
        let mut sprites = MeteredVec::new(meter);
        for (index, (row, e)) in pending.into_iter().zip(elements).enumerate() {
            let site = Site {
                obj: &obj,
                output: SkipOutput::AtlasSprite,
                sprite: Some(index),
            };
            let (start, end) = match row {
                Err(r) => {
                    self.skip(&site, r.reason())?;
                    continue;
                }
                Ok(_) if lost.as_slice().get(index) == Some(&true) => {
                    self.skip(&site, Reason::plain("duplicate_key"))?;
                    continue;
                }
                Ok(range) => range,
            };
            let key = keys.as_slice().get(start..end).unwrap_or_default();
            let Some(k) = self.take_checked(key, &site)? else {
                continue;
            };
            let vars = Vars {
                atlas: atlas_name.as_bytes(),
                sprite: e.name.as_bytes(),
                ..Vars::default()
            };
            let members = self.member_paths(TemplateKind::AtlasSprite, &obj, vars)?;
            let plan = SpritePlan {
                index,
                x: e.x,
                y: e.y,
                width: u64::try_from(e.width).unwrap_or(0),
                height: u64::try_from(e.height).unwrap_or(0),
                key: k,
                members,
            };
            self.charged(|| table::push(&mut sprites, plan))?;
        }
        atlas.sprites = sprites;
        Ok(())
    }

    /// E30's losers: a flag per sprite, set when a later sprite has its key.
    fn losers(
        &self,
        pending: &MeteredVec<Pending>,
        keys: &MeteredVec<u8>,
    ) -> Result<MeteredVec<bool>, Failure> {
        let meter = self.seam.meter;
        let rows = pending.as_slice();
        let key = |i: u32| {
            let i = usize::try_from(i).unwrap_or(usize::MAX);
            match rows.get(i) {
                Some(Ok((s, e))) => keys.as_slice().get(*s..*e).unwrap_or_default(),
                _ => &[],
            }
        };
        let mut order: MeteredVec<u32> =
            self.charged(|| table::with_capacity(meter, rows.len()))?;
        for (i, row) in rows.iter().enumerate() {
            if row.is_ok() {
                let i = u32::try_from(i).map_err(|_| Failure::Run(Reason::plain("internal")))?;
                self.charged(|| table::push(&mut order, i))?;
            }
        }
        order
            .as_mut_slice()
            .sort_unstable_by(|a, b| key(*a).cmp(key(*b)).then(a.cmp(b)));
        let mut lost: MeteredVec<bool> =
            self.charged(|| table::with_capacity(meter, rows.len()))?;
        for _ in rows {
            self.charged(|| table::push(&mut lost, false))?;
        }
        for pair in order.as_slice().windows(2) {
            if let [a, b] = pair
                && key(*a) == key(*b)
                && let Some(flag) = lost
                    .as_mut_slice()
                    .get_mut(usize::try_from(*a).unwrap_or(usize::MAX))
            {
                *flag = true;
            }
        }
        Ok(lost)
    }
}

/// A crop's plan skip, from its rectangle and the sheet's mode alone
/// (section 5's crops): `empty_rect`, then Pillow's two refusals of the
/// crop, then its PNG encoder's of a crop wider than it takes in the mode
/// (`pillow_memory_error`, review round 2's C5), so no crop is made that
/// cannot be written.
fn crop_skip(e: &Element, mode: Mode) -> Option<Refusal> {
    if e.width <= 0 || e.height <= 0 {
        return Some(Refusal::EmptyRect);
    }
    let pixels = e.width.unsigned_abs().checked_mul(e.height.unsigned_abs());
    if pixels.is_none_or(|p| p > u128::from(MAX_CROP_PIXELS)) {
        return Some(Refusal::Bomb);
    }
    let min = i128::from(i32::MIN);
    let max = i128::from(i32::MAX);
    if e.x < min || e.y < min || e.x + e.width > max || e.y + e.height > max {
        return Some(Refusal::Overflow);
    }
    // Within the bomb's bound, the width is a `u32`.
    if too_wide(u32::try_from(e.width).unwrap_or(u32::MAX), mode) {
        return Some(Refusal::TooWide);
    }
    None
}

#[cfg(test)]
mod tests {
    use unityrs_core::RawStr;
    use unityrs_texture::Mode;

    use unityrs_texture::Format;

    use super::{Element, crop_skip, too_wide};

    fn rect(width: i128, height: i128) -> Element {
        Element {
            name: RawStr::from(b"s".as_slice()),
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn a_crop_wider_than_the_png_encoder_takes_is_refused_at_the_plan() {
        // Pillow 9.5.0's encoder takes `INT_MAX / bits - 7` pixels a row:
        // 67,108,856 in RGBA, 89,478,478 in RGB (review round 2's C5). The
        // check plans a one-row image, so nothing is allocated.
        let reason = |w, mode| crop_skip(&rect(w, 1), mode).map(|r| r.reason().detail);
        assert_eq!(reason(67_108_856, Mode::Rgba), None);
        assert_eq!(
            reason(67_108_857, Mode::Rgba),
            Some(Some("pillow_memory_error".to_owned()))
        );
        assert_eq!(reason(67_108_857, Mode::Rgb), None);
        assert_eq!(reason(89_478_478, Mode::Rgb), None);
        assert_eq!(
            reason(89_478_479, Mode::Rgb),
            Some(Some("pillow_memory_error".to_owned()))
        );
        // The bomb's check comes first.
        assert_eq!(
            reason(178_956_971, Mode::Rgba),
            Some(Some("decompression_bomb".to_owned()))
        );
    }

    #[test]
    fn a_block_texture_wider_than_the_png_encoder_takes_is_refused_from_its_plan() {
        // Review round 3's R1: a block format has no width cap at decode,
        // so a DXT1 sheet 67,108,860 x 4 (within `texture_pixels`) plans,
        // and its PNG would be refused. The decoder's plan reads only the
        // data's length, so nothing is allocated.
        let plan = |w: i32| {
            let data = usize::try_from(w).unwrap() / 4 * 8 + 8;
            unityrs_texture::plan(Format::Dxt1, data, w, 4).unwrap()
        };
        let wide = plan(67_108_860);
        assert!(too_wide(wide.width, wide.mode));
        let fits = plan(67_108_856);
        assert!(!too_wide(fits.width, fits.mode));
        // A raw format never gets here: its decoder's cap is the same.
        assert!(unityrs_texture::plan(Format::Rgba32, 1 << 30, 67_108_857, 1).is_err());
    }
}
