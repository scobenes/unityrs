"""The handler cases of tools/fixtures/meshdecode.py, not a fixture script: each a Mesh or a Sprite laid out from the
tree `tree(class_id, version)` gives (the hand-built one of meshdecode_trees.py; `meshdecode.py --tpk` points
`TREES` at the TPK's, at run time), with the version the handler is given, which `meshdecode.py` runs through
UnityPy's MeshHandler and records.

A case is a dict: `name`; `source`, `mesh` or `sprite` (a Sprite's `m_RD`, as `export/SpriteHelper.py:117` hands it
to the handler); `version`, the handler's 5-tuple; `rows`, the tree; `data`, the object's bytes; `endian`, the
handler's `endianess`; and, for `m_StreamData`, `resource` (the bytes `get_resource_data` returns) and `reader`
(whether the Mesh has an object reader). `cases()` lists them by what they cover: every format of each
`VertexFormat` table, half and float bit patterns, derived streams and their strides, 16-byte alignment and
offsets, pre-2018 colour, channel assignment by version, 4.x's own streams and masks, the fixed streams below 4,
index formats (2017.3's and 2017.4's rules, `m_Use16BitIndices`), every topology, the skin, `m_StreamData`, the
compressed mesh's gates and their order, `m_UVInfo`, sprites, object mode's `UnknownObject`s, a Mesh's own
arrays, each error path, and channels, submeshes and fields by the count (the work tests' scale). Run on its own it
only imports (tools/parity/tests/test_fixture_modules.py).
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from meshdecode_build import (  # noqa: E402
    add,
    default_value,
    drop,
    element,
    fit,
    lay_out,
    leaf,
    retype,
    splitmix,
    end,
    to_node,
    vec,
    write,
)
from meshdecode_trees import tree as hand_tree  # noqa: E402

# What `tree` builds from: (class_id, version 5-tuple) -> rows.
TREES = hand_tree
TYPES = {"a": 0, "b": 1, "c": 2, "f": 3, "p": 4, "x": 5}
# The component size of each format, by table (enums/VertexFormat.py:43-80).
CHANNEL = {0: "f", 1: "e", 2: "B", 3: "B", 4: "I"}
V2017 = {0: "f", 1: "e", 2: "B", 3: "B", 4: "b", 5: "H", 6: "h", 7: "B", 8: "b", 9: "H", 10: "h", 11: "I", 12: "i"}
V2019 = {0: "f", 1: "e", 2: "B", 3: "b", 4: "H", 5: "h", 6: "B", 7: "b", 8: "H", 9: "h", 10: "I", 11: "i"}
SIZE = {"f": 4, "e": 2, "B": 1, "b": 1, "H": 2, "h": 2, "I": 4, "i": 4}


def vtuple(version: str) -> list[int]:
    from UnityPy.helpers.UnityVersion import UnityVersion

    return list(UnityVersion.from_str(version).as_tuple())


def tree(class_id: int, version: str) -> list[list]:
    """Class 43's or 213's tree at `version`, from `TREES`."""
    return TREES(class_id, tuple(vtuple(version)))


def table(version: tuple) -> dict:
    return CHANNEL if version[0] < 2017 else V2017 if version[0] < 2019 else V2019


def sizes(version: tuple) -> dict:
    return {f: SIZE[c] for f, c in table(version).items()}


class Obj:
    """A Mesh (class 43) or a Sprite (213) at a version, its mesh part at `at`: the root, or the Sprite's m_RD."""

    def __init__(self, source: str, version: str, rows=None):
        self.source = source
        self.cls = 43 if source == "mesh" else 213
        self.at = () if source == "mesh" else ("m_RD",)
        self.version = vtuple(version)
        self.rows = rows if rows is not None else tree(self.cls, version)
        self.value = default_value(to_node(self.rows))

    def part(self) -> dict:
        v = self.value
        for name in self.at:
            v = v[name]
        return v

    def elem(self, *path):
        return element(self.rows, *self.at, *path)

    def set(self, **fields):
        part = self.part()
        part.update(fit(part, fields))
        return self

    def vertex_data(self, count: int, channels: dict, data: bytes, n: int | None = None, **extra):
        """m_VertexData of `count` vertices: channels {index: (stream, offset, format, dimension)} among `n`."""
        n = n if n is not None else (14 if self.version[0] >= 2018 else 8)
        vd = self.part()["m_VertexData"]
        fields = {"m_VertexCount": count, "m_DataSize": data, **extra}
        if "m_Channels" in vd:
            template = self.elem("m_VertexData", "m_Channels")
            names = ("stream", "offset", "format", "dimension")
            chans = [{k: v for k, v in zip(names, channels.get(i, (0, 0, 0, 0))) if k in template} for i in range(n)]
            fields["m_Channels"] = [fit(template, c) for c in chans]
        vd.update(fit(vd, fields))
        return self

    def submeshes(self, *spans):
        """SubMeshes: (firstByte, indexCount, topology) each, `None` topology for a SubMesh without the field."""
        template = self.elem("m_SubMeshes")
        subs = []
        for first, count, topology in spans:
            values = {"firstByte": first, "indexCount": count, "topology": topology}
            subs.append(fit(template, {k: v for k, v in values.items() if k in template and v is not None}))
        return self.set(m_SubMeshes=subs)

    def indices(self, values, code="H"):
        return self.set(m_IndexBuffer=list(struct.pack(f"<{len(values)}{code}", *values)))

    def case(self, name: str, version=None, endian="<", **extra) -> dict:
        return {
            "name": name,
            "source": self.source,
            "version": list(version) if version is not None else self.version,
            "rows": self.rows,
            "data": write(self.rows, self.value),
            "endian": endian,
            **extra,
        }


def mesh(version="2019.4.0f1", rows=None) -> Obj:
    return Obj("mesh", version, rows)


