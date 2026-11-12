// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py. See THIRD_PARTY_LICENSES.
//! The `sprites` kind's scenes (`docs/BATCH_SPEC.md` 1.5's `outputs.sprites`,
//! `docs/plans/extract.md` E17): Unity `Sprite`s (class 213) beside the
//! `Texture2D` sheets they render from, in bundles whose views join them,
//! and [`sprite_scenes`], which lists them by name for
//! `tools/parity/sprites_scenes.py` (the reference `tools/parity/sprites_kind.py`
//! is recorded on them by `tools/fixtures/sprites.py`).
//!
//! A Sprite is a [`super::V`] of the 2022.3 layout the testkit's files
//! carry, with the fields `tools/fixtures/sprite_build.py`'s Sprite tree
//! holds, in its order, from the same sources (nothing from a TPK or a
//! dump, `docs/plans/core.md` section 9, Q2): of the fields UnityPy 1.25.2's
//! MIT class definitions (`classes/generated.py`) declare, those without a
//! default and those `get_image_from_sprite` reads, each node's type the
//! one a source names, `classes/generated.py` or the MIT readers of
//! AssetStudio's release v0.16.47 (format facts only: no AssetStudio code is
//! ported or kept here). `m_RD` is a `SpriteRenderData` of the layout from
//! 5.6 (`m_SubMeshes`, `m_IndexBuffer`, `m_VertexData`), its `VertexData`
//! the one from 2018 (no `m_CurrentChannels`, 14 `ChannelInfo`s), with
//! `alphaTexture` last; then `m_SpriteAtlas`, `m_RenderDataKey` (a `pair`
//! of a `GUID`, four `unsigned int`s `data[0]` to `data[3]`, and an
//! `SInt64`) and `m_AtlasTags` (a vector of `string`).
//!
//! A vertex is three `float`s in channel 0 (`VertexFormat` 0, `float`, from
//! 2019), the stream padded with zeros to a multiple of 16 bytes as
//! `sprite_build.py` pads it, and the indices `UInt16`s, stored as the
//! `UInt8`s of `m_IndexBuffer`. No scene has a UV channel: a tight Sprite
//! here is masked by its triangles (`mask_sprite`), never drawn through an
//! affine transform, whose last bits depend on the host's BLAS.

use super::scenes::{CutOptions, one, scene};
use super::{AtlasScene, BuildError, Object, V, file, format, pair, pptr, s, sheet, st, texture};
use crate::ngui::SceneBundle;

/// `settingsRaw` of a Sprite packed by its rectangle (`packingMode` 1),
/// not rotated.
pub const RECT: u32 = 2;
/// `settingsRaw` of a tight Sprite (`packingMode` 0), its mesh's polygon.
pub const TIGHT: u32 = 0;

/// `settingsRaw` of a packed rectangle Sprite turned by `rotation`
/// (`SpritePackingRotation`, bits 2 to 5).
#[must_use]
pub fn packed(rotation: u32) -> u32 {
    1 | RECT | rotation << 2
}

/// A Sprite's mesh: its vertices (each in channel 0), its submeshes
/// (`firstByte`, `indexCount`, topology 0) and its indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// The vertices' positions.
    pub vertices: Vec<[f32; 3]>,
    /// The submeshes: `firstByte`, `indexCount`.
    pub submeshes: Vec<(u32, u32)>,
    /// The indices.
    pub indices: Vec<u16>,
}

impl Mesh {
    /// One triangle (0, 0), (`side`, 0), (0, `side`), as one submesh.
    #[must_use]
    pub fn triangle(side: f32) -> Self {
        Self {
            vertices: vec![[0.0, 0.0, 0.0], [side, 0.0, 0.0], [0.0, side, 0.0]],
            submeshes: vec![(0, 3)],
            indices: vec![0, 1, 2],
        }
    }
}

