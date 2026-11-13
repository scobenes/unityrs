// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/MeshHelper.py's MeshHandler (its constructor, process, copy_from_mesh and copy_from_spriterenderdata), with export/SpriteHelper.py's read of a Sprite's m_RD. See THIRD_PARTY_LICENSES.
//! `MeshHandler`: a Mesh's, or a sprite's render data's, vertex arrays and
//! index buffer (`process`, `helpers/MeshHelper.py:110-195`), and its
//! triangles ([`Processed::get_triangles`]).
//!
//! [`MeshHandler::process`] runs UnityPy's steps in order, so the first
//! failure is UnityPy's:
//!
//! 1. `m_VertexData`, asserted present (`MeshHelper.py:112-113`); then the channels and
//!    streams by version (`:118-140`): below 4 the four fixed streams,
//!    asserted present, then `get_channels`, which is P4
//!    ([`Unsupported::FixedChannels`]); at 4 the file's `m_Streams` and
//!    `m_Channels`, asserted present; from 5 `m_Channels`, asserted
//!    present, and the streams [`crate::vertex`] derives.
//! 2. A Mesh's `m_StreamData`, when it names a path: its bytes, through
//!    the caller's [`Resources`], take the place of `m_DataSize`
//!    (`MeshHelper.py:142-155`). With no resources, UnityPy's assert for a Mesh with no
//!    object reader.
//! 3. The index size (`MeshHelper.py:158-167`): `m_Use16BitIndices` when the Mesh has
//!    it; else, from 2017.4 or at 2017.3 for a mesh whose `m_MeshCompression`
//!    is 0, whether `m_IndexFormat` is 0 (a Mesh without it is 32-bit
//!    there); else 16-bit. Then `copy_from_mesh` or
//!    `copy_from_spriterenderdata`, or, for a source object mode built as
//!    an `UnknownObject`, `ValueError` (`:168-172`).
//! 4. The index buffer, when non-empty: `bytes()` of the list, unpacked as
//!    `H` or `I` (`MeshHelper.py:174-186`).
//! 5. The vertex data (`MeshHelper.py:188-189`, [`crate::vertex`]).
//! 6. A Mesh's compressed mesh (`MeshHelper.py:191-192`,
//!    [`crate::compressed`]): the vertex count, always, and each array it
//!    holds, over the vertex data's; its triangles' indices replace the
//!    index buffer.
//! 7. The vertex count, when 0, from the positions (`MeshHelper.py:194-195`).
//!
//! **What differs from UnityPy's object: the handler changes nothing it
//! reads** (`PARITY.md` section 19.3). `process` returns its results rather
//! than setting them on the handler, and reads its source without changing
//! it: UnityPy's sets a streamed Mesh's `m_DataSize` to the resource's bytes
//! (`MeshHelper.py:155`), and its `get_triangles` writes a strip's new count into
//! the `SubMesh` (`:666`). So a second `process` gives the first's answer,
//! where UnityPy's reads what its first left on the handler: `copy_from_mesh`
//! keeps the index tuple it unpacked, which `bytes()` then refuses past 255
//! (`ValueError`) or packs again. The constructor's `ValueError` for a
//! missing version cannot arise: [`MeshHandler::new`] takes one, as every
//! UnityPy caller passes one or a Mesh's reader holds one
//! (`MeshHelper.py:95-108`).
//!
//! **Work.** Every read of a field counts its scans on the meter `process`
//! is given (`view`'s module docs), so the source is resolved there, not
//! when the [`Source`] is made.

use std::sync::Arc;

use unityrs_core::env::Environment;
use unityrs_core::limits::{Charge, Meter, MeteredVec};
use unityrs_core::{Bytes, Parsed, RawStr, SerializedFile};

use crate::Version;
use crate::compressed::decompress_compressed_mesh;
use crate::error::{MeshError, StructError, Unsupported};
use crate::triangles::{Triangles, get_triangles};
use crate::unpack::{Values, VertexArray, widen};
use crate::vertex::{Attributes, Streams, VertexData, field, get_streams, read_vertex_data};
use crate::view::{
    BONE_WEIGHTS, CHANNEL_INFO, Ints, List, MESH, Obj, SPRITE, SPRITE_RENDER_DATA, STREAM_INFO,
    STREAMING_INFO, VERTEX_DATA,
};
use unityrs_core::Value;