def sprite(version="2019.4.0f1", rows=None) -> Obj:
    return Obj("sprite", version, rows)


def positions(o: Obj, count: int, fmt: int, dim: int, seed: int, chn: int = 0) -> Obj:
    """`count` vertices of one channel, `chn`, in format `fmt`, from seeded bytes, laid out as derived."""
    size = sizes(o.version)[fmt]
    data = splitmix(seed, count * dim * size)
    return o.vertex_data(count, {chn: (0, 0, fmt, dim)}, data + bytes(-len(data) % 16))


def tri(o: Obj, count: int = 3) -> Obj:
    return o.submeshes((0, count, 0)).indices(list(range(count)))


# --- the cases


def format_cases() -> list[dict]:
    """Every format of each table, at the versions that pick it, dimension 4 then 3 (the stride of 3)."""
    out = []
    for version in ("5.6.7f1", "2016.4.0f1", "2017.1.0f3", "2018.4.0f1", "2019.4.0f1", "6000.0.0f1"):
        for fmt in table(vtuple(version)):
            for dim in (4, 3):
                o = tri(positions(mesh(version), 5, fmt, dim, seed=fmt * 7 + dim + vtuple(version)[0]))
                out.append(o.case(f"format_{version}_{fmt}_d{dim}"))
        bad = max(table(vtuple(version))) + 1
        out.append(tri(positions(mesh(version), 3, 0, 3, 1).vertex_data(3, {0: (0, 0, bad, 3)}, bytes(48))).case(
            f"format_{version}_invalid_{bad}"))  # fmt: skip
        out.append(tri(positions(mesh(version), 3, 0, 3, 1).vertex_data(3, {0: (0, 0, 255, 3)}, bytes(48))).case(
            f"format_{version}_invalid_255"))  # fmt: skip
    return out


HALVES = [0x0000, 0x8000, 0x0001, 0x8001, 0x03FF, 0x0400, 0x3C00, 0xBC00, 0x3555, 0x7BFF, 0xFBFF, 0x7C00, 0xFC00,
          0x7C01, 0xFC01, 0x7E00, 0xFE00, 0x7FFF, 0xFFFF, 0x3BFF]  # fmt: skip
FLOATS = [0x00000000, 0x80000000, 0x00000001, 0x807FFFFF, 0x00800000, 0x3F800000, 0x7F7FFFFF, 0xFF7FFFFF, 0x7F800000,
          0xFF800000, 0x7F800001, 0xFFA00005, 0x7FC00000, 0xFFC00001, 0x7FFFFFFF, 0x3EAAAAAB]  # fmt: skip


def bit_cases() -> list[dict]:
    """Half and float bit patterns: zeros, subnormals, extremes, infinities, NaNs with payloads."""
    halves = struct.pack(f"<{len(HALVES)}H", *HALVES)
    floats = struct.pack(f"<{len(FLOATS)}I", *FLOATS)
    out = [
        tri(mesh().vertex_data(5, {0: (0, 0, 1, 4)}, halves + bytes(-len(halves) % 16))).case("bits_half"),
        tri(mesh().vertex_data(4, {0: (0, 0, 0, 4)}, floats)).case("bits_float"),
        tri(mesh("5.6.7f1").vertex_data(5, {0: (0, 0, 1, 4)}, halves + bytes(-len(halves) % 16))).case("bits_half_5"),
    ]
    for endian in ("<", ">"):
        o = tri(mesh().vertex_data(4, {0: (0, 0, 0, 4), 1: (1, 0, 1, 4), 2: (2, 0, 10, 2), 3: (3, 0, 5, 2)},
                                   floats + halves[:32] + splitmix(3, 32) + splitmix(4, 16)))  # fmt: skip
        out.append(o.case(f"bits_endian_{'little' if endian == '<' else 'big'}", endian=endian))
    return out


def stream_cases() -> list[dict]:
    """Derived streams: several, with gaps, strides of several channels, unaligned counts, masked dimensions."""
    out = []
    v = vtuple("2019.4.0f1")
    rows = {0: ("f", [(i, -i, 0.5 * i) for i in range(5)]), 1: ("e", [(1.0, 0.0, -1.0, 0.5)] * 5),
            3: ("B", [(i, 128, 0, 255) for i in range(5)]), 4: ("f", [(0.25 * i, 0.75) for i in range(5)]),
            12: ("H", [(i, 2 * i) for i in range(5)]), 13: ("I", [(7,)] * 5)}  # fmt: skip
    chans = {0: (0, 0, 0, 3), 1: (0, 12, 1, 4), 3: (1, 0, 2, 4), 4: (1, 4, 0, 2), 12: (2, 0, 8, 2), 13: (2, 4, 10, 1)}
    data = lay_out(5, chans, sizes(v), rows)
    out.append(tri(mesh().vertex_data(5, chans, data)).case("streams_three"))
    # Streams 0 and 1 empty: stream 2 starts at 0.
    gap = {0: (2, 0, 0, 3)}
    out.append(tri(mesh().vertex_data(3, gap, lay_out(3, gap, sizes(v), {0: ("f", [(1, 2, 3)] * 3)}))).case("streams_gap"))
    # A dimension whose low four bits are 3 (19), and one of 16, whose low bits are 0.
    for dim, name in ((19, "masked"), (16, "zero_low_bits"), (0x34, "of_52")):
        out.append(tri(mesh().vertex_data(3, {0: (0, 0, 0, dim), 1: (0, 0, 0, 3)}, splitmix(dim, 96))).case(f"streams_dim_{name}"))
    # A channel offset past its stream's derived stride, read within the data, then past it.
    out.append(tri(mesh().vertex_data(3, {0: (0, 8, 0, 1)}, splitmix(5, 20))).case("streams_offset_in"))
    out.append(tri(mesh().vertex_data(3, {0: (0, 8, 0, 1)}, splitmix(5, 19))).case("streams_offset_one_short"))
    out.append(tri(mesh().vertex_data(3, {0: (0, 255, 0, 1)}, splitmix(5, 16))).case("streams_offset_out"))
    # Data shorter than the layout by one byte, and longer.
    out.append(tri(mesh().vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(6, 47))).case("streams_short"))
    out.append(tri(mesh().vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(6, 99))).case("streams_long"))
    # A stream of 255 offsets: the highest stream a UInt8 names.
    out.append(tri(mesh().vertex_data(1, {0: (255, 0, 0, 1)}, splitmix(7, 16))).case("streams_255"))
    # Count 0, and empty data: nothing read.
    out.append(tri(mesh().vertex_data(0, {0: (0, 0, 0, 3)}, splitmix(8, 48))).case("streams_count_0"))
    out.append(tri(mesh().vertex_data(3, {0: (0, 0, 0, 3)}, b"")).case("streams_no_data"))
    # Positions absent: the count comes out 0 (the compressed mesh's gate resets it).
    out.append(tri(mesh().vertex_data(3, {4: (0, 0, 0, 2)}, splitmix(9, 32))).case("streams_no_positions"))
    # No channels at all: max() of nothing.
    out.append(tri(mesh().vertex_data(3, {}, splitmix(9, 32), n=0)).case("streams_no_channels"))
    # A bigger count: 300 vertices of three channels.
    big = {0: (0, 0, 0, 3), 1: (0, 12, 0, 3), 4: (1, 0, 1, 2)}
    out.append(tri(mesh().vertex_data(300, big, splitmix(10, 300 * 24 + 300 * 4 + 16))).case("streams_300"))
    return out