/// A Sprite: its name, `textureRect` and `settingsRaw`,
/// `m_PixelsToUnits`, its texture's and alpha texture's pointers and its
/// mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteSpec {
    /// `m_Name`.
    pub name: Vec<u8>,
    /// `m_RD.textureRect`: `x`, `y`, `width`, `height`.
    pub rect: [f32; 4],
    /// `m_RD.settingsRaw`.
    pub settings: u32,
    /// `m_PixelsToUnits`.
    pub ppu: f32,
    /// `m_RD.texture`.
    pub texture: (i32, i64),
    /// `m_RD.alphaTexture` (null: none).
    pub alpha: (i32, i64),
    /// `m_RD`'s mesh.
    pub mesh: Mesh,
    /// `m_SpriteAtlas` (null: none).
    pub atlas: (i32, i64),
    /// `m_RenderDataKey`: its `GUID`'s words and its id.
    pub key: ([u32; 4], i64),
    /// `m_AtlasTags`.
    pub tags: Vec<Vec<u8>>,
}

impl SpriteSpec {
    /// A rectangle Sprite `name` of `rect` on the texture at `texture`, with
    /// no alpha texture and no mesh, `m_PixelsToUnits` 1.
    #[must_use]
    pub fn rect(name: &str, rect: [f32; 4], texture: (i32, i64)) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            rect,
            settings: RECT,
            ppu: 1.0,
            texture,
            alpha: (0, 0),
            mesh: Mesh::default(),
            atlas: (0, 0),
            key: ([1, 2, 3, 4], 7),
            tags: Vec::new(),
        }
    }

    /// A tight Sprite `name` of `rect` on `texture`, masked by `mesh`'s
    /// triangles at `m_PixelsToUnits` 16.
    #[must_use]
    pub fn tight(name: &str, rect: [f32; 4], texture: (i32, i64), mesh: Mesh) -> Self {
        Self {
            settings: TIGHT,
            ppu: 16.0,
            mesh,
            ..Self::rect(name, rect, texture)
        }
    }

    /// The same Sprite with `settingsRaw` `settings`.
    #[must_use]
    pub fn settings(mut self, settings: u32) -> Self {
        self.settings = settings;
        self
    }

    /// The same Sprite with its alpha texture at `alpha`.
    #[must_use]
    pub fn alpha(mut self, alpha: (i32, i64)) -> Self {
        self.alpha = alpha;
        self
    }

    /// The same Sprite with `m_SpriteAtlas` at `atlas`.
    #[must_use]
    pub fn in_atlas(mut self, atlas: (i32, i64)) -> Self {
        self.atlas = atlas;
        self
    }

    /// The same Sprite with `m_AtlasTags` `tags`.
    #[must_use]
    pub fn tagged(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|t| t.as_bytes().to_vec()).collect();
        self
    }

    /// The same Sprite with `m_RenderDataKey` `key`.
    #[must_use]
    pub fn keyed(mut self, key: ([u32; 4], i64)) -> Self {
        self.key = key;
        self
    }

    /// The Sprite's value.
    #[must_use]
    pub fn value(&self) -> V {
        sprite_value(self)
    }

    /// The Sprite as an object at `path_id`.
    #[must_use]
    pub fn object(&self, path_id: i64) -> Object {
        Object::new(path_id, 213, sprite_value(self))
    }
}

fn floats(ty: &str, names: &[&str], values: &[f32]) -> V {
    V::Struct(
        ty.to_owned(),
        names
            .iter()
            .zip(values)
            .map(|(n, v)| ((*n).to_owned(), V::Float(*v)))
            .collect(),
    )
}

pub(super) fn floats_of(ty: &str, names: &[&str], values: &[f32]) -> V {
    floats(ty, names, values)
}

pub(super) fn rectf(r: [f32; 4]) -> V {
    floats("Rectf", &["x", "y", "width", "height"], &r)
}

fn bytes_vector(data: &[u8]) -> V {
    super::vector(V::UInt8(0), data.iter().map(|b| V::UInt8(*b)).collect())
}

