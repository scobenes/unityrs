// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's get_image_from_sprite (the sheet decoded, its alpha texture merged in, then the Sprite rendered), and export/Texture2DConverter.py's parse_image_data for a side of 0. See THIRD_PARTY_LICENSES.
//! The sprites kind's export (4.4's export order: the sprite groups come
//! first): each group in the order of its first Sprite, its Sprites in
//! plan order. A group's sheet is dereferenced again from the record the
//! plan kept of it ([`GroupSheet`]: no Sprite's render data is read again,
//! B3's review round 1, R9), read, and decoded once (section 3's answers 4
//! to 10, 12 and 13, [`decoded`]; a side of 0 decodes to UnityPy's 0 x 0
//! `RGBA` image), its alpha texture with it and merged in
//! ([`Sheet::merged`]: sizes that differ are `render`, before any count;
//! else the merged pixels counted as export work first, and both decoded
//! images dropped once it is made); a failure there is the skip of every
//! Sprite of the group, with the sheet's reason. Then each Sprite is
//! rendered ([`render`]; a tight one's mesh read and processed only where
//! the render asks for it), its image asked answer 11 (an image wider than
//! Pillow's encoder takes, `pillow_memory_error`), counted as export work,
//! and its PNG made and handed to the sink. A sheet is dropped after its
//! group (`docs/plans/batch.md` 2.6 step 6).

use std::cell::{OnceCell, RefCell};

use unityrs_core::Parsed;
use unityrs_core::limits::MeteredVec;
use unityrs_meshdecode::{MeshError, MeshHandler, Source, Version};
use unityrs_texture::sprite::{Sheet, render};
use unityrs_texture::{Format, Image, Mode};

use super::super::attempt::{Attempt, Site, UNKNOWN_PLATFORM};
use super::super::classify::{self, Class, Reason};
use super::super::export::{Made, Pixels, decoded, png};
use super::super::plan::too_wide;
use super::super::read::{self, top_bundle};
use super::super::table;
use super::super::{Failure, OutputKind, SkipOutput};
use super::plan::sheet_answers;
use super::{GroupSheet, SpritePlan, Sprites};

/// Place `i` of `v`.
fn slot(v: &mut MeteredVec<u32>, i: usize) -> Result<&mut u32, Failure> {
    v.as_mut_slice()
        .get_mut(i)
        .ok_or_else(|| Failure::Run(Reason::plain("internal")))
}

/// Bytes a pixel of `mode` takes.
fn bpp(mode: Mode) -> u64 {
    match mode {
        Mode::L => 1,
        Mode::Rgb => 3,
        Mode::Rgba => 4,
    }
}