def channel_cases() -> list[dict]:
    """Channel assignment by version, an unknown channel, and pre-2018 colour forced to four components."""
    out = []
    for version, n in (("2018.4.0f1", 14), ("5.6.7f1", 8), ("2017.4.0f1", 8)):
        chans = {i: (0, 4 * i, 0, 1) for i in range(n)}
        out.append(tri(mesh(version).vertex_data(3, chans, splitmix(n, 3 * 4 * n))).case(f"channels_all_{version}"))
        extra = {**chans, n: (0, 4 * n, 0, 1)}
        out.append(tri(mesh(version).vertex_data(3, extra, splitmix(n, 3 * 4 * (n + 1)), n=n + 1)).case(
            f"channels_unknown_{version}"))  # fmt: skip
    # Colour (2) of format 2: below 2018 read as four bytes whatever its dimension; from 2018 channel 2 is tangents.
    for version in ("5.6.7f1", "2017.4.0f1", "2018.1.0f2"):
        for dim in (1, 3, 4):
            chans = {0: (0, 0, 0, 3), 2: (0, 12, 2, dim)}
            out.append(tri(mesh(version).vertex_data(3, chans, splitmix(dim, 64))).case(f"colour_{version}_d{dim}"))
    # The forced colour at the data's end reads past it.
    out.append(tri(mesh("5.6.7f1").vertex_data(4, {2: (0, 0, 2, 1)}, splitmix(2, 16))).case("colour_past_end"))
    out.append(tri(mesh("5.6.7f1").vertex_data(4, {2: (0, 0, 2, 1)}, splitmix(2, 6))).case("colour_short"))
    return out


def own_stream_cases() -> list[dict]:
    """4.x: the file's own streams, their masks and strides; a stream past the list."""
    out = []
    o = mesh("4.7.2f1")
    stream = o.elem("m_VertexData", "m_Streams")

    def streams(*specs):
        return [fit(stream, {"channelMask": m, "offset": off, "stride": st}) for m, off, st in specs]

    chans = {0: (0, 0, 0, 3), 1: (0, 12, 0, 3), 2: (1, 0, 2, 4), 3: (1, 4, 0, 2), 5: (1, 12, 0, 4)}
    data = splitmix(11, 3 * 24 + 3 * 28 + 16)
    for name, mask0, mask1 in (("all", 0b11, 0b101100), ("mask_skips", 0b01, 0b100100), ("mask_b", 0b1, 0b0)):
        o = tri(mesh("4.7.2f1").vertex_data(3, chans, data, n=6, m_Streams=streams((mask0, 0, 24), (mask1, 72, 28))))
        out.append(o.case(f"own_{name}"))
    # Channel 5 past a mask of 0b100: bin(4)[::-1] is "001b0", index 5 raises IndexError.
    o = tri(mesh("4.7.2f1").vertex_data(3, chans, data, n=6, m_Streams=streams((0b11, 0, 24), (0b100, 72, 28))))
    out.append(o.case("own_mask_index_error"))
    # A channel naming stream 2 of two.
    o = mesh("4.7.2f1").vertex_data(3, {0: (2, 0, 0, 3)}, data, n=6, m_Streams=streams((1, 0, 12), (0, 0, 0)))
    out.append(tri(o).case("own_stream_index_error"))
    # A format VertexChannelFormat lacks, and a stride of 255 reading past the data.
    o = mesh("4.7.2f1").vertex_data(3, {0: (0, 0, 7, 3)}, data, n=6, m_Streams=streams((1, 0, 12)))
    out.append(tri(o).case("own_invalid_format"))
    o = mesh("4.7.2f1").vertex_data(3, {0: (0, 0, 0, 3)}, data, n=6, m_Streams=streams((1, 0, 255)))
    out.append(tri(o).case("own_stride_past_end"))
    # No m_Streams in 4.x's tree: the assert.
    rows = drop(tree(43, "4.7.2f1"), "m_VertexData", "m_Streams")
    out.append(tri(mesh("4.7.2f1", rows).vertex_data(3, {0: (0, 0, 0, 3)}, data, n=6)).case("own_no_streams"))
    return out