/// `m_SubMeshes`: each `SubMesh` with `firstByte`, `indexCount`, topology
/// 0, `firstVertex` 0, `vertexCount` all the vertices and a zero
/// `localAABB`.
fn submeshes(mesh: &Mesh) -> V {
    let count = u32::try_from(mesh.vertices.len()).unwrap_or(u32::MAX);
    let aabb = || {
        st(
            "AABB",
            vec![
                ("m_Center", floats("Vector3f", &["x", "y", "z"], &[0.0; 3])),
                ("m_Extent", floats("Vector3f", &["x", "y", "z"], &[0.0; 3])),
            ],
        )
    };
    let sub = |first: u32, n: u32| {
        st(
            "SubMesh",
            vec![
                ("firstByte", V::UInt(first)),
                ("indexCount", V::UInt(n)),
                ("topology", V::Int(0)),
                ("firstVertex", V::UInt(0)),
                ("vertexCount", V::UInt(count)),
                ("localAABB", aabb()),
            ],
        )
    };
    super::vector(
        sub(0, 0),
        mesh.submeshes.iter().map(|(f, n)| sub(*f, *n)).collect(),
    )
}

/// `m_VertexData`: the vertex count, 14 channels (channel 0 three
/// `float`s, the others empty) and the stream.
fn vertex_data(mesh: &Mesh) -> V {
    let channel = |dimension: u8| {
        st(
            "ChannelInfo",
            vec![
                ("stream", V::UInt8(0)),
                ("offset", V::UInt8(0)),
                ("format", V::UInt8(0)),
                ("dimension", V::UInt8(dimension)),
            ],
        )
    };
    let channels = (0..14)
        .map(|i| channel(if i == 0 { 3 } else { 0 }))
        .collect();
    let mut data: Vec<u8> = mesh
        .vertices
        .iter()
        .flatten()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    data.resize(data.len().next_multiple_of(16), 0);
    st(
        "VertexData",
        vec![
            (
                "m_VertexCount",
                V::UInt(u32::try_from(mesh.vertices.len()).unwrap_or(u32::MAX)),
            ),
            ("m_Channels", super::vector(channel(0), channels)),
            ("m_DataSize", V::Bytes(data)),
        ],
    )
}

fn sprite_value(spec: &SpriteSpec) -> V {
    let indices: Vec<u8> = spec
        .mesh
        .indices
        .iter()
        .flat_map(|i| i.to_le_bytes())
        .collect();
    let rd = st(
        "SpriteRenderData",
        vec![
            ("texture", pptr("Texture2D", spec.texture)),
            ("m_SubMeshes", submeshes(&spec.mesh)),
            ("m_IndexBuffer", bytes_vector(&indices)),
            ("m_VertexData", vertex_data(&spec.mesh)),
            ("textureRect", rectf(spec.rect)),
            (
                "textureRectOffset",
                floats("Vector2f", &["x", "y"], &[0.0; 2]),
            ),
            ("settingsRaw", V::UInt(spec.settings)),
            ("alphaTexture", pptr("Texture2D", spec.alpha)),
        ],
    );
    let (words, id) = spec.key;
    st(
        "Sprite",
        vec![
            ("m_Name", V::Str(spec.name.clone())),
            ("m_Rect", rectf(spec.rect)),
            ("m_Offset", floats("Vector2f", &["x", "y"], &[0.0; 2])),
            ("m_PixelsToUnits", V::Float(spec.ppu)),
            ("m_Extrude", V::UInt(1)),
            ("m_RD", rd),
            ("m_SpriteAtlas", pptr("SpriteAtlas", spec.atlas)),
            ("m_RenderDataKey", key(words, id)),
            (
                "m_AtlasTags",
                super::vector(s(""), spec.tags.iter().map(|t| V::Str(t.clone())).collect()),
            ),
        ],
    )
}

/// A `pair<GUID, SInt64>`: a `GUID` of `words` (`data[0]` to `data[3]`)
/// and `id`.
pub(super) fn key(words: [u32; 4], id: i64) -> V {
    let names = ["data[0]", "data[1]", "data[2]", "data[3]"];
    let guid = st(
        "GUID",
        names
            .iter()
            .zip(words)
            .map(|(n, w)| (*n, V::UInt(w)))
            .collect(),
    );
    pair(guid, V::Int64(id))
}