/// Where a Mesh's `m_StreamData` bytes come from: UnityPy's
/// `get_resource_data(path, assets_file, offset, size)`
/// (`helpers/ResourceReader.py:10-38`) from the Mesh's own file.
pub trait Resources: Send + Sync {
    /// The `size` bytes at `offset` of the resource `path` names.
    fn resource(&self, path: &RawStr, offset: u64, size: u64)
    -> Result<Bytes, unityrs_core::Error>;
}

const _: () = crate::send_sync::<dyn Resources>();

/// The core's resource read from an object's file: [`Environment::resource`],
/// its port of `get_resource_data`.
#[derive(Clone, Copy, Debug)]
pub struct FileResources<'a> {
    /// The Environment that loaded the file.
    pub env: &'a Environment,
    /// The file holding the Mesh (UnityPy's `object_reader.assets_file`).
    pub file: &'a Arc<SerializedFile>,
}

const _: () = crate::send_sync::<FileResources<'static>>();

impl Resources for FileResources<'_> {
    fn resource(
        &self,
        path: &RawStr,
        offset: u64,
        size: u64,
    ) -> Result<Bytes, unityrs_core::Error> {
        self.env.resource(self.file, path, offset, size)
    }
}

/// What the handler is given: a Mesh, or a Sprite's render data, read
/// when the handler runs.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    parsed: &'a Parsed,
    kind: Kind,
}

const _: () = crate::send_sync::<Source<'static>>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Mesh,
    RenderData,
}

impl<'a> Source<'a> {
    /// A Mesh object's value (its tree's root a `Mesh` node, which
    /// `process` checks).
    #[must_use]
    pub const fn mesh(parsed: &'a Parsed) -> Self {
        Self {
            parsed,
            kind: Kind::Mesh,
        }
    }

    /// A Sprite object's render data, `m_Sprite.m_RD`, as `SpriteHelper`
    /// hands it to the handler (`export/SpriteHelper.py:117`): `process`
    /// returns an `AttributeError` where object mode built the Sprite as an
    /// `UnknownObject` without one.
    #[must_use]
    pub const fn sprite(parsed: &'a Parsed) -> Self {
        Self {
            parsed,
            kind: Kind::RenderData,
        }
    }

    /// The object the handler reads, its reads counted on `meter`.
    fn root<'m>(&self, meter: &'m Meter) -> Result<Obj<'m>, MeshError>
    where
        'a: 'm,
    {
        match self.kind {
            Kind::Mesh => Obj::root(self.parsed, &MESH, meter),
            Kind::RenderData => Obj::root(self.parsed, &SPRITE, meter)?
                .class("m_RD", &SPRITE_RENDER_DATA)?
                .ok_or(MeshError::NoAttribute("m_RD")),
        }
    }

    /// `isinstance(mesh, Mesh)` (`helpers/MeshHelper.py:143`, `:158`).
    fn is_mesh(&self, obj: &Obj<'_>) -> bool {
        self.kind == Kind::Mesh && obj.built()
    }

    /// `isinstance(mesh, SpriteRenderData)` (`helpers/MeshHelper.py:169`).
    fn is_render_data(&self, obj: &Obj<'_>) -> bool {
        self.kind == Kind::RenderData && obj.built()
    }
}

/// `MeshHandler(src, version)` (`helpers/MeshHelper.py:71-108`).
#[derive(Clone, Copy, Debug)]
pub struct MeshHandler<'a> {
    src: Source<'a>,
    version: Version,
    little: bool,
}

const _: () = crate::send_sync::<MeshHandler<'static>>();

impl<'a> MeshHandler<'a> {
    /// A handler of `src` at `version`, little-endian (UnityPy's default,
    /// and every caller's).
    #[must_use]
    pub fn new(src: Source<'a>, version: Version) -> Self {
        Self {
            src,
            version,
            little: true,
        }
    }