def fixed_stream_cases() -> list[dict]:
    """Below 4: the four fixed streams (get_channels, P4), and one missing (the assert); a 3.4 Mesh without
    m_VertexData (the assert)."""
    o = mesh("3.5.0f5")
    vd = o.part()["m_VertexData"]
    vd.update(m_VertexCount=3, m_DataSize=splitmix(12, 36))
    vd["m_Streams[0]"].update(channelMask=1, offset=0, stride=12)
    out = [tri(o).case("fixed_streams")]
    rows = drop(tree(43, "3.5.0f5"), "m_VertexData", "m_Streams[2]")
    out.append(tri(mesh("3.5.0f5", rows)).case("fixed_stream_missing"))
    out.append(tri(mesh("3.4.0f5")).case("no_vertex_data_3_4"))
    return out


def index_cases() -> list[dict]:
    """The index size by version and field, and index buffers that do not unpack."""
    out = []
    pos = {0: (0, 0, 0, 3)}

    def m(version, fmt=None, compression=0, rows=None, **fields):
        o = mesh(version, rows).vertex_data(4, pos, splitmix(13, 48))
        if fmt is not None:
            o.set(m_IndexFormat=fmt)
        return o.set(m_MeshCompression=compression, **fields)

    for version in ("2017.1.0f3", "2017.3.0f3", "2017.3.0b1", "2017.4.0f1", "2017.4.0a1", "2019.4.0f1"):
        for fmt in (0, 1, 2) if version != "2017.1.0f3" else (None,):
            for compression in (0, 1):
                o = m(version, fmt, compression)
                o.submeshes((0, 6, 0)).indices([0, 1, 2, 2, 1, 3], "I" if fmt == 1 else "H")
                out.append(o.case(f"index_{version}_f{fmt}_c{compression}"))
    # 2017.3's rule reads the handler's version, not the tree's: a 2017.4 tree under a 2017.3 handler.
    o = m("2017.4.0f1", 1, 1).submeshes((0, 3, 0)).indices([0, 1, 2], "I")
    out.append(o.case("index_tree_2017_4_handler_2017_3", version=(2017, 3, 1, 4, 1)))
    # No m_IndexFormat from 2017.4: None == 0 is False, so 32-bit.
    rows = drop(tree(43, "2019.4.0f1"), "m_IndexFormat")
    out.append(m("2019.4.0f1", rows=rows).submeshes((0, 3, 0)).indices([0, 1, 2], "I").case("index_no_format"))
    # m_Use16BitIndices decides first: 0 is 32-bit, 2 is 16-bit.
    rows = add(tree(43, "2019.4.0f1"), (), leaf("int", "m_Use16BitIndices"))
    for use, code in ((0, "I"), (2, "H"), (-1, "H")):
        o = m("2019.4.0f1", 1 if code == "H" else 0, rows=rows, m_Use16BitIndices=use).submeshes((0, 3, 0))
        out.append(o.indices([0, 1, 2], code).case(f"index_use16_{use}"))
    # Buffers that do not unpack: an odd length for 16-bit, 6 bytes for 32-bit, one byte.
    for name, fmt, raw in (("odd", 0, [1, 0, 2, 0, 3]), ("six", 1, [1, 0, 0, 0, 2, 0]), ("one", 0, [1])):
        out.append(m("2019.4.0f1", fmt).submeshes((0, 1, 0)).set(m_IndexBuffer=raw).case(f"index_unpack_{name}"))
    # An empty buffer: no index buffer, so get_triangles' assert.
    out.append(m("2019.4.0f1", 0).submeshes((0, 3, 0)).case("index_empty"))
    # Indices up to 0xFFFF and 0xFFFFFFFF.
    o = m("2019.4.0f1", 1).submeshes((0, 3, 0)).indices([0xFFFFFFFF, 0x80000000, 7], "I")
    out.append(o.case("index_32_high"))
    out.append(m("2019.4.0f1", 0).submeshes((0, 3, 0)).indices([0xFFFF, 0x8000, 7]).case("index_16_high"))
    return out


def topology_cases() -> list[dict]:
    """Every topology: lists exact, short and past the buffer, strips with degenerates and odd winding, quads,
    lines and points, values no member has, and a SubMesh without `topology`."""
    out = []
    idx = [0, 1, 2, 2, 1, 3, 3, 3, 4, 4, 5, 6, 7, 8, 9, 10]

    def m(*spans, indices=idx, fmt=0, rows=None):
        o = mesh("2019.4.0f1", rows).vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(14, 48)).set(m_IndexFormat=fmt)
        return o.submeshes(*spans).indices(indices, "I" if fmt else "H")

    spans = {
        "list": [(0, 6, 0)], "list_count_4": [(0, 4, 0)], "list_cut_1": [(20, 7, 0)], "list_cut_2": [(18, 9, 0)],
        "list_past_end": [(0, 25, 0)], "list_first_past_end": [(40, 6, 0)], "list_empty": [(0, 0, 0)],
        "list_odd_first_byte": [(3, 3, 0)], "list_huge": [(0, 3 * (1 << 20), 0)],
        "strip": [(0, 16, 1)], "strip_3": [(0, 3, 1)], "strip_2": [(0, 2, 1)], "strip_1": [(0, 1, 1)],
        "strip_0": [(0, 0, 1)], "strip_from_4": [(8, 8, 1)], "strip_past_end_0": [(32, 4, 1)],
        "strip_past_end_1": [(30, 3, 1)], "strip_past_end_2": [(0, 17, 1)], "strip_first_past_end": [(40, 3, 1)],
        "strip_to_end": [(20, 6, 1)],
        "quads": [(0, 16, 2)], "quads_4": [(4, 4, 2)], "quads_count_5": [(0, 5, 2)], "quads_count_6": [(0, 6, 2)],
        "quads_count_7": [(0, 7, 2)], "quads_past_end": [(16, 12, 2)], "quads_past_end_2": [(28, 4, 2)],
        "quads_past_end_3": [(26, 8, 2)], "quads_past_end_count_5": [(24, 5, 2)],
        "lines": [(0, 4, 3)], "line_strip": [(0, 4, 4)], "points": [(0, 4, 5)], "topology_6": [(0, 4, 6)],
        "topology_minus_1": [(0, 4, -1)], "mixed": [(0, 6, 0), (12, 6, 1), (0, 8, 2)],
        "second_fails": [(0, 6, 0), (0, 4, 3)], "no_submeshes": [],
    }  # fmt: skip
    for name, s in spans.items():
        out.append(m(*s).case(f"topology_{name}"))
    out.append(m((0, 6, 0), (12, 6, 1), indices=idx, fmt=1).case("topology_32bit"))
    out.append(m((24, 4, 0), (8, 5, 1), indices=idx, fmt=1).case("topology_32bit_first_byte"))
    rows = drop(tree(43, "2019.4.0f1"), "m_SubMeshes", "Array", "data", "topology")
    out.append(m((0, 6, None), rows=rows).case("topology_missing"))
    return out