/// An RGBA32 sheet `name` at `path_id`, `w` x `h`, of base `base`.
pub(super) fn rgba(path_id: i64, name: &str, base: u8, w: u8, h: u8) -> Object {
    let size = (i32::from(w), i32::from(h));
    texture(
        path_id,
        name.as_bytes(),
        format::RGBA32,
        size,
        sheet(base, w, h, false),
    )
}

/// An RGB24 sheet.
fn rgb(path_id: i64, name: &str, base: u8, w: u8, h: u8) -> Object {
    let size = (i32::from(w), i32::from(h));
    texture(
        path_id,
        name.as_bytes(),
        format::RGB24,
        size,
        sheet(base, w, h, true),
    )
}

/// The options of a scene exporting Sprites under `key`.
pub(super) fn sprites(key: &str) -> CutOptions {
    CutOptions {
        sprites_key: Some(key.to_owned()),
        ..CutOptions::default()
    }
}

/// One input `x.bundle` of the CAB `cab`, Sprites exported under `key`.
pub(super) fn alone(cab: &str, objects: &[Object], key: &str) -> Result<AtlasScene, BuildError> {
    let data = file(&[], objects, true)?;
    Ok(scene(
        vec![one("x.bundle", &[(cab, data)], &[])],
        &["x.bundle"],
        sprites(key),
    ))
}

/// Sprites on sheets of their own file, in two groups: an RGBA32 sheet
/// (`a`, `c` turned a quarter, `mask`, tight, and `flip`, mirrored) and an
/// RGB24 one (`b`, and `mask_rgb`, whose mask becomes its alpha), the
/// groups interleaved in object order, so the export runs `a`, `c`, `mask`,
/// `flip`, then `b`, `mask_rgb`.
pub fn own_sheets() -> Result<AtlasScene, BuildError> {
    let tri = Mesh::triangle(0.25);
    alone(
        "CAB-s1",
        &[
            rgba(1, "sheet", 1, 8, 8),
            SpriteSpec::rect("a", [1.0, 1.0, 4.0, 3.0], (0, 1)).object(2),
            rgb(3, "other", 0x40, 4, 4),
            SpriteSpec::rect("b", [0.0, 0.0, 4.0, 4.0], (0, 3)).object(4),
            SpriteSpec::rect("c", [2.0, 1.0, 5.0, 3.0], (0, 1))
                .settings(packed(4))
                .object(5),
            SpriteSpec::tight("mask", [0.0, 0.0, 4.0, 4.0], (0, 1), tri.clone()).object(6),
            SpriteSpec::tight("mask_rgb", [0.0, 0.0, 4.0, 4.0], (0, 3), tri).object(7),
            SpriteSpec::rect("flip", [1.0, 2.0, 3.0, 2.0], (0, 1))
                .settings(packed(1))
                .object(8),
        ],
        "{type}/{name}.png",
    )
}

