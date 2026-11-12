//! The export (4.4's export order): the sprite groups first
//! ([`super::sprites`]), then each sheet with the atlases that cut
//! it, the sheets in the order of each one's first atlas in plan order, the
//! sheet's texture output first, then each atlas's JSON and crops; then
//! every other texture in plan order. Every buffer an input sizes is
//! charged to the input's meter before it is allocated (E59), the export's
//! index tables among them, and each pixel buffer counted as export work
//! first (`docs/plans/batch.md` 2.6 step 10). Each output goes to the sink
//! as it is made, after the stop check, and a sheet's decode is dropped
//! after its group (step 6). Each core call is under the refusal probe on
//! its own (review round 2's S4).

use std::sync::Arc;

use md5::{Digest, Md5};
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost};
use unityrs_core::{Parsed, Value};
use unityrs_imageio::{Buffer, Charger, EncodeError, ImageError, visible_hash};
use unityrs_texture::{Format, Image, Mode, decode, plan};

use super::attempt::{Attempt, Site};
use super::classify::{self, Class, Reason};
use super::plan::{AtlasPlan, Planned, SpritePlan, TexturePlan, png_mode, too_wide};
use super::read::{Texture, field};
use super::seam::Seam;
use super::table;
use super::{Failure, MemberPath, Output, OutputKind, Picture, SkipOutput, resource};
use crate::pyjson::{self, Count, Sink, WriteError};

/// The imageio charger over the input's meter: each request at its
/// `heap_cost`.
struct Metered<'m>(&'m Arc<Meter>);

impl Charger for Metered<'_> {
    type Charge = Charge;
    type Refusal = ChargeError;

    fn charge(&mut self, _: Buffer, requests: &[usize]) -> Result<Charge, ChargeError> {
        let bytes = requests.iter().fold(0u64, |sum, r| {
            sum.saturating_add(heap_cost(u64::try_from(*r).unwrap_or(u64::MAX)))
        });
        self.0.charge(bytes)
    }
}

fn md5(bytes: &[u8]) -> [u8; 16] {
    let digest = Md5::digest(bytes);
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest);
    out
}

/// A made file: its bytes, their charge and, for a PNG, its picture.
pub(super) type File = (Vec<u8>, Charge, Option<Picture>);

/// The image of texture `t` of `file`, past section 3's answers 1 to 3
/// (`format`): its data (answer 4, [`resource::data`], each core call
/// probed), then its decode (answers 5 to 10, 12 and 13), its pixels
/// counted as export work before they are made; the decode's plan and the
/// decode each under the probe on their own. A `plain` candidate's image
/// wider than the PNG encoder takes is its output's skip from the plan
/// (answer 11), before the decode.
pub(super) fn decoded(
    seam: &Seam<'_>,
    file: &Arc<SerializedFile>,
    t: &Texture,
    format: Format,
    plain: bool,
) -> Result<Image, Class> {
    let _sample = super::read::Sample(file.meter(), seam.hooks);
    let data = resource::data(seam, file, t)?;
    let (width, height) = t.sides();
    let planned =
        seam.probe(|| plan(format, data.len(), width, height).map_err(|e| classify::decode(&e)))?;
    if plain && too_wide(planned.width, planned.mode) {
        return Err(unwritable());
    }
    seam.work(planned.output_bytes)?;
    seam.probe(|| {
        #[cfg(feature = "test-hooks")]
        let result = unityrs_texture::contain(std::panic::AssertUnwindSafe(|| {
            seam.hooks.decoder_fault();
            decode(format, &data, width, height, seam.meter)
        }));
        #[cfg(not(feature = "test-hooks"))]
        let result = decode(format, &data, width, height, seam.meter);
        result.map_err(|e| classify::decode(&e))
    })
}

/// An image's pixels, to be written as a PNG.
pub(super) struct Pixels<'p> {
    pub mode: Mode,
    pub width: u32,
    pub height: u32,
    pub data: &'p [u8],
}

/// The encoder's refusal of an image as its class: a width past Pillow's
/// encoder's is the bytes' (`decode_error`, `pillow_memory_error`, review
/// round 2's C5), a refused charge the meter's or the budget's, the
/// allocator's `host_memory`; any other is the cutter's own fault.
fn encoded(e: EncodeError<ChargeError>) -> Class {
    match e {
        EncodeError::Image(ImageError::TooWide { .. }) => {
            Class::Skip(Reason::decode_error("pillow_memory_error"))
        }
        EncodeError::Refused { refusal, .. } => classify::charge(&refusal),
        EncodeError::Alloc { .. } | EncodeError::ZlibMemory => table::host_memory(),
        _ => Class::Run(Reason::plain("internal")),
    }
}