    /// The handler with `endianess` other than `<` (`little` false):
    /// components are then not swapped, and read big-endian
    /// (`helpers/MeshHelper.py:363`).
    #[must_use]
    pub fn little_endian(mut self, little: bool) -> Self {
        self.little = little;
        self
    }

    /// `process()` (module docs). `resources` reads `m_StreamData`;
    /// `None` is a Mesh with no object reader. Every array is charged to
    /// `meter` before it is allocated, and every loop's work counted on it
    /// before the loop runs (the crate's docs). It changes nothing, so a
    /// second call gives the first's answer.
    pub fn process(
        &self,
        resources: Option<&dyn Resources>,
        meter: &Arc<Meter>,
    ) -> Result<Processed<'a>, MeshError> {
        let version = self.version;
        let mesh = self.src.root(meter)?;
        // UnityPy MeshHelper.py:112-113
        let vd = mesh
            .class("m_VertexData", &VERTEX_DATA)?
            .ok_or(MeshError::Assertion(""))?;
        let (channels, streams) = channels_and_streams(version, &vd, meter)?;

        // UnityPy MeshHelper.py:142-155
        let streamed = if self.src.is_mesh(&mesh) {
            stream_data(&mesh, resources)?
        } else {
            None
        };

        let mut use_16bit = true;
        let mut attrs = Attributes::default();
        let index = if self.src.is_mesh(&mesh) {
            // UnityPy MeshHelper.py:159-167
            if let Some(v) = mesh.int32("m_Use16BitIndices")? {
                use_16bit = v != 0;
            } else if version.at_least(&[2017, 4])?
                || (version.minor_is(2017, 3)?
                    && field(&mesh, "m_MeshCompression", u64::from(u8::MAX))? == 0)
            {
                use_16bit = mesh.int32("m_IndexFormat")? == Some(0);
            }
            copy_from_mesh(&mesh, &mut attrs, meter)?
        } else if self.src.is_render_data(&mesh) {
            copy_from_spriterenderdata(&mesh)?
        } else {
            return Err(MeshError::UnknownMeshType);
        };

        // UnityPy MeshHelper.py:174-186
        let mut index_buffer = match index {
            Some(ints) => Some(unpack_indices(ints, use_16bit, meter)?),
            None => None,
        };

        // UnityPy MeshHelper.py:188-189
        let mut vertex_count = 0;
        if version.at_least(&[3, 5])? {
            let data = VertexData {
                obj: vd,
                streamed: streamed.as_deref(),
            };
            let read = read_vertex_data(
                version,
                self.little,
                data,
                &channels,
                &streams,
                &mut attrs,
                meter,
            )?;
            if let Some(n) = read {
                vertex_count = n;
            }
        }

        // UnityPy MeshHelper.py:191-192
        if self.src.is_mesh(&mesh) && version.at_least(&[2, 6])? {
            let done = decompress_compressed_mesh(version, &mesh, &mut attrs, meter)?;
            vertex_count = done.vertex_count;
            if let Some((values, charge)) = done.triangles {
                index_buffer = Some(IndexBuffer {
                    values,
                    _charge: charge,
                });
            }
        }

        // UnityPy MeshHelper.py:194-195
        if vertex_count == 0
            && let Some(v) = attrs.vertices.as_ref().filter(|v| !v.is_empty())
        {
            vertex_count = u64::try_from(v.len()).unwrap_or(u64::MAX);
        }

        Ok(Processed {
            src: self.src,
            version,
            vertex_count,
            use_16bit,
            index_buffer,
            attrs,
        })
    }
}