/// Sheets in a dependency and pointers that find none: `far` and `far2`
/// share the dependency's sheet; `own` has its own texture and the
/// dependency's RGB24 alpha texture; `gone` names a CAB outside the view
/// (`outside_view`); `null`, `missing` and `past` a null pointer, a missing
/// object and an `m_FileID` past the externals (`bad_pointer`).
pub fn dependency_sheets() -> Result<AtlasScene, BuildError> {
    let dep = file(
        &[],
        &[
            rgba(10, "dep", 0x20, 4, 4),
            rgb(11, "dep_alpha", 0x70, 2, 2),
        ],
        true,
    )?;
    let cab = file(
        &["archive:/CAB-d1/CAB-d1", "archive:/CAB-gone/CAB-gone"],
        &[
            SpriteSpec::rect("far", [0.0, 0.0, 4.0, 4.0], (1, 10)).object(1),
            SpriteSpec::rect("far2", [1.0, 1.0, 2.0, 2.0], (1, 10)).object(2),
            SpriteSpec::rect("gone", [0.0, 0.0, 1.0, 1.0], (2, 5)).object(3),
            SpriteSpec::rect("null", [0.0, 0.0, 1.0, 1.0], (0, 0)).object(4),
            SpriteSpec::rect("missing", [0.0, 0.0, 1.0, 1.0], (0, 99)).object(5),
            SpriteSpec::rect("past", [0.0, 0.0, 1.0, 1.0], (3, 1)).object(6),
            rgba(7, "own", 0x30, 2, 2),
            SpriteSpec::rect("own", [0.0, 0.0, 2.0, 2.0], (0, 7))
                .alpha((1, 11))
                .object(8),
        ],
        true,
    )?;
    Ok(scene(
        vec![
            one("d.bundle", &[("CAB-d1", dep)], &[]),
            one("x.bundle", &[("CAB-s2", cab)], &["d.bundle"]),
        ],
        &["x.bundle"],
        sprites("{safe_input}/{name}_{path_id}.png"),
    ))
}

/// Alpha textures: an RGB24 sheet with an RGBA32 alpha texture of its size
/// (`merged`, `merged2` and the tight `alpha_mask`, one group), the same
/// sheet alone (`plain`, a group of its own), and with an alpha texture of
/// another size (`mismatch`, `Image.merge`'s `ValueError`: `render`).
pub fn alpha_textures() -> Result<AtlasScene, BuildError> {
    alone(
        "CAB-s3",
        &[
            rgb(1, "rgb", 0x10, 8, 8),
            rgba(2, "alpha", 0x90, 8, 8),
            rgba(3, "small_alpha", 0x50, 8, 4),
            SpriteSpec::rect("merged", [1.0, 1.0, 4.0, 4.0], (0, 1))
                .alpha((0, 2))
                .object(4),
            SpriteSpec::rect("plain", [1.0, 1.0, 4.0, 4.0], (0, 1)).object(5),
            SpriteSpec::rect("merged2", [0.0, 0.0, 8.0, 8.0], (0, 1))
                .alpha((0, 2))
                .object(6),
            SpriteSpec::rect("mismatch", [0.0, 0.0, 2.0, 2.0], (0, 1))
                .alpha((0, 3))
                .object(7),
            SpriteSpec::tight(
                "alpha_mask",
                [2.0, 2.0, 4.0, 4.0],
                (0, 1),
                Mesh::triangle(0.25),
            )
            .alpha((0, 2))
            .object(8),
        ],
        "{name}.png",
    )
}

/// Names and keys (`{safe_name}.png`): a second `dup` (`duplicate_name`),
/// `a_b` after `a b` (one safe name: `duplicate_key`), a name whose key
/// has a segment past 255 bytes (`unwritable_key(segment_too_long)`), a
/// Sprite with bytes after its value (`check_read`: `asset_bundle`) and one
/// cut short after its name (`read`).
pub fn names_and_keys() -> Result<AtlasScene, BuildError> {
    let one_px = |name: &str| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, 1));
    let long = "n".repeat(260);
    alone(
        "CAB-s4",
        &[
            rgba(1, "t", 0x60, 4, 4),
            one_px("dup").object(2),
            one_px("dup").object(3),
            one_px("a b").object(4),
            one_px("a_b").object(5),
            one_px(&long).object(6),
            one_px("trailing").object(7).with_trailing(4),
            one_px("cut").object(8).cut_to(24),
            one_px("last").object(9),
        ],
        "{safe_name}.png",
    )
}