/// A PNG of `p`, its bytes and charge, and its picture.
pub(super) fn png(p: &Pixels<'_>, meter: &Arc<Meter>) -> Result<(Vec<u8>, Charge, Picture), Class> {
    let image = unityrs_imageio::Image::new(p.width, p.height, png_mode(p.mode), p.data)
        .map_err(|e| encoded(EncodeError::Image(e)))?;
    let encoded = unityrs_imageio::encode(&image, &mut Metered(meter)).map_err(encoded)?;
    let picture = Picture {
        mode: png_mode(p.mode).name(),
        width: p.width,
        height: p.height,
        visible_hash: visible_hash(&image),
    };
    let (bytes, charge) = encoded.into_parts();
    Ok((bytes, charge, picture))
}

/// A crop of `sheet` (section 5's crops): `sprite`'s rectangle, zero past
/// the sheet's edge, in the sheet's mode, its pixels counted as export work
/// and charged (under the probe, on its own) before it is allocated.
fn crop(sheet: &Image, sprite: &SpritePlan, seam: &Seam<'_>) -> Result<(Vec<u8>, Charge), Class> {
    let bpp: u64 = match sheet.mode() {
        Mode::Rgb => 3,
        Mode::Rgba => 4,
    };
    let row = sprite.width.saturating_mul(bpp);
    let size = row.saturating_mul(sprite.height);
    seam.work(size)?;
    let charge = seam.probe(|| {
        seam.meter
            .charge(heap_cost(size))
            .map_err(|e| classify::charge(&e))
    })?;
    let n = usize::try_from(size).map_err(|_| table::host_memory())?;
    let mut out = Vec::new();
    out.try_reserve_exact(n).map_err(|_| table::host_memory())?;
    out.resize(n, 0);
    let (sw, sh) = (i128::from(sheet.width()), i128::from(sheet.height()));
    let (w, h) = (i128::from(sprite.width), i128::from(sprite.height));
    let x0 = sprite.x.max(0);
    let x1 = (sprite.x + w).min(sw);
    let bpp = i128::from(bpp);
    let src_row = sw * bpp;
    let dst_row = w * bpp;
    if x0 < x1 {
        for r in 0..h {
            let y = sprite.y + r;
            if y < 0 || y >= sh {
                continue;
            }
            let src = usize::try_from(y * src_row + x0 * bpp).unwrap_or(usize::MAX);
            let dst = usize::try_from(r * dst_row + (x0 - sprite.x) * bpp).unwrap_or(usize::MAX);
            let len = usize::try_from((x1 - x0) * bpp).unwrap_or(0);
            let from = sheet.pixels().get(src..src.saturating_add(len));
            let to = out.get_mut(dst..dst.saturating_add(len));
            if let (Some(from), Some(to)) = (from, to) {
                to.copy_from_slice(from);
            }
        }
    }
    Ok((out, charge))
}

/// A key of the JSON's object, after its first.
fn key(out: &mut impl Sink, k: &str) {
    out.put(b",\n  \"");
    out.put(k.as_bytes());
    out.put(b"\": ");
}

/// A value as read, or `null` when absent.
fn value_or_null(
    parsed: &Parsed,
    v: Option<&Value>,
    out: &mut impl Sink,
    meter: &Arc<Meter>,
) -> Result<(), WriteError> {
    if let Some(v) = v {
        return pyjson::value(parsed, v, 1, out, meter);
    }
    out.put(b"null");
    Ok(())
}

/// E24's JSON of `atlas` cut from a sheet of `width` by `height`: section
/// 5's object, written as `json.dumps(value, indent=2)` writes it, the
/// writer's stack charged to `meter`.
fn atlas_json(
    atlas: &AtlasPlan,
    width: u32,
    height: u32,
    out: &mut impl Sink,
    meter: &Arc<Meter>,
) -> Result<(), WriteError> {
    let parsed = &atlas.parsed;
    let root = parsed.value();
    let get = |name: &[u8]| field(parsed, root, name);
    let sprites = get(b"mSprites");
    let count = match sprites {
        Some(Value::List(v)) => v.len(),
        Some(Value::Prims(p)) => p.len(),
        _ => 0,
    };
    out.put(b"{\n  \"name\": ");
    pyjson::string(atlas.name.as_bytes(), out);
    key(out, "texture_width");
    pyjson::int(width.into(), out);
    key(out, "texture_height");
    pyjson::int(height.into(), out);
    key(out, "pixel_size");
    value_or_null(parsed, get(b"mPixelSize"), out, meter)?;
    key(out, "settings");
    value_or_null(parsed, get(b"mSettings"), out, meter)?;
    key(out, "sprite_count");
    pyjson::int(i128::try_from(count).unwrap_or(i128::MAX), out);
    key(out, "sprites");
    value_or_null(parsed, sprites, out, meter)?;
    out.put(b"\n}");
    Ok(())
}