def skin_cases() -> list[dict]:
    """m_Skin up to 2018.1: bone indices and weights, then 2018.1's channels 12 and 13 in their place."""
    out = []
    for version in ("2017.4.0f1", "2018.1.0f2", "5.6.7f1"):
        o = mesh(version)
        bone = o.elem("m_Skin")
        weights = [1.0, 0.5, struct.unpack("<f", struct.pack("<I", 0x7F800001))[0], -0.0]
        skin = [fit(bone, {**{f"weight[{j}]": weights[(i + j) % 4] for j in range(4)},
                           **{f"boneIndex[{j}]": i * 4 + j - 1 for j in range(4)}}) for i in range(3)]  # fmt: skip
        chans = {0: (0, 0, 0, 3)}
        if version.startswith("2018"):
            chans |= {12: (0, 12, 0, 2), 13: (0, 20, 2, 4)}
        o.vertex_data(3, chans, splitmix(15, 96)).set(m_Skin=skin)
        out.append(tri(o).case(f"skin_{version}"))
    # m_Skin's other element type (classes/generated.py:4633), the same fields: no version claim.
    o = mesh("2017.4.0f1", retype(tree(43, "2017.4.0f1"), ("m_Skin", "Array", "data"), "BoneInfluence"))
    bone = o.elem("m_Skin")
    skin = [fit(bone, {**{f"weight[{j}]": 0.25 * j for j in range(4)}, **{f"boneIndex[{j}]": i + j for j in range(4)}})
            for i in range(3)]  # fmt: skip
    out.append(tri(o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(40, 48)).set(m_Skin=skin)).case("skin_bone_influence"))
    return out


def stream_data_cases() -> list[dict]:
    """m_StreamData: the vertex data through get_resource_data, with a reader, without one, and an empty path."""
    out = []
    stored = splitmix(16, 48)
    for version, offset in (("2019.4.0f1", 7), ("6000.0.0f1", 1 << 40)):
        o = mesh(version).vertex_data(4, {0: (0, 0, 0, 3)}, b"")
        o.set(m_StreamData=fit(o.part()["m_StreamData"], {"offset": offset, "size": 48, "path": "archive:/CAB-x/CAB-x.resS"}))
        out.append(tri(o).case(f"stream_data_{version}", resource=stored, reader=True))
        out.append(tri(o).case(f"stream_data_{version}_no_reader", resource=stored, reader=False))
    # The largest offset the 2020 `UInt64` holds, as the resource call records it.
    o = mesh("6000.0.0f1").vertex_data(4, {0: (0, 0, 0, 3)}, b"")
    o.set(m_StreamData=fit(o.part()["m_StreamData"], {"offset": (1 << 64) - 1, "size": 48, "path": "archive:/CAB-x/CAB-x.resS"}))
    out.append(tri(o).case("stream_data_6000.0.0f1_max_offset", resource=stored, reader=True))
    o = mesh().vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(17, 48))
    o.set(m_StreamData=fit(o.part()["m_StreamData"], {"offset": 3, "size": 9, "path": ""}))
    out.append(tri(o).case("stream_data_empty_path", resource=stored, reader=False))
    # The stream's bytes replace m_DataSize even when it holds data; a short resource reads past its end.
    o = mesh().vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(18, 48))
    o.set(m_StreamData=fit(o.part()["m_StreamData"], {"offset": 0, "size": 20, "path": "a.resS"}))
    out.append(tri(o).case("stream_data_short", resource=stored[:20], reader=True))
    return out


def compressed_cases() -> list[dict]:
    """The compressed mesh's gates, in UnityPy's order: each P2a array with items, alone."""
    out = []
    for version, names in (("2019.4.0f1", ["m_Vertices", "m_UV", "m_Normals", "m_Tangents", "m_FloatColors",
                                           "m_Weights", "m_Triangles", "m_Colors", "m_BindPoses", "m_NormalSigns"]),
                           ("4.7.2f1", ["m_BindPoses", "m_Colors", "m_FloatColors"])):  # fmt: skip
        for name in names:
            o = mesh(version)
            cm = o.part()["m_CompressedMesh"]
            if name not in cm:
                continue
            cm[name] = {**cm[name], "m_NumItems": 2, "m_Data": [1, 2, 3]}
            if version.startswith("4"):
                stream = o.elem("m_VertexData", "m_Streams")
                o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(19, 36), n=6,
                              m_Streams=[fit(stream, {"channelMask": 1, "offset": 0, "stride": 12})])  # fmt: skip
            else:
                o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(19, 48))
            out.append(tri(o).case(f"compressed_{version}_{name}"))
    out += compressed_order_cases() + uv_info_cases()
    return out