/// Every way a render or its sheet fails, each Sprite on its own: crops
/// past Pillow's bounds (`decompression_bomb`, `crop_overflow` from a C
/// `int` and a C `long`, `pillow_memory_error` at the width cap), images
/// with a side of 0 (`empty_image`, one just below the cap), and the other
/// exceptions (`render`: a NaN, an infinity, a right edge before the left,
/// a rotation of 5, a tight Sprite with no vertices, an index past them);
/// then a sheet that fails at the plan (`unsupported_format(Alpha8)`) and
/// sheets that fail at the export, for every Sprite of their group
/// (`no_image_data`, `not_enough_data`); and sheets with a side of 0, which
/// UnityPy decodes to a 0 x 0 image and renders from (B3's review round 1,
/// R1): one alone (`empty_sheet`, its crop padded), one as the alpha
/// texture beside an 8 x 8 sheet (`Image.merge`'s size mismatch, `render`),
/// and both a sheet's and its alpha texture's (a merge of two 0 x 0
/// images, then a crop of it, and a crop of 0 x 0, `empty_image`), and an
/// 8 x 0 sheet beside an 8 x 8 alpha texture (`alpha_zero_main`, `render`).
/// `texture_first` is a second Sprite on the Alpha8 sheet (its skip's
/// reason charged, shared with `bad_format`), with an alpha texture that
/// names no object: the texture's answers come first (B3's review round 2,
/// S5), so its skip is the texture's.
pub fn render_failures() -> Result<AtlasScene, BuildError> {
    let on = |name: &str, rect: [f32; 4]| SpriteSpec::rect(name, rect, (0, 1));
    let cap = 536_870_912.0;
    let past = Mesh {
        indices: vec![0, 1, 3],
        ..Mesh::triangle(0.25)
    };
    let no_vertices = Mesh {
        submeshes: vec![(0, 0)],
        ..Mesh::default()
    };
    let mut objects = vec![rgba(1, "sheet", 0x11, 8, 8)];
    let sprites = [
        on("bomb", [0.0, 0.0, 20000.0, 20000.0]),
        on("c_int", [3.0e9, 0.0, 1.0, 1.0]),
        on("c_int_negative", [-3.0e9, 0.0, 1.0, 1.0]),
        on("c_long", [1.0e19, 0.0, 0.0, 1.0]),
        on("width_cap", [0.0, 0.0, cap, 0.0]),
        on("below_cap", [0.0, 0.0, 536_870_880.0, 0.0]),
        on("zero_width", [1.0, 1.0, 0.0, 3.0]),
        on("nan", [f32::NAN, 0.0, 1.0, 1.0]),
        on("infinity", [0.0, 0.0, f32::INFINITY, 1.0]),
        on("negative_width", [2.0, 1.0, -1.0, 3.0]),
        on("rotation_5", [1.0, 0.0, 5.0, 3.0]).settings(packed(5)),
        SpriteSpec::tight("no_vertices", [0.0, 0.0, 4.0, 4.0], (0, 1), no_vertices),
        SpriteSpec::tight("index_past", [0.0, 0.0, 4.0, 4.0], (0, 1), past),
        on("fine", [0.0, 0.0, 2.0, 2.0]),
    ];
    for (at, spec) in (2..).zip(sprites) {
        objects.push(spec.object(at));
    }
    let size = (4, 4);
    objects.extend([
        texture(30, b"alpha8", format::ALPHA8, size, vec![0; 16]),
        texture(31, b"empty", format::RGBA32, (0, 0), vec![0; 4]),
        texture(34, b"empty_wide", format::RGBA32, (4, 0), vec![0; 4]),
        texture(35, b"empty_alpha", format::RGBA32, (0, 4), vec![0; 4]),
        texture(36, b"empty_rgb", format::RGB24, (8, 0), vec![0; 4]),
        texture(32, b"no_data", format::RGBA32, size, Vec::new()),
        texture(33, b"short", format::RGBA32, size, vec![0; 10]),
        SpriteSpec::rect("bad_format", [0.0, 0.0, 1.0, 1.0], (0, 30)).object(40),
        SpriteSpec::rect("empty_sheet", [0.0, 0.0, 1.0, 1.0], (0, 31)).object(41),
        SpriteSpec::rect("no_data_1", [0.0, 0.0, 1.0, 1.0], (0, 32)).object(42),
        SpriteSpec::rect("short_sheet", [0.0, 0.0, 1.0, 1.0], (0, 33)).object(43),
        SpriteSpec::rect("no_data_2", [1.0, 1.0, 1.0, 1.0], (0, 32)).object(44),
        SpriteSpec::rect("alpha_zero_beside", [0.0, 0.0, 1.0, 1.0], (0, 1))
            .alpha((0, 35))
            .object(45),
        SpriteSpec::rect("alpha_zero_both", [0.0, 0.0, 1.0, 1.0], (0, 34))
            .alpha((0, 35))
            .object(46),
        SpriteSpec::rect("alpha_zero_both_2", [0.0, 0.0, 0.0, 0.0], (0, 34))
            .alpha((0, 35))
            .object(47),
        SpriteSpec::rect("alpha_zero_main", [0.0, 0.0, 1.0, 1.0], (0, 36))
            .alpha((0, 1))
            .object(48),
        SpriteSpec::rect("texture_first", [0.0, 0.0, 1.0, 1.0], (0, 30))
            .alpha((0, 99))
            .object(49),
    ]);
    alone("CAB-s5", &objects, "{name}.png")
}