/// An output's identity: its kind, key, unit key and member paths.
pub(super) struct Made<'p> {
    pub plan_index: u64,
    pub kind: OutputKind,
    pub key: &'p str,
    pub unit: Option<&'p str>,
    pub members: &'p [MemberPath],
}

/// Section 3's answer 11: a texture output whose PNG the encoder refuses
/// for its width.
fn unwritable() -> Class {
    Class::Skip(Reason::decode_error("pillow_memory_error"))
}

/// The bytes of `members`' paths: a kept path's, or a rejected one's text.
fn paths_cost(members: &[MemberPath]) -> u64 {
    members.iter().fold(0, |sum: u64, m| {
        let len = match &m.path {
            Ok(p) => p.len(),
            Err(r) => r.path.len(),
        };
        sum.saturating_add(table::string(len))
    })
}

impl Attempt<'_> {
    /// Makes the output of `file` and hands it to the sink: its key, unit
    /// key and member paths charged with its bytes before they are copied;
    /// a refused charge is the output's skip. Whether the sink took it
    /// (review round 2's C6: a unit is set only by an output handed over).
    pub(super) fn output(
        &mut self,
        made: &Made<'_>,
        site: &Site<'_>,
        file: (Vec<u8>, Charge, Option<Picture>),
    ) -> Result<bool, Failure> {
        let (bytes, mut charge, picture) = file;
        // The member paths' copy is its own allocation: its table at its
        // heap cost (review round 4's S1), then each path's bytes.
        let copy = match made.members.len() {
            0 => 0,
            n => heap_cost(table::slots::<MemberPath>(n)),
        };
        let cost = table::string(made.key.len())
            .saturating_add(made.unit.map_or(0, |u| table::string(u.len())))
            .saturating_add(copy)
            .saturating_add(paths_cost(made.members));
        let grown = self
            .seam
            .probe(|| charge.grow(cost).map_err(|e| table::refused(&e)));
        if let Err(c) = grown {
            self.record(c, site)?;
            return Ok(false);
        }
        let output = Output {
            plan_index: made.plan_index,
            key: made.key.to_owned(),
            kind: made.kind,
            unit: made.unit.map(str::to_owned),
            serialized_file: site.obj.file().name().clone(),
            path_id: site.obj.path_id(),
            sprite: site.sprite,
            md5: md5(&bytes),
            bytes,
            picture,
            members: made.members.to_vec(),
            _charge: Some(charge),
        };
        self.emit(output)?;
        Ok(true)
    }

    /// Texture `i`'s image: its data, then its decode (section 3's answers
    /// 4 to 13), its pixels counted as export work before it is made. The
    /// data's read probes each of its core calls ([`resource::data`]); the
    /// decode's plan and the decode are each under the probe on their own.
    /// For a `plain` candidate, whose image is wanted for its PNG alone, an
    /// image wider than the PNG encoder takes is that output's skip from the
    /// plan, before the decode (answer 11; only a block format's can be,
    /// the raw decoder's cap being the same). A sheet is decoded whatever
    /// its width: its atlases cut it, and the refusal is its texture
    /// output's alone (review round 4's A1, [`Self::export_sheet`]).
    fn image(&mut self, i: usize, plain: bool) -> Result<Image, Class> {
        let internal = || Class::Run(Reason::plain("internal"));
        let t = self.texture(i)?;
        let Ok(format) = self.selection(i, &t) else {
            return Err(internal());
        };
        let Some(file) = self.texs.as_slice().get(i).map(|x| x.obj.file().clone()) else {
            return Err(internal());
        };
        decoded(&self.seam, &file, &t, format, plain)
    }

    /// The export, in export order: each sheet's group in the order its
    /// first atlas was planned, then the other textures. The atlases are
    /// grouped by a permutation sorted by (sheet, plan index), each group a
    /// run of it, the runs taken in the order of their first atlas; a
    /// texture's plan is found by binary search (the textures are planned
    /// in object order). Each table is charged to `M_X` first.
    pub fn export(&mut self, planned: &Planned) -> Result<(), Failure> {
        self.exporting = true;
        self.export_sprites(&planned.sprites)?;
        let meter = self.seam.meter;
        let atlases = planned.atlases.as_slice();
        let textures = planned.textures.as_slice();
        let sheet = |j: u32| {
            atlases
                .get(usize::try_from(j).unwrap_or(usize::MAX))
                .map_or(usize::MAX, |a| a.sheet)
        };
        let internal = || Failure::Run(Reason::plain("internal"));
        let mut order: MeteredVec<u32> =
            self.charged(|| table::with_capacity(meter, atlases.len()))?;
        for j in 0..atlases.len() {
            let j = u32::try_from(j).map_err(|_| internal())?;
            self.charged(|| table::push(&mut order, j))?;
        }
        order
            .as_mut_slice()
            .sort_unstable_by(|a, b| sheet(*a).cmp(&sheet(*b)).then(a.cmp(b)));
        // Each run: its first atlas (the least plan index), its span.
        let mut runs: MeteredVec<(u32, usize, usize)> = MeteredVec::new(meter);
        let sorted = order.as_slice();
        let mut start = 0;
        while let Some(&first) = sorted.get(start) {
            let s = sheet(first);
            let len = sorted
                .get(start..)
                .unwrap_or_default()
                .iter()
                .take_while(|j| sheet(**j) == s)
                .count();
            self.charged(|| table::push(&mut runs, (first, start, start + len)))?;
            start += len;
        }
        runs.as_mut_slice().sort_unstable_by_key(|r| r.0);
        let texture = |tex: usize| {
            textures
                .binary_search_by_key(&tex, |t| t.tex)
                .ok()
                .and_then(|i| textures.get(i))
        };
        for &(first, from, to) in runs.as_slice() {
            self.seam.stop()?;
            let group = sorted.get(from..to).unwrap_or_default();
            let s = sheet(first);
            self.export_sheet(s, texture(s), group, atlases)?;
        }
        for tp in textures {
            let is_sheet = {
                let at = sorted.partition_point(|j| sheet(*j) < tp.tex);
                sorted.get(at).is_some_and(|j| sheet(*j) == tp.tex)
            };
            if is_sheet {
                continue;
            }
            self.seam.stop()?;
            let Some(obj) = self.texs.as_slice().get(tp.tex).map(|t| t.obj.clone()) else {
                continue;
            };
            let site = Site {
                obj: &obj,
                output: SkipOutput::Texture,
                sprite: None,
            };
            let made = Made {
                plan_index: tp.plan_index,
                kind: OutputKind::Texture,
                key: &tp.key,
                unit: None,
                members: tp.members.as_slice(),
            };
            match self
                .image(tp.tex, true)
                .and_then(|img| self.texture_png(&img))
            {
                Ok(file) => {
                    self.output(&made, &site, file)?;
                }
                Err(c) => self.record(c, &site)?,
            }
        }
        Ok(())
    }

    fn texture_png(&self, img: &Image) -> Result<File, Class> {
        let p = Pixels {
            mode: img.mode(),
            width: img.width(),
            height: img.height(),
            data: img.pixels(),
        };
        let meter = self.seam.meter;
        self.seam
            .probe(|| png(&p, meter).map(|(b, c, pic)| (b, c, Some(pic))))
    }

    fn export_sheet(
        &mut self,
        sheet: usize,
        texture: Option<&TexturePlan>,
        group: &[u32],
        atlases: &[AtlasPlan],
    ) -> Result<(), Failure> {
        let Some(obj) = self.texs.as_slice().get(sheet).map(|t| t.obj.clone()) else {
            return Ok(());
        };
        let tex_site = Site {
            obj: &obj,
            output: SkipOutput::Texture,
            sprite: None,
        };
        let atlas = |j: &u32| atlases.get(usize::try_from(*j).unwrap_or(usize::MAX));
        let img = match self.image(sheet, false) {
            Ok(img) => img,
            Err(Class::Skip(r)) => {
                if texture.is_some() {
                    self.skip(&tex_site, r.clone())?;
                }
                for a in group.iter().filter_map(atlas).filter(|a| a.has_outputs()) {
                    let site = Site {
                        obj: &a.obj,
                        output: SkipOutput::Atlas,
                        sprite: None,
                    };
                    self.skip(&site, r.clone())?;
                }
                return Ok(());
            }
            Err(c) => return Err(c.into()),
        };
        let mut unit = None;
        if let Some(tp) = texture {
            self.seam.stop()?;
            // Answer 11 is the texture output's own skip, before its PNG is
            // made; the atlases still cut the sheet (section 5).
            let file = if too_wide(img.width(), img.mode()) {
                Err(unwritable())
            } else {
                self.texture_png(&img)
            };
            match file {
                Ok(file) => {
                    let made = Made {
                        plan_index: tp.plan_index,
                        kind: OutputKind::Texture,
                        key: &tp.key,
                        unit: Some(&tp.key),
                        members: tp.members.as_slice(),
                    };
                    if self.output(&made, &tex_site, file)? {
                        unit = Some(tp.key.as_str());
                    }
                }
                Err(c) => self.record(c, &tex_site)?,
            }
        }
        for a in group.iter().filter_map(atlas) {
            self.export_atlas(a, &img, unit)?;
        }
        Ok(())
    }

    fn export_atlas(
        &mut self,
        a: &AtlasPlan,
        img: &Image,
        unit: Option<&str>,
    ) -> Result<(), Failure> {
        let mut unit = unit;
        if let Some(key) = &a.json_key {
            self.seam.stop()?;
            let site = Site {
                obj: &a.obj,
                output: SkipOutput::AtlasJson,
                sprite: None,
            };
            match self.json(a, img) {
                Ok(Some(file)) => {
                    let u = unit.unwrap_or(key);
                    let made = Made {
                        plan_index: a.json_plan_index,
                        kind: OutputKind::AtlasJson,
                        key,
                        unit: Some(u),
                        members: a.json_members.as_slice(),
                    };
                    if self.output(&made, &site, file)? {
                        unit = Some(u);
                    }
                }
                Ok(None) => self.skip(&site, Reason::plain("json_unwritable"))?,
                Err(c) => self.record(c, &site)?,
            }
        }
        for s in a.sprites.as_slice() {
            self.seam.stop()?;
            let site = Site {
                obj: &a.obj,
                output: SkipOutput::AtlasSprite,
                sprite: Some(s.index),
            };
            let seam = self.seam;
            let made = crop(img, s, &seam).and_then(|(pixels, _held)| {
                let p = Pixels {
                    mode: img.mode(),
                    width: u32::try_from(s.width).unwrap_or(u32::MAX),
                    height: u32::try_from(s.height).unwrap_or(u32::MAX),
                    data: &pixels,
                };
                seam.probe(|| png(&p, seam.meter))
            });
            match made {
                Ok((bytes, charge, pic)) => {
                    let made = Made {
                        plan_index: s.plan_index,
                        kind: OutputKind::AtlasSprite,
                        key: &s.key,
                        unit,
                        members: s.members.as_slice(),
                    };
                    self.output(&made, &site, (bytes, charge, Some(pic)))?;
                }
                Err(c) => self.record(c, &site)?,
            }
        }
        Ok(())
    }

    /// The atlas's JSON, charged before it is written; none when a value
    /// cannot be written (E25). The measure, the charge and the write are
    /// each under the probe on their own; the writer's stack is charged to
    /// `M_X` as it grows.
    fn json(&self, a: &AtlasPlan, img: &Image) -> Result<Option<File>, Class> {
        let seam = self.seam;
        let meter = seam.meter;
        let written = |r: Result<(), WriteError>| match r {
            Ok(()) => Ok(true),
            Err(WriteError::Unwritable(_)) => Ok(false),
            Err(WriteError::Charge(e)) => Err(table::refused(&e)),
        };
        let mut count = Count::default();
        if !seam.probe(|| written(atlas_json(a, img.width(), img.height(), &mut count, meter)))? {
            return Ok(None);
        }
        let charge = seam.probe(|| {
            meter
                .charge(heap_cost(count.0))
                .map_err(|e| table::refused(&e))
        })?;
        let n = usize::try_from(count.0).map_err(|_| table::host_memory())?;
        let mut out = Vec::new();
        out.try_reserve_exact(n).map_err(|_| table::host_memory())?;
        if !seam.probe(|| written(atlas_json(a, img.width(), img.height(), &mut out, meter)))? {
            return Ok(None);
        }
        Ok(Some((out, charge, None)))
    }
}