# `decompress_compressed_mesh`'s arrays in the order it tests them (helpers/MeshHelper.py:468-628), by version.
ORDER = {
    "2019.4.0f1": ["m_Vertices", "m_UV", "m_Normals", "m_Tangents", "m_FloatColors", "m_Weights", "m_Triangles"],
    "4.7.2f1": ["m_Vertices", "m_UV", "m_BindPoses", "m_Normals", "m_Tangents", "m_Weights", "m_Triangles",
                "m_Colors"],
}  # fmt: skip


def compressed_mesh_case(version: str, held: list[str], uv_info: int | None = None, rows=None) -> Obj:
    """A Mesh of `version` whose compressed arrays `held` have items, with `m_UVInfo` when given."""
    o = mesh(version, rows)
    cm = o.part()["m_CompressedMesh"]
    for name in held:
        cm[name] = {**cm[name], "m_NumItems": 2, "m_Data": [1, 2, 3]}
    if uv_info is not None:
        cm["m_UVInfo"] = uv_info
    if version.startswith("4"):
        stream = o.elem("m_VertexData", "m_Streams")
        o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(19, 36), n=6,
                      m_Streams=[fit(stream, {"channelMask": 1, "offset": 0, "stride": 12})])  # fmt: skip
    else:
        o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(19, 48))
    return tri(o)


def compressed_order_cases() -> list[dict]:
    """Every array from one on held, for each in order: the first is every array held, and each refusal names the
    first `decompress_compressed_mesh` would unpack (m_BindPoses at 4.x)."""
    return [
        compressed_mesh_case(version, names[k:]).case(f"compressed_{version}_from_{names[k]}")
        for version, names in ORDER.items()
        for k in range(len(names))
    ]


def uv_info_cases() -> list[dict]:
    """`m_UV` with items under each `m_UVInfo` (helpers/MeshHelper.py:482-506): 0 unpacks two channels; otherwise
    only a channel whose nibble has `kUVChannelExists` (4) unpacks, so 0x3, 0x30 and 0xB3B3B3B3 unpack nothing; an
    `UnknownObject` compressed mesh without `m_UVInfo` raises."""
    out = [
        compressed_mesh_case("2019.4.0f1", ["m_UV"], info).case(f"compressed_uv_info_{info:#x}")
        for info in (0x3, 0x4, 0x30, 0x40, 0x7000_0000, 0xB3B3_B3B3)
    ]
    cm = ("m_CompressedMesh",)
    unknown = add(tree(43, "2019.4.0f1"), cm, leaf("int", "m_Extra"))
    out.append(compressed_mesh_case("2019.4.0f1", ["m_UV"], 0x3, rows=unknown).case("compressed_uv_info_unknown"))
    rows = add(drop(tree(43, "2019.4.0f1"), *cm, "m_UVInfo"), cm, leaf("int", "m_Extra"))
    out.append(compressed_mesh_case("2019.4.0f1", ["m_UV"], rows=rows).case("compressed_uv_info_missing"))
    rows = drop(tree(43, "2019.4.0f1"), *cm, "m_UVInfo")
    out.append(compressed_mesh_case("2019.4.0f1", ["m_UV"], rows=rows).case("compressed_uv_info_none"))
    return out


def scale_cases() -> list[dict]:
    """N channels of dimension 0 at 4.x (each skipped), N submeshes of no indices, and N fields beside a Mesh's own,
    for N of 4, 32 and 256: tests/work.rs measures the work each counts."""
    out = []
    for n in (4, 32, 256):
        o = mesh("4.7.2f1")
        stream = o.elem("m_VertexData", "m_Streams")
        o.vertex_data(1, {}, splitmix(28, 16), n=n, m_Streams=[fit(stream, {"channelMask": 1, "offset": 0, "stride": 12})])
        out.append(tri(o).case(f"scale_channels_{n}"))
        o = positions(mesh(), 3, 0, 3, 29).submeshes(*[(0, 0, 0)] * n).indices([0, 1, 2])
        out.append(o.case(f"scale_submeshes_{n}"))
        rows = tree(43, "2019.4.0f1")
        for i in range(n):
            rows = add(rows, (), leaf("int", f"m_Extra{i}"))
        out.append(tri(positions(mesh(rows=rows), 3, 0, 3, 30)).case(f"scale_fields_{n}"))
    return out


def element_field_cases() -> list[dict]:
    """N elements of M fields more each, N and M of 4 and 32: channels of dimension 0 at 4.x (each skipped) and
    submeshes of no indices. tests/work.rs pins what an element counts apart from its fields' scans."""
    out = []
    for n in (4, 32):
        for m in (4, 32):
            extra = [leaf("UInt8", f"m_Extra{i}") for i in range(m)]
            rows = tree(43, "4.7.2f1")
            for field in extra:
                rows = add(rows, ("m_VertexData", "m_Channels", "Array", "data"), field)
            o = mesh("4.7.2f1", rows)
            stream = o.elem("m_VertexData", "m_Streams")
            o.vertex_data(1, {}, splitmix(33, 16), n=n, m_Streams=[fit(stream, {"channelMask": 1, "offset": 0, "stride": 12})])
            out.append(tri(o).case(f"scale_channels_{n}_fields_{m}"))
            rows = tree(43, "2019.4.0f1")
            for field in extra:
                rows = add(rows, ("m_SubMeshes", "Array", "data"), field)
            o = positions(mesh(rows=rows), 3, 0, 3, 34).submeshes(*[(0, 0, 0)] * n).indices([0, 1, 2])
            out.append(o.case(f"scale_submeshes_{n}_fields_{m}"))
    return out


