"""The trees tools/fixtures/meshdecode.py's handler cases are laid out from, built here by hand, not a fixture
script: a Mesh (class 43) or a Sprite (213) at a version, as node rows (meshdecode_build.py).

Nothing here comes from a TPK or a dump (docs/plans/core.md section 9, Q2). A class holds, of the fields UnityPy
1.25.2's MIT class definitions (`classes/generated.py`) declare: the fields `helpers/MeshHelper.py` reads; the fields
without a default, which object mode needs to build the class rather than an `UnknownObject` (`PARITY.md` section
9.2); and, in `StreamInfo` and `VertexData`, the layout fields AssetStudio's readers give beside them (`align`,
`dividerOp`, `frequency`, `m_CurrentChannels`). A field is in every tree unless a version boundary is drawn for it,
and each boundary names its source where it is drawn: a line of MeshHelper, or one of AssetStudio's MIT readers, of
Perfare/AssetStudio's release v0.16.47 (format facts only: no AssetStudio code is ported or kept here):
- `AssetStudio/Classes/Mesh.cs`: `SubMesh`, `StreamInfo`, `ChannelInfo`, `VertexData`, `CompressedMesh`,
  `BoneWeights4` and `Mesh`;
- `AssetStudio/Classes/Sprite.cs`: `SpriteVertex`, `SpriteRenderData` and `Sprite`;
- `AssetStudio/Classes/Texture2D.cs`: `StreamingInfo`;
- `AssetStudio/Classes/AnimationClip.cs`: `PackedFloatVector` and `PackedIntVector`;
- `AssetStudio/Classes/PPtr.cs`: `PPtr`.
An integer leaf's type is the one those readers read it as (`ReadUInt32` an `unsigned int`, `ReadInt32` an `int`,
`ReadByte` a `UInt8`, `ReadUInt16` a `UInt16`, `ReadInt64` an `SInt64`), unless the reader's class notes it otherwise
in a comment (`StreamingInfo.offset`, `//ulong`: a `UInt64`); a float is `float`, a string `string`, a byte array
`TypelessData` or a vector of `UInt8`; no leaf has an alignment flag.

`meshdecode.py --tpk` runs every case on UnityPy's own TPK trees as well, at run time, and reports where the two
trees' outcomes differ; those trees and outcomes exist only in memory and in a pipe, written to no path.

Run on its own it only imports (tools/parity/tests/test_fixture_modules.py).
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from meshdecode_build import leaf, vec  # noqa: E402


def node(ty: str, name: str, *fields: list[list]) -> list[list]:
    """A class node `name` of type `ty` holding `fields` (rows rooted at level 0)."""
    return [[0, ty, name, 0], *[[r[0] + 1, *r[1:]] for f in fields for r in f]]


def vector(name: str, *fields: list[list], ty: str) -> list[list]:
    """A vector of class `ty`, each element holding `fields`."""
    return vec(name, node(ty, "data", *fields))


def xyz(ty: str, name: str, axes: str) -> list[list]:
    return node(ty, name, *[leaf("float", a) for a in axes])


def at_least(version: tuple, *v: int) -> bool:
    return version >= v


def aabb(name: str) -> list[list]:
    # classes/generated.py AABB
    return node("AABB", name, xyz("Vector3f", "m_Center", "xyz"), xyz("Vector3f", "m_Extent", "xyz"))


def sub_mesh(version: tuple) -> list[list]:
    """`SubMesh` (classes/generated.py): `firstByte`, `indexCount` and `topology` are what `get_triangles` reads
    (helpers/MeshHelper.py:630-683). Below 4 a submesh whose `topology` is not 0 is a strip (`:646`, then `:651`); no
    P1b run reaches `get_triangles` below 4 (`process` refuses a 3.x Mesh's fixed streams first). `topology` is in
    every version's tree: AssetStudio's `SubMesh` (Classes/Mesh.cs) reads an `Int32` there at every version, but reads
    by layout, not by field name. UnityPy's MIT `SubMesh` names `isTriStrip` beside `topology`
    (classes/generated.py:11105), with no version, so the name is licensed and the versions that have it unsourced. This
    3.x SubMesh cannot stand in for PARITY's 3.x-strips divergence test (section 19.2), and P4 sources which versions
    have `isTriStrip` before any 3.x triangles are recorded.
    `version` is unused: no field has a boundary."""
    del version
    return vector(
        "m_SubMeshes",
        leaf("unsigned int", "firstByte"),
        leaf("unsigned int", "indexCount"),
        leaf("int", "topology"),
        leaf("unsigned int", "firstVertex"),
        leaf("unsigned int", "vertexCount"),
        aabb("localAABB"),
        ty="SubMesh",
    )


def packed(name: str, floats: bool) -> list[list]:
    """A `PackedBitVector` (classes/generated.py): `m_NumItems` is what `decompress_compressed_mesh`'s gates read;
    a float vector has `m_Range` and `m_Start` besides, as AssetStudio's `PackedFloatVector` and `PackedIntVector`
    (Classes/AnimationClip.cs) read them."""
    scale = [leaf("float", "m_Range"), leaf("float", "m_Start")] if floats else []
    return node(
        "PackedBitVector", name, leaf("unsigned int", "m_NumItems"), *scale, vec("m_Data", leaf("UInt8", "data")),
        leaf("UInt8", "m_BitSize"),
    )  # fmt: skip


def compressed_mesh(version: tuple) -> list[list]:
    """`CompressedMesh` (classes/generated.py), whose arrays `decompress_compressed_mesh` reads
    (helpers/MeshHelper.py:468-628), `m_BindPoses` only below 5 (:516) and `m_FloatColors` only from 5 (:568); their
    boundaries, `m_Colors` from 3.5 below 5 and `m_UVInfo` from 5 are AssetStudio's `CompressedMesh`
    (Classes/Mesh.cs)."""
    five = at_least(version, 5)
    fields = [packed("m_Vertices", True), packed("m_UV", True)]
    if not five:
        fields.append(packed("m_BindPoses", True))
    fields += [packed("m_Normals", True), packed("m_Tangents", True), packed("m_Weights", False),
               packed("m_NormalSigns", False), packed("m_TangentSigns", False)]  # fmt: skip
    if five:
        fields.append(packed("m_FloatColors", True))
    fields += [packed("m_BoneIndices", False), packed("m_Triangles", False)]
    if five:
        fields.append(leaf("unsigned int", "m_UVInfo"))
    elif at_least(version, 3, 5):
        fields.append(packed("m_Colors", False))
    return node("CompressedMesh", "m_CompressedMesh", *fields)


def stream_info(name: str, version: tuple) -> list[list]:
    """`StreamInfo` (classes/generated.py): `channelMask`, `offset` and `stride`, which `read_vertex_data` reads
    (helpers/MeshHelper.py:338-376); below 4 a 32-bit `stride` and `align`, from 4 a byte's `stride`, `dividerOp` and
    `frequency`, as AssetStudio's `StreamInfo` (Classes/Mesh.cs) reads them."""
    four = at_least(version, 4)
    rest = [leaf("UInt8", "dividerOp"), leaf("UInt16", "frequency")] if four else [leaf("unsigned int", "align")]
    return node("StreamInfo", name, leaf("unsigned int", "channelMask"), leaf("unsigned int", "offset"),
                leaf("UInt8" if four else "unsigned int", "stride"), *rest)  # fmt: skip


def vertex_data(version: tuple) -> list[list]:
    """`VertexData` (classes/generated.py), which `process` reads by version (helpers/MeshHelper.py:118-140): below
    4 the four `m_Streams[i]`, at 4 `m_Streams` and `m_Channels`, from 5 `m_Channels`, and `m_CurrentChannels` below
    2018, as AssetStudio's `VertexData` (Classes/Mesh.cs) reads them; `m_Channels`' elements as its `ChannelInfo`."""
    fields = [leaf("unsigned int", "m_CurrentChannels")] if not at_least(version, 2018) else []
    fields.append(leaf("unsigned int", "m_VertexCount"))
    if at_least(version, 4):
        channel = [leaf("UInt8", n) for n in ("stream", "offset", "format", "dimension")]
        fields.append(vector("m_Channels", *channel, ty="ChannelInfo"))
    if not at_least(version, 4):
        fields += [stream_info(f"m_Streams[{i}]", version) for i in range(4)]
    elif not at_least(version, 5):
        fields.append(vec("m_Streams", stream_info("data", version)))
    fields.append(leaf("TypelessData", "m_DataSize"))
    return node("VertexData", "m_VertexData", *fields)


def skin(version: tuple) -> list[list]:
    """`m_Skin` (classes/generated.py:4633, a list of `BoneInfluence` or of `BoneWeights4`, two classes of the same
    fields, classes/generated.py:6956-6976), which `copy_from_mesh` copies (helpers/MeshHelper.py:226-233). The element
    type's name is a choice between UnityPy's two: `BoneWeights4` at every version (one skin case is `BoneInfluence`,
    with no version claim). AssetStudio's `Mesh` (Classes/Mesh.cs) reads the element's layout, four floats and four
    `Int32`s, by position, not by type name. `version` is unused: no field has a boundary."""
    del version
    weights = [leaf("float", f"weight[{i}]") for i in range(4)]
    bones = [leaf("int", f"boneIndex[{i}]") for i in range(4)]
    return vector("m_Skin", *weights, *bones, ty="BoneWeights4")


def mesh(version: tuple) -> list[list]:
    """A Mesh (classes/generated.py). `m_IndexFormat` from 2017.3, the first version `process` reads it at
    (helpers/MeshHelper.py:162-165). The other boundaries are AssetStudio's `Mesh` (Classes/Mesh.cs):
    `m_Use16BitIndices` below 3.5 and `m_VertexData` from 3.5 (below it `process` asserts, :113); `m_Skin` below
    2018.2; `m_StreamData` from 2018.3, which `process` reads (:142-155), its `offset` 64 bits from 2020 as
    AssetStudio's `StreamingInfo` (Classes/Texture2D.cs) reads it, and unsigned, a `UInt64`, as that class notes it
    unsigned in a comment (`public long offset; //ulong`), so no tree here gives a negative offset."""
    fields = [leaf("string", "m_Name"), sub_mesh(version)]
    if not at_least(version, 2018, 2):
        fields.append(skin(version))
    fields.append(vector("m_BindPose", *[leaf("float", f"e{r}{c}") for r in range(4) for c in range(4)], ty="Matrix4x4f"))
    if not at_least(version, 3, 5):
        fields.append(leaf("int", "m_Use16BitIndices"))
    else:
        fields.append(vertex_data(version))
    fields += [compressed_mesh(version), aabb("m_LocalAABB"), leaf("UInt8", "m_MeshCompression"),
               leaf("int", "m_MeshUsageFlags")]  # fmt: skip
    if at_least(version, 2017, 3):
        fields.append(leaf("int", "m_IndexFormat"))
    fields.append(vec("m_IndexBuffer", leaf("UInt8", "data")))
    if at_least(version, 2018, 3):
        offset = "UInt64" if at_least(version, 2020) else "unsigned int"
        fields.append(node("StreamingInfo", "m_StreamData", leaf(offset, "offset"), leaf("unsigned int", "size"),
                           leaf("string", "path")))  # fmt: skip
    return node("Mesh", "Base", *fields)


def render_data(version: tuple) -> list[list]:
    """`SpriteRenderData` (classes/generated.py): `m_SubMeshes`, `m_IndexBuffer` and `m_VertexData`, which `process`
    reads, and `vertices` and `indices`, which `copy_from_spriterenderdata` reads (helpers/MeshHelper.py:235-251); the
    first three from 5.6 and the others below, as AssetStudio's `SpriteRenderData` (Classes/Sprite.cs) reads them, and
    `texture` as its `PPtr` (Classes/PPtr.cs)."""
    fields = [node("PPtr<Texture2D>", "texture", leaf("int", "m_FileID"), leaf("SInt64", "m_PathID"))]
    if at_least(version, 5, 6):
        fields += [sub_mesh(version), vec("m_IndexBuffer", leaf("UInt8", "data")), vertex_data(version)]
    else:
        fields += [sprite_vertices(), vec("indices", leaf("UInt16", "data"))]
    rect = node("Rectf", "textureRect", *[leaf("float", n) for n in ("x", "y", "width", "height")])
    fields += [rect, xyz("Vector2f", "textureRectOffset", "xy"), leaf("unsigned int", "settingsRaw")]
    return node("SpriteRenderData", "m_RD", *fields)


def sprite_vertices() -> list[list]:
    """`vertices`, a list of `SpriteVertex` (classes/generated.py): `pos`. AssetStudio's `SpriteVertex`
    (Classes/Sprite.cs) has a `uv` up to 4.3, which no case reaches (none is below 5.5)."""
    return vector("vertices", xyz("Vector3f", "pos", "xyz"), ty="SpriteVertex")


def sprite(version: tuple) -> list[list]:
    """A Sprite (classes/generated.py): its render data `m_RD`, which `SpriteHelper` hands the handler, beside the
    fields without a default, read as AssetStudio's `Sprite` (Classes/Sprite.cs) reads them."""
    rect = node("Rectf", "m_Rect", *[leaf("float", n) for n in ("x", "y", "width", "height")])
    return node(
        "Sprite", "Base", leaf("string", "m_Name"), rect, xyz("Vector2f", "m_Offset", "xy"),
        leaf("float", "m_PixelsToUnits"), leaf("unsigned int", "m_Extrude"), render_data(version),
    )  # fmt: skip


def tree(class_id: int, version: tuple) -> list[list]:
    """The hand-built tree of class 43 (Mesh) or 213 (Sprite) at `version`, a (major, minor, build, ...) tuple."""
    return {43: mesh, 213: sprite}[class_id](tuple(version))