/// The channels and the streams, by version (`helpers/MeshHelper.py:118-140`).
fn channels_and_streams<'a>(
    version: Version,
    vd: &Obj<'a>,
    meter: &Arc<Meter>,
) -> Result<(List<'a>, Streams<'a>), MeshError> {
    let major = version.major()?;
    if major < 4 {
        // UnityPy MeshHelper.py:119-124: each `is not None`, in order.
        for name in [
            "m_Streams_0_",
            "m_Streams_1_",
            "m_Streams_2_",
            "m_Streams_3_",
        ] {
            if vd.class(name, &STREAM_INFO)?.is_none() {
                return Err(MeshError::Assertion(""));
            }
        }
        // UnityPy MeshHelper.py:132
        return Err(MeshError::Unsupported(Unsupported::FixedChannels));
    }
    if major == 4 {
        // UnityPy MeshHelper.py:134-136
        let streams = vd
            .list("m_Streams", &STREAM_INFO)?
            .ok_or(MeshError::Assertion(""))?;
        let channels = vd
            .list("m_Channels", &CHANNEL_INFO)?
            .ok_or(MeshError::Assertion(""))?;
        return Ok((channels, Streams::File(streams)));
    }
    // UnityPy MeshHelper.py:138-140
    let channels = vd
        .list("m_Channels", &CHANNEL_INFO)?
        .ok_or(MeshError::Assertion(""))?;
    let count = field(vd, "m_VertexCount", u64::from(u32::MAX))?;
    let streams = get_streams(version, &channels, count, meter)?;
    Ok((channels, streams))
}

/// A Mesh's `m_StreamData` bytes, when it names a path
/// (`helpers/MeshHelper.py:142-155`).
fn stream_data(
    mesh: &Obj<'_>,
    resources: Option<&dyn Resources>,
) -> Result<Option<Bytes>, MeshError> {
    let Some(sd) = mesh.class("m_StreamData", &STREAMING_INFO)? else {
        return Ok(None);
    };
    let path = sd.string("path")?.ok_or(MeshError::NoAttribute("path"))?;
    if path.is_empty() {
        return Ok(None);
    }
    let resources = resources.ok_or(MeshError::Assertion(
        "No object reader assigned to the input Mesh!",
    ))?;
    let offset = field(&sd, "offset", u64::MAX)?;
    let size = field(&sd, "size", u64::MAX)?;
    resources
        .resource(path, offset, size)
        .map(Some)
        .map_err(MeshError::Resource)
}

/// The length of a list-valued field, which Python tests for truth.
fn list_len(v: &Value, name: &'static str) -> Result<usize, MeshError> {
    match v {
        Value::List(items) => Ok(items.len()),
        Value::Tuple(items) => Ok(items.len()),
        Value::Prims(p) => Ok(p.len()),
        Value::Bytes(b) => Ok(b.len()),
        _ => Err(MeshError::malformed(format!("{name} is not a list"))),
    }
}

/// `copy_from_mesh()` (`helpers/MeshHelper.py:197-233`): the index buffer
/// to unpack, and the skin's bone indices and weights into `attrs`.
fn copy_from_mesh<'a>(
    mesh: &Obj<'a>,
    attrs: &mut Attributes,
    meter: &Arc<Meter>,
) -> Result<Option<Ints<'a>>, MeshError> {
    // UnityPy MeshHelper.py:203-204
    let index = mesh.ints("m_IndexBuffer")?.filter(|i| i.len() > 0);
    // UnityPy MeshHelper.py:206-224: the Mesh's own arrays (P2a).
    for name in [
        "m_Vertices",
        "m_Normals",
        "m_Tangents",
        "m_UV",
        "m_UV1",
        "m_Colors",
    ] {
        if let Some((_, v)) = mesh.get(name)?
            && list_len(v, name)? > 0
        {
            return Err(MeshError::Unsupported(Unsupported::MeshArray(name)));
        }
    }
    // UnityPy MeshHelper.py:226-233
    if let Some(skin) = mesh.list("m_Skin", &BONE_WEIGHTS)?
        && skin.len() > 0
    {
        let (indices, weights) = copy_skin(&skin, meter)?;
        attrs.bone_indices = Some(indices);
        attrs.bone_weights = Some(weights);
    }
    Ok(index)
}