def big_cases() -> list[dict]:
    """Arrays of 4 KiB or more each, past tests/charges.rs's slack: an index buffer of 1,026 indices read as a list, a
    strip and quads, and a skin of 256 bones below 2018.2 beside 256 vertices' positions."""
    out = []
    idx = [i % 1000 for i in range(1026)]
    for name, topology, count in (("list", 0, 1026), ("strip", 1, 1026), ("quads", 2, 1024)):
        o = mesh().vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(31, 48))
        out.append(o.submeshes((0, count, topology)).indices(idx).case(f"big_{name}"))
    o = mesh("2017.4.0f1")
    bone = o.elem("m_Skin")
    skin = [fit(bone, {**{f"weight[{j}]": 0.25 * j for j in range(4)},
                       **{f"boneIndex[{j}]": (4 * i + j) % 256 for j in range(4)}}) for i in range(256)]  # fmt: skip
    o.vertex_data(256, {0: (0, 0, 0, 3)}, splitmix(32, 256 * 12)).set(m_Skin=skin)
    out.append(tri(o).case("big_skin"))
    return out


def sprite_cases() -> list[dict]:
    """Sprites: render data with m_IndexBuffer at 5.6 and later, `indices` (a 4.x field) in a later tree,
    `vertices`, a Sprite before 5.6 (no m_VertexData), and object mode's UnknownObjects."""
    out = []
    for version in ("5.6.0f3", "2019.4.0f1", "2022.3.0f1"):
        o = sprite(version)
        o.vertex_data(4, {0: (0, 0, 0, 3), 4: (0, 12, 0, 2)}, splitmix(20, 80))
        out.append(o.submeshes((0, 6, 0)).indices([0, 1, 2, 2, 1, 3]).case(f"sprite_{version}"))
    out.append(sprite("5.5.0f3").case("sprite_5_5"))
    rd = ("m_RD",)
    rows = add(tree(213, "2019.4.0f1"), rd, vec("indices", leaf("UInt16", "data")))
    for name, values in (("indices", [0, 1, 2]), ("indices_wide", [0, 1, 300])):
        o = sprite(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(21, 48)).submeshes((0, 3, 0))
        out.append(o.set(indices=values).case(f"sprite_{name}"))
    o = sprite(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(21, 48)).submeshes((0, 3, 0))
    out.append(o.set(indices=[0, 1, 2]).indices([2, 1, 0]).case("sprite_index_buffer_first"))
    # 5.5's `vertices` field in a 2019.4 render data (a tree no Unity version writes).
    vertex = tree(213, "5.5.0f3")
    i = next(k for k, r in enumerate(vertex) if r[2] == "vertices")
    rows = add(tree(213, "2019.4.0f1"), rd, [[r[0] - vertex[i][0], *r[1:]] for r in vertex[i : end(vertex, i)]])
    o = sprite(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(22, 48)).submeshes((0, 3, 0)).indices([0, 1, 2])
    template = o.elem("vertices")
    out.append(o.set(vertices=[template] * 2).case("sprite_vertices"))
    out.append(sprite(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(22, 48)).case("sprite_vertices_empty"))
    # An extra field: the Sprite stays a Sprite; its render data becomes an UnknownObject.
    rows = add(tree(213, "2019.4.0f1"), (), leaf("int", "m_Extra"))
    out.append(tri(sprite(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(23, 48))).case("sprite_extra_field"))
    rows = add(tree(213, "2019.4.0f1"), rd, leaf("int", "m_Extra"))
    out.append(tri(sprite(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(23, 48))).case("sprite_rd_unknown"))
    # A Sprite missing m_Name: an UnknownObject, whose m_RD is read as it is.
    rows = drop(tree(213, "2019.4.0f1"), "m_Name")
    out.append(tri(sprite(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(23, 48))).case("sprite_unknown"))
    rows = drop(drop(tree(213, "2019.4.0f1"), "m_Name"), "m_RD")
    out.append(sprite(rows=rows).case("sprite_unknown_no_rd"))
    return out