/// One input's bundle for [`sprite_winners`]: Sprites `shared` and `own_<id>`
/// on a sheet of base `base`.
fn winner_input(id: &str, base: u8) -> Result<SceneBundle, BuildError> {
    let stem = id.trim_end_matches(".bundle");
    let cab_name = format!("CAB-w{stem}");
    let cab = file(
        &[],
        &[
            rgba(1, "sheet", base, 2, 2),
            SpriteSpec::rect("shared", [0.0, 0.0, 2.0, 2.0], (0, 1)).object(2),
            SpriteSpec::rect(&format!("own_{stem}"), [0.0, 0.0, 1.0, 1.0], (0, 1)).object(3),
        ],
        true,
    )?;
    Ok(one(id, &[(cab_name.as_str(), cab)], &[]))
}

/// Several inputs exporting the Sprite key `shared.png` (`{name}.png`),
/// for 4.5's winners: `a` and `b` alike, `c` with other pixels.
pub fn sprite_winners() -> Result<AtlasScene, BuildError> {
    let bundles = vec![
        winner_input("a.bundle", 1)?,
        winner_input("b.bundle", 1)?,
        winner_input("c.bundle", 0x31)?,
    ];
    let inputs = bundles.iter().map(|b| b.id.clone()).collect();
    Ok(AtlasScene {
        bundles,
        inputs,
        options: sprites("{name}.png"),
    })
}

/// Every `sprites` scene, by name.
pub fn sprite_scenes() -> Result<Vec<(&'static str, AtlasScene)>, BuildError> {
    let all: [(&'static str, super::scenes::Make); 18] = [
        ("own_sheets", own_sheets),
        ("dependency_sheets", dependency_sheets),
        ("alpha_textures", alpha_textures),
        ("names_and_keys", names_and_keys),
        ("render_failures", render_failures),
        ("sprite_winners", sprite_winners),
        ("atlas_lookup", super::sprite_atlases::atlas_lookup),
        ("atlas_dependency", super::sprite_atlases::atlas_dependency),
        ("nested_cabs", super::sprite_atlases::nested_cabs),
        ("sprite_fields", super::sprite_atlases::sprite_fields),
        ("sprite_classes", super::sprite_objects::sprite_classes),
        ("atlas_classes", super::sprite_objects::atlas_classes),
        ("shadowed_rows", super::sprite_objects::shadowed_rows),
        ("sprite_pointers", super::sprite_objects::sprite_pointers),
        (
            "sprite_attributes",
            super::sprite_objects::sprite_attributes,
        ),
        ("int_tags", super::sprite_objects::int_tags),
        ("raised_tags", super::sprite_atlases::raised_tags),
        (
            "raised_tags_swapped",
            super::sprite_atlases::raised_tags_swapped,
        ),
    ];
    all.into_iter().map(|(name, f)| Ok((name, f()?))).collect()
}