impl Attempt<'_> {
    /// The sprite groups, in the order of each one's first Sprite (their
    /// numbers are that order, as `join` gives them), each group's Sprites
    /// in plan order: placed by a counting sort of their groups, linear and
    /// counted (B3's review round 2, S4): two tables charged to `M_X`
    /// first, and one unit of decode work for each group and twice each
    /// Sprite, counted before (a refusal fails the input: no object is its
    /// site).
    pub(in super::super) fn export_sprites(&mut self, planned: &Sprites) -> Result<(), Failure> {
        let meter = self.seam.meter;
        let sprites = planned.planned.as_slice();
        let groups = planned.groups.as_slice();
        if sprites.is_empty() {
            return Ok(());
        }
        let internal = || Failure::Run(Reason::plain("internal"));
        let units = sprites.len().saturating_mul(2).saturating_add(groups.len());
        meter
            .count_decode(u64::try_from(units).unwrap_or(u64::MAX))
            .map_err(|e| Failure::from(classify::charge(&e)))?;
        // `ends[g]`: where group `g`'s run ends, once every Sprite is
        // placed (its start is `ends[g - 1]`, or 0).
        let mut ends: MeteredVec<u32> =
            self.charged(|| table::with_capacity(meter, groups.len() + 1))?;
        for _ in 0..=groups.len() {
            self.charged(|| table::push(&mut ends, 0))?;
        }
        for s in sprites {
            let g = usize::try_from(s.group).map_err(|_| internal())?;
            *slot(&mut ends, g + 1)? += 1;
        }
        for g in 1..=groups.len() {
            let before = *slot(&mut ends, g - 1)?;
            *slot(&mut ends, g)? += before;
        }
        let mut order: MeteredVec<u32> =
            self.charged(|| table::with_capacity(meter, sprites.len()))?;
        for _ in 0..sprites.len() {
            self.charged(|| table::push(&mut order, 0))?;
        }
        for (j, s) in sprites.iter().enumerate() {
            let g = usize::try_from(s.group).map_err(|_| internal())?;
            let next = slot(&mut ends, g)?;
            let at = usize::try_from(*next).map_err(|_| internal())?;
            *next += 1;
            *slot(&mut order, at)? = u32::try_from(j).map_err(|_| internal())?;
        }
        let order = order.as_slice();
        let ends = ends.as_slice();
        for (g, sheet) in groups.iter().enumerate() {
            self.seam.stop()?;
            let start = if g == 0 {
                0
            } else {
                ends.get(g - 1).copied().ok_or_else(internal)?
            };
            let end = ends.get(g).copied().ok_or_else(internal)?;
            let run = usize::try_from(start)
                .ok()
                .zip(usize::try_from(end).ok())
                .and_then(|(a, b)| order.get(a..b))
                .ok_or_else(internal)?;
            let at = |j: &u32| sprites.get(usize::try_from(*j).unwrap_or(usize::MAX));
            for s in run.iter().filter_map(at) {
                self.seam
                    .hooks
                    .sprite_group(g, s.obj.file().name(), s.obj.path_id());
            }
            self.export_group(sheet, run, sprites)?;
        }
        Ok(())
    }

    /// One group: its sheet, then each Sprite of `run` (indices into
    /// `sprites`, in plan order).
    fn export_group(
        &mut self,
        record: &GroupSheet,
        run: &[u32],
        sprites: &[SpritePlan],
    ) -> Result<(), Failure> {
        let (texture, alpha) = match self.group_sheet(record) {
            Ok(images) => images,
            Err(c) => return self.skip_group(c, run, sprites),
        };
        let Some(alpha) = alpha else {
            // A sheet the renderer refuses (an `L` one, BC4's, which a
            // version 2 spec may export: `SpriteError::UnverifiedMode`) is
            // each of its Sprites' skip, as a merge's refusal is, never the
            // input's failure: the input's other outputs go on.
            return match Sheet::new(&texture, None, self.seam.meter) {
                Ok(sheet) => self.export_run(&sheet, run, sprites),
                Err(e) => self.skip_group(classify::render(&e), run, sprites),
            };
        };
        // The merged sheet is the texture's size, RGBA: counted once the
        // sizes are known to match (`Sheet::merged` refuses others before
        // it allocates).
        let same = (texture.width(), texture.height()) == (alpha.width(), alpha.height());
        if same {
            let pixels = u64::from(texture.width())
                .saturating_mul(u64::from(texture.height()))
                .saturating_mul(4);
            if let Err(c) = self.seam.work(pixels) {
                return Err(c.into());
            }
        }
        let seam = self.seam;
        let merged = seam.probe(|| {
            Sheet::merged(&texture, &alpha, seam.meter).map_err(|e| classify::render(&e))
        });
        drop(alpha);
        drop(texture);
        match merged {
            Ok(sheet) => self.export_run(&sheet, run, sprites),
            Err(c) => self.skip_group(c, run, sprites),
        }
    }

    /// Each Sprite of `run`, from `sheet`.
    fn export_run(
        &mut self,
        sheet: &Sheet<'_>,
        run: &[u32],
        sprites: &[SpritePlan],
    ) -> Result<(), Failure> {
        let at = |j: &u32| sprites.get(usize::try_from(*j).unwrap_or(usize::MAX));
        for s in run.iter().filter_map(at) {
            self.seam.stop()?;
            self.export_sprite(sheet, s)?;
        }
        Ok(())
    }

    /// A group's failure before any render: each of its Sprites' skip, with
    /// the sheet's reason; a class that is no object's skip ends the cut.
    fn skip_group(&mut self, c: Class, run: &[u32], sprites: &[SpritePlan]) -> Result<(), Failure> {
        let Class::Skip(reason) = c else {
            return Err(c.into());
        };
        for s in run
            .iter()
            .filter_map(|j| sprites.get(usize::try_from(*j).unwrap_or(usize::MAX)))
        {
            let site = Site {
                obj: &s.obj,
                output: SkipOutput::Sprite,
                sprite: None,
            };
            self.skip(&site, reason.clone())?;
        }
        Ok(())
    }

    /// The group's sheet, decoded: its texture, then its alpha texture,
    /// each dereferenced from the group's record (from its atlas's file,
    /// where it has one) and held, while it is read and decoded, with its
    /// top-level bundle.
    fn group_sheet(&mut self, record: &GroupSheet) -> Result<(Image, Option<Image>), Class> {
        let seam = self.seam;
        let (holder, _held) = match record.atlas {
            Some(p) => {
                let atlas = read::deref(&seam, &record.from, p)?;
                let held = top_bundle(atlas.file());
                (atlas.file().clone(), held)
            }
            None => (record.from.clone(), None),
        };
        let texture = read::deref(&seam, &holder, record.texture)?;
        let held = top_bundle(texture.file());
        let image = self.decode_texture(&texture)?;
        drop(texture);
        drop(held);
        let alpha = match record.alpha {
            Some(p) => {
                let alpha = read::deref(&seam, &holder, p)?;
                let _held = top_bundle(alpha.file());
                Some(self.decode_texture(&alpha)?)
            }
            None => None,
        };
        Ok((image, alpha))
    }

    /// A texture's image: its object read and the plan's answers again
    /// (which held), then its data and decode; a side of 0 decodes to the
    /// 0 x 0 `RGBA` image (its data read first, as UnityPy's
    /// `get_image_data` reads it before `parse_image_data`).
    fn decode_texture(&self, obj: &unityrs_core::ObjectRef) -> Result<Image, Class> {
        let seam = self.seam;
        let t = seam.probe(|| read::texture(obj))?;
        sheet_answers(&t, obj, self.options.formats)?;
        let format = if t.width == 0 || t.height == 0 {
            // `plan` gives the 0 x 0 image for a side of 0 whatever the
            // format; any of the five stands in.
            Format::Rgba32
        } else {
            let platform = obj.file().target_platform().unwrap_or(UNKNOWN_PLATFORM);
            read::formats(&t, platform, self.options.formats)?
        };
        decoded(&seam, obj.file(), &t, format, false)
    }

    /// One Sprite: its render, its image's export work, its PNG.
    fn export_sprite(&mut self, sheet: &Sheet<'_>, s: &SpritePlan) -> Result<(), Failure> {
        let site = Site {
            obj: &s.obj,
            output: SkipOutput::Sprite,
            sprite: None,
        };
        let image = match self.render_sprite(sheet, s) {
            Ok(i) => i,
            Err(c) => return self.record(c, &site),
        };
        // Pillow writes no PNG of an image with a side of 0 (`ImageError::
        // ZeroSide`): the Sprite's `empty_image`. Then answer 11, before the
        // count, as a texture's is (`decoded`).
        if image.width() == 0 || image.height() == 0 {
            return self.skip(&site, Reason::plain("empty_image"));
        }
        if too_wide(image.width(), image.mode()) {
            return self.skip(&site, Reason::decode_error("pillow_memory_error"));
        }
        let pixels = u64::from(image.width())
            .saturating_mul(u64::from(image.height()))
            .saturating_mul(bpp(image.mode()));
        if let Err(c) = self.seam.work(pixels) {
            return self.record(c, &site);
        }
        let p = Pixels {
            mode: image.mode(),
            width: image.width(),
            height: image.height(),
            data: image.pixels(),
        };
        let seam = self.seam;
        let file = seam.probe(|| png(&p, seam.meter));
        drop(image);
        match file {
            Ok((bytes, charge, picture)) => {
                let made = Made {
                    plan_index: s.plan_index,
                    kind: OutputKind::Sprite,
                    key: &s.key,
                    unit: None,
                    members: s.members.as_slice(),
                };
                self.output(&made, &site, (bytes, charge, Some(picture)))?;
                Ok(())
            }
            Err(c) => self.record(c, &site),
        }
    }

    /// The Sprite's image (E17's [`render`]) from its group's sheet and its
    /// render data; a tight Sprite's mesh is its own `m_RD`'s, read again
    /// (checked) and processed at its file's version only when the render
    /// asks for it. A failure of that read, or a file with no version, is
    /// the Sprite's, before the render's own.
    fn render_sprite(&self, sheet: &Sheet<'_>, s: &SpritePlan) -> Result<Image, Class> {
        let seam = self.seam;
        let meter = seam.meter;
        let cell: OnceCell<Parsed> = OnceCell::new();
        let failed: RefCell<Option<Class>> = RefCell::new(None);
        let fail = |c: Class| {
            *failed.borrow_mut() = Some(c);
            MeshError::NoAttribute("m_RD")
        };
        let mesh = || {
            if cell.get().is_none() {
                let read = seam.probe(|| {
                    s.obj
                        .read_typetree(None, true)
                        .map_err(|e| classify::parse(&e))
                });
                match read {
                    Ok(p) => {
                        let _ = cell.set(p);
                    }
                    Err(c) => return Err(fail(c)),
                }
            }
            let Some(version) = s.obj.file().version().map(|v| Version::of(&v.version)) else {
                return Err(fail(Class::Skip(Reason::decode_error("version"))));
            };
            let Some(parsed) = cell.get() else {
                return Err(fail(Class::Run(Reason::plain("internal"))));
            };
            MeshHandler::new(Source::sprite(parsed), version).process(None, meter)
        };
        let image =
            seam.probe(|| render(sheet, &s.data, mesh, meter).map_err(|e| classify::render(&e)));
        if let Some(c) = failed.into_inner() {
            return Err(c);
        }
        image
    }
}