def object_cases() -> list[dict]:
    """Object mode: a Mesh missing a required field (UnknownObject: Unknown mesh type), one with an extra field
    (still a Mesh), and nested classes that become UnknownObjects with and without the fields read."""
    out = []
    base = tree(43, "2019.4.0f1")

    def m(rows, count=3, chans=None):
        return tri(mesh(rows=rows).vertex_data(count, chans or {0: (0, 0, 0, 3)}, splitmix(24, 48)))

    out.append(m(drop(base, "m_Name")).case("object_mesh_unknown"))
    out.append(m(add(base, (), leaf("int", "m_Extra"))).case("object_mesh_extra"))
    vd = ("m_VertexData",)
    out.append(m(add(base, vd, leaf("int", "m_Extra"))).case("object_vertex_data_unknown"))
    out.append(m(add(drop(base, *vd, "m_Channels"), vd, leaf("int", "m_Extra"))).case("object_vertex_data_no_channels"))
    out.append(m(drop(base, *vd, "m_Channels")).case("object_no_channels"))
    ch = ("m_VertexData", "m_Channels", "Array", "data")
    out.append(m(add(base, ch, leaf("int", "m_Extra"))).case("object_channel_unknown"))
    for field in ("format", "dimension", "stream", "offset"):
        out.append(m(drop(base, *ch, field)).case(f"object_channel_no_{field}"))
    sub = ("m_SubMeshes", "Array", "data")
    out.append(m(add(base, sub, leaf("int", "m_Extra"))).case("object_submesh_unknown"))
    out.append(m(drop(base, *sub, "firstByte")).case("object_submesh_no_first_byte"))
    out.append(m(drop(add(base, sub, leaf("int", "m_Extra")), *sub, "topology")).case("object_submesh_unknown_no_topology"))
    out.append(m(add(base, ("m_StreamData",), leaf("int", "m_Extra"))).case("object_stream_data_unknown"))
    cm = ("m_CompressedMesh",)
    out.append(m(add(base, cm, leaf("int", "m_Extra"))).case("object_compressed_unknown"))
    out.append(m(add(base, ("m_CompressedMesh", "m_Vertices"), leaf("int", "m_Extra"))).case("object_packed_unknown"))
    # 4.x streams that are UnknownObjects, one without `stride`.
    s = ("m_VertexData", "m_Streams", "Array", "data")
    for name, rows in (("unknown", add(tree(43, "4.7.2f1"), s, leaf("int", "m_Extra"))),
                       ("no_stride", drop(add(tree(43, "4.7.2f1"), s, leaf("int", "m_Extra")), *s, "stride"))):  # fmt: skip
        o = mesh("4.7.2f1", rows)
        stream = o.elem("m_VertexData", "m_Streams")
        o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(25, 36), n=6,
                      m_Streams=[fit(stream, {"channelMask": 1, "offset": 0, **({"stride": 12} if "stride" in stream else {})})])  # fmt: skip
        out.append(tri(o).case(f"object_stream_{name}"))
    # A Mesh's own vertex array beside m_VertexData (a tree no Unity version writes): copy_from_mesh copies it,
    # colours by a loop of their own (helpers/MeshHelper.py:221-224).
    vector3 = [[0, "Vector3f", "data", 0], [1, "float", "x", 0], [1, "float", "y", 0], [1, "float", "z", 0]]
    rgba = [[0, "ColorRGBA", "data", 0], *[[1, "float", c, 0] for c in "rgba"]]
    for name, elem, value in (("m_Vertices", vector3, {"x": 1.0, "y": 2.0, "z": 3.0}),
                              ("m_UV1", vector3, {"x": 1.0, "y": 2.0, "z": 3.0}),
                              ("m_Colors", rgba, {"r": 1.0, "g": 0.5, "b": 0.0, "a": 1.0})):  # fmt: skip
        o = tri(mesh(rows=add(base, (), vec(name, elem))).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(24, 48)))
        out.append(o.set(**{name: [value]}).case(f"object_own_{name}"))
    for name, elem in (("m_Vertices", vector3), ("m_Colors", rgba)):
        o = tri(mesh(rows=add(base, (), vec(name, elem))).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(24, 48)))
        out.append(o.case(f"object_own_empty_{name}"))
    # A Mesh whose m_IndexBuffer is a vector of UInt16 (wider than any version's), values past a byte.
    rows = retype(base, ("m_IndexBuffer", "Array", "data"), "UInt16")
    o = mesh(rows=rows).vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(26, 48)).submeshes((0, 1, 0))
    out.append(o.set(m_IndexBuffer=[1, 0]).case("object_index_u16_bytes"))
    return out


def version_cases() -> list[dict]:
    """The handler's version: a type byte no UnityVersionType has, and `u` (255), which is one."""
    o = tri(positions(mesh(), 3, 0, 3, 27))
    return [
        o.case("version_type_6", version=(2019, 4, 0, 6, 1)),
        o.case("version_type_u", version=(2019, 4, 0, 255, 0)),
        o.case("version_tree_2019_handler_2018", version=(2018, 4, 0, 3, 1)),
    ]


def array_scale_cases() -> list[dict]:
    """Arrays of N, for N of 4, 32 and 256, the rest of each case the same: an index buffer of N indices under one list,
    one strip and one quads submesh; N indices under one strip whose every step is degenerate; N bones at 2017.4; one
    channel of N vertices at 2019.4, of floats and of ints; and N channels of dimension 0 at 2019.4. tests/work.rs pins
    the work each array's loop counts."""
    out = []
    for n in (4, 32, 256):
        o = mesh().vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(35, 48))
        out.append(o.submeshes((0, n, 0), (0, n, 1), (0, n, 2)).indices([i % 4 for i in range(n)]).case(f"scale_indices_{n}"))
        o = mesh().vertex_data(4, {0: (0, 0, 0, 3)}, splitmix(35, 48))
        # Each index twice in a row, so every step's three repeat one: the strip drops every one.
        degenerate = o.submeshes((0, n, 1)).indices([i // 2 % 4 for i in range(n)])
        out.append(degenerate.case(f"scale_strip_degenerate_{n}"))
        o = mesh("2017.4.0f1")
        bone = o.elem("m_Skin")
        skin = [fit(bone, {**{f"weight[{j}]": 0.25 for j in range(4)}, **{f"boneIndex[{j}]": j for j in range(4)}})
                for _ in range(n)]  # fmt: skip
        out.append(tri(o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(36, 48)).set(m_Skin=skin)).case(f"scale_bones_{n}"))
        out.append(tri(positions(mesh(), n, 0, 3, 37)).case(f"scale_vertices_float_{n}"))
        out.append(tri(positions(mesh(), n, 6, 4, 38)).case(f"scale_vertices_int_{n}"))
        out.append(tri(mesh().vertex_data(1, {}, splitmix(39, 16), n=n)).case(f"scale_channels_2019_{n}"))
    return out


GROUPS = (format_cases, bit_cases, stream_cases, channel_cases, own_stream_cases, fixed_stream_cases, index_cases,
          topology_cases, skin_cases, stream_data_cases, compressed_cases, sprite_cases, object_cases, version_cases,
          scale_cases, element_field_cases, big_cases, array_scale_cases)  # fmt: skip
# What is being built or run, for meshdecode.py --tpk's failure line (a builder's name, then a case's).
AT = ""


def cases() -> list[dict]:
    global AT
    out = []
    for group in GROUPS:
        AT = group.__name__
        out += group()
    return out