/// The skin's bone indices, then its weights, each a list of 4-tuples
/// (`helpers/MeshHelper.py:228-233`).
fn copy_skin(skin: &List<'_>, meter: &Arc<Meter>) -> Result<(VertexArray, VertexArray), MeshError> {
    const INDICES: [&str; 4] = [
        "boneIndex_0_",
        "boneIndex_1_",
        "boneIndex_2_",
        "boneIndex_3_",
    ];
    const WEIGHTS: [&str; 4] = ["weight_0_", "weight_1_", "weight_2_", "weight_3_"];
    let n = skin.len().saturating_mul(4);
    // Charged, then counted: a refused charge counts no work. One unit a
    // value, 8 a bone (its four indices and four weights).
    let mut indices = MeteredVec::<i64>::with_capacity(meter, n)?;
    let mut weights = MeteredVec::<f64>::with_capacity(meter, n)?;
    meter.count_decode(u64::try_from(n).unwrap_or(u64::MAX).saturating_mul(2))?;
    for bone in skin.iter() {
        let bone = bone?;
        for name in INDICES {
            let (_, v) = bone.get(name)?.ok_or(MeshError::NoAttribute(name))?;
            match v {
                Value::Int(i) if i32::try_from(*i).is_ok() => indices.push(*i)?,
                _ => return Err(MeshError::malformed(format!("{name} is not an int"))),
            }
        }
    }
    for bone in skin.iter() {
        let bone = bone?;
        for name in WEIGHTS {
            let (_, v) = bone.get(name)?.ok_or(MeshError::NoAttribute(name))?;
            match v {
                Value::F32(bits) => weights.push(widen(*bits))?,
                Value::F64(bits) => weights.push(f64::from_bits(*bits))?,
                _ => return Err(MeshError::malformed(format!("{name} is not a float"))),
            }
        }
    }
    let (indices, c1) = indices.into_parts();
    let (weights, c2) = weights.into_parts();
    Ok((
        VertexArray::new(4, Values::Int(indices), c1),
        VertexArray::new(4, Values::Float(weights), c2),
    ))
}

/// `copy_from_spriterenderdata()` (`helpers/MeshHelper.py:235-251`): the
/// index buffer to unpack.
fn copy_from_spriterenderdata<'a>(rd: &Obj<'a>) -> Result<Option<Ints<'a>>, MeshError> {
    // UnityPy MeshHelper.py:240-244: `m_IndexBuffer`, else `indices`.
    let mut index = rd.ints("m_IndexBuffer")?.filter(|i| i.len() > 0);
    if index.is_none() {
        index = rd.ints("indices")?.filter(|i| i.len() > 0);
    }
    // UnityPy MeshHelper.py:246-251: the sprite's own vertices (P2a).
    if let Some((_, v)) = rd.get("vertices")?
        && list_len(v, "vertices")? > 0
    {
        return Err(MeshError::Unsupported(Unsupported::SpriteVertices));
    }
    Ok(index)
}

/// `bytes(index)` unpacked as `<{n}H` or `<{n}I`
/// (`helpers/MeshHelper.py:174-186`).
fn unpack_indices(
    ints: Ints<'_>,
    use_16bit: bool,
    meter: &Arc<Meter>,
) -> Result<IndexBuffer, MeshError> {
    let len = ints.len();
    let size = if use_16bit { 2 } else { 4 };
    let count = len / size;
    // Charged, then counted: a refused charge counts no work.
    let mut out = MeteredVec::<u32>::with_capacity(meter, count)?;
    meter.count_decode(u64::try_from(len).unwrap_or(u64::MAX).saturating_mul(2))?;
    // UnityPy MeshHelper.py:175: `bytes()` checks every value first.
    if ints.as_u8().is_none() {
        for i in 0..len {
            ints.byte(i)?;
        }
    }
    if !len.is_multiple_of(size) {
        let wanted = u64::try_from(count * size).unwrap_or(u64::MAX);
        return Err(MeshError::Struct(StructError::UnpackSize(wanted)));
    }
    for k in 0..count {
        let at = k * size;
        let byte = |j: usize| ints.byte(at + j).map(u32::from);
        let v = if use_16bit {
            byte(0)? | byte(1)? << 8
        } else {
            byte(0)? | byte(1)? << 8 | byte(2)? << 16 | byte(3)? << 24
        };
        out.push(v)?;
    }
    let (values, charge) = out.into_parts();
    Ok(IndexBuffer {
        values,
        _charge: charge,
    })
}

/// The unpacked index buffer (`m_IndexBuffer` after `process`).
#[derive(Debug)]
pub struct IndexBuffer {
    values: Vec<u32>,
    // Declared last: the charge is returned after the values are freed.
    _charge: Charge,
}

const _: () = crate::send_sync::<IndexBuffer>();

impl IndexBuffer {
    /// The indices.
    #[must_use]
    pub fn as_slice(&self) -> &[u32] {
        &self.values
    }
}

/// What `process()` leaves on the handler: the vertex count, the index
/// size and buffer, and the attributes the channels and the skin filled.
#[derive(Debug)]
pub struct Processed<'a> {
    src: Source<'a>,
    version: Version,
    vertex_count: u64,
    use_16bit: bool,
    index_buffer: Option<IndexBuffer>,
    attrs: Attributes,
}

const _: () = crate::send_sync::<Processed<'static>>();

impl Processed<'_> {
    /// `m_VertexCount`.
    #[must_use]
    pub fn vertex_count(&self) -> u64 {
        self.vertex_count
    }

    /// `m_Use16BitIndices`.
    #[must_use]
    pub fn use_16bit_indices(&self) -> bool {
        self.use_16bit
    }

    /// `m_IndexBuffer`: `None` where the source had none.
    #[must_use]
    pub fn index_buffer(&self) -> Option<&IndexBuffer> {
        self.index_buffer.as_ref()
    }

    /// `m_Vertices`.
    #[must_use]
    pub fn vertices(&self) -> Option<&VertexArray> {
        self.attrs.vertices.as_ref()
    }

    /// `m_Normals`.
    #[must_use]
    pub fn normals(&self) -> Option<&VertexArray> {
        self.attrs.normals.as_ref()
    }

    /// `m_Tangents`.
    #[must_use]
    pub fn tangents(&self) -> Option<&VertexArray> {
        self.attrs.tangents.as_ref()
    }

    /// `m_Colors`.
    #[must_use]
    pub fn colors(&self) -> Option<&VertexArray> {
        self.attrs.colors.as_ref()
    }

    /// `m_UV0` to `m_UV7`: `uv(i)`, `None` past 7.
    #[must_use]
    pub fn uv(&self, i: usize) -> Option<&VertexArray> {
        self.attrs.uv.get(i).and_then(Option::as_ref)
    }

    /// `m_BoneWeights`.
    #[must_use]
    pub fn bone_weights(&self) -> Option<&VertexArray> {
        self.attrs.bone_weights.as_ref()
    }

    /// `m_BoneIndices`.
    #[must_use]
    pub fn bone_indices(&self) -> Option<&VertexArray> {
        self.attrs.bone_indices.as_ref()
    }

    /// `m_BindPose`, which a compressed mesh below 5 sets: each matrix's
    /// 16 `float`s flat, row by row, so a tuple of 16 (UnityPy's
    /// `reshape(shape=(4, 4))` gives each as four rows of 4, the last
    /// matrix's rows cut short, then empty, where the count is no multiple
    /// of 16).
    #[must_use]
    pub fn bind_pose(&self) -> Option<&VertexArray> {
        self.attrs.bind_pose.as_ref()
    }

    /// `get_triangles()` ([`crate::triangles`]): the triangles charged to
    /// `meter`, and the work of reading the submeshes counted on it. It
    /// changes nothing, so each call reads the submeshes' `indexCount`s as
    /// the source holds them, where UnityPy's second call reads the counts
    /// its first wrote into a strip's `SubMesh`.
    pub fn get_triangles(&self, meter: &Arc<Meter>) -> Result<Triangles, MeshError> {
        get_triangles(
            &self.src.root(meter)?,
            self.version,
            self.use_16bit,
            self.index_buffer.as_ref().map(IndexBuffer::as_slice),
            meter,
        )
    }
}
