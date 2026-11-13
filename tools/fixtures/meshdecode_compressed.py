"""The compressed-mesh cases of tools/fixtures/meshdecode.py, not a fixture script: Meshes whose `m_CompressedMesh`
arrays hold items, which `decompress_compressed_mesh` (helpers/MeshHelper.py:468-628) unpacks through
`PackedBitVector`'s `unpack_ints` and `unpack_floats` (helpers/PackedBitVector.py:20-85), laid out by
meshdecode_cases.py's `mesh` on the tree `meshdecode_cases.TREES` gives (so `meshdecode.py --tpk` runs them on the
TPK's trees too).

Each vector is packed here from its items, least significant bit first, never by UnityPy: every bit size from 1 to
32 for integers and floats, bit size 0, a vector one byte short, start offsets (the UV channels), floats whose range
or start is a NaN or an infinity (each NaN a single operand, so its result is the same on every host), the
normals' and tangents' branches and short pairs, the skin's weights in 31sts with a fourth weight, a vertex past the
count and an exhausted index list, packed colours, bind poses cut short, a missing `m_BitSize` or `m_Range`, an
`UnknownObject` vector, a bit size past 32 (which unityrs refuses), and vectors of N items for the work tests. The
cases where PARITY.md section 19.1's two Tier C rows show (the UV offset, the fourth bone weight) are recorded twice
by meshdecode.py: UnityPy's answer and the corrected one. Every float is exact in binary32, and every square
`normalize` takes exact, so no case depends on the host's `pow` (crates/unityrs-meshdecode/src/compressed.rs).
Run on its own it only imports (tools/parity/tests/test_fixture_modules.py).
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import meshdecode_cases as mc  # noqa: E402
from meshdecode_build import add, drop, fit, leaf, splitmix  # noqa: E402

CM = ("m_CompressedMesh",)


def f32(bits: int) -> float:
    """The binary32 of `bits` as a Python float."""
    return struct.unpack("<f", struct.pack("<I", bits))[0]


def ints(values, bits: int, data_len: int | None = None) -> dict:
    """A PackedBitVector of integers, `bits` bits an item, least significant first; `data_len` cuts `m_Data`."""
    total = sum(v << (i * bits) for i, v in enumerate(values)) if bits else 0
    data = list(total.to_bytes((len(values) * bits + 7) // 8, "little"))
    return {"m_NumItems": len(values), "m_Data": data[:data_len], "m_BitSize": bits}


def floats(quanta, bits: int, start: float = 0.0, step: float = 1.0, value_range: float | None = None, **k) -> dict:
    """A PackedBitVector of floats: item i is `start + quanta[i] * range / (2**bits - 1)`, the range `step` a
    quantum unless given."""
    rng = step * ((1 << bits) - 1) if value_range is None else value_range
    return {**ints(quanta, bits, **k), "m_Range": rng, "m_Start": start}


def seeded(seed: int, n: int, bits: int) -> list[int]:
    """`n` items of `bits` bits from seeded bytes."""
    raw = splitmix(seed, 8 * n)
    return [int.from_bytes(raw[8 * i : 8 * i + 8], "little") & ((1 << bits) - 1) for i in range(n)]


def case(name: str, version: str = "2019.4.0f1", rows=None, submesh: int = 3, **fields) -> dict:
    """A Mesh of `version` whose compressed arrays are `fields` (a vector's dict, or `m_UVInfo`'s int), beside
    meshdecode_cases' three vertices of vertex data and one triangle list of `submesh` indices."""
    o = mc.mesh(version, rows)
    cm = o.part()["m_CompressedMesh"]
    for key, value in fields.items():
        cm[key] = fit(cm[key], value) if isinstance(value, dict) else value
    if version.startswith("4"):
        stream = o.elem("m_VertexData", "m_Streams")
        o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(19, 36), n=6,
                      m_Streams=[fit(stream, {"channelMask": 1, "offset": 0, "stride": 12})])  # fmt: skip
    else:
        o.vertex_data(3, {0: (0, 0, 0, 3)}, splitmix(19, 48))
    o.submeshes((0, submesh, 0)).indices(list(range(3)))
    return o.case(f"compressed_{name}")


def values_cases() -> list[dict]:
    """Every array at once, as test_rows_meshes.py's compressed_and_old_meshes builds them; the 4.x arrays; UV
    channels of each dimension; bind poses cut short."""
    out = [
        case(
            "values",
            m_Vertices=floats([1, 2, 3, 4, 5, 6], 8),
            m_UV=floats(list(range(10, 22)), 8),
            m_UVInfo=0x555,  # channels 0, 1 and 2, two components each: the third diverges (Tier C)
            m_Normals=floats([2, 2, 4, 4], 8, step=0.25),
            m_NormalSigns=ints([0, 1], 1),
            m_Tangents=floats([2, 2, 4, 4], 8, step=0.25),
            m_TangentSigns=ints([1, 0, 0, 1], 1),
            m_FloatColors=floats(list(range(8)), 8),
            m_Weights=ints([31, 10, 10, 10], 5),  # the second vertex's fourth weight diverges (Tier C)
            m_BoneIndices=ints([7, 1, 2, 3, 4], 4),
            m_Triangles=ints([1, 0, 1], 2),
        ),
        case(
            "4.7.2f1_values",
            "4.7.2f1",
            m_Vertices=floats(list(range(6)), 8),
            m_UV=floats(list(range(10, 18)), 8),  # no m_UVInfo below 5: channel 0, then 1 at 4 items a vertex
            m_BindPoses=floats(list(range(16)), 8),
            m_Colors=ints([0x11223344, 0xFF00FF00], 32),
        ),
        case("4.7.2f1_bind_poses_cut", "4.7.2f1", m_BindPoses=floats(list(range(18)), 8, step=0.5)),
        case("uv_plain_short", m_Vertices=floats(list(range(6)), 8), m_UV=floats(list(range(10, 16)), 8), m_UVInfo=0),
        # Channels 0, 2, 5 and 7 of 1, 3, 4 and 2 components: 5 and 7 diverge.
        case("uv_dims", m_Vertices=floats(list(range(6)), 8), m_UV=floats(list(range(20)), 8, start=-2.0),
             m_UVInfo=0x4 | 0x6 << 8 | 0x7 << 20 | 0x5 << 28),  # fmt: skip
        # Ten items for three channels of two: UnityPy's third reads items 4 to 7, the corrected one runs out.
        case("uv_short", m_Vertices=floats(list(range(6)), 8), m_UV=floats(list(range(10)), 8), m_UVInfo=0x555),
    ]
    return out


def packed_cases() -> list[dict]:
    """unpack_ints and unpack_floats at each bit size, at 0, one byte short, NaN and infinite scales, and past 32."""
    out = []
    for bits in (1, 3, 5, 7, 8, 9, 13, 16, 17, 24, 31, 32):
        out.append(case(f"ints_{bits}", m_Triangles=ints(seeded(bits, 6, bits), bits), submesh=6))
    for bits in (1, 7, 8, 13, 24, 32):
        quanta = seeded(100 + bits, 9, bits)
        out.append(case(f"floats_{bits}", m_Vertices=floats(quanta, bits, start=f32(0xC1A80000), value_range=f32(0x4295999A))))
    out += [
        case("floats_0", m_Vertices=floats([5, 6, 7, 8, 9, 10], 0, start=2.5)),
        case("floats_short", m_Vertices=floats([1, 2, 3, 4, 5, 6], 8, data_len=5)),
        case("ints_short", m_Triangles=ints([1, 2, 3], 13, data_len=4)),
        case("floats_nan_range", m_Vertices=floats([1, 2, 3, 4, 5, 6], 8, start=1.0, value_range=f32(0x7FC01234))),
        case("floats_nan_start", m_Vertices=floats([1, 2, 3], 0, start=f32(0xFFC00042))),
        case("floats_inf_range", m_Vertices=floats([1, 2, 3, 4, 5, 6], 8, start=-3.0, value_range=f32(0xFF800000))),
        # UnityPy unpacks it; unityrs refuses an item past 32 bits (Malformed).
        case("ints_33", m_Triangles=ints([1, (1 << 33) - 1, 5], 33)),
    ]
    # The division first: `x * (range / 255...)`, not `(x * range) / ...` (PARITY.md section 15.3).
    out.append(case("divides_first", m_Vertices=floats(seeded(7, 30, 24), 24, start=f32(0xC13B3333), value_range=f32(0x42153333))))
    return out


def normal_cases() -> list[dict]:
    """The normals' and tangents' branches, and where `zip` stops or a pair is cut short."""
    three = floats(list(range(9)), 8)  # three vertices
    quarter = {"step": 0.25}
    return [
        # (1, 0): z 0, so -0.0 for sign 0; (0, 0): z 1; (1, 1): normalised; (0.5, 0.5) beside it.
        case("normals_branches", m_Vertices=floats(list(range(12)), 8), m_Normals=floats([4, 0, 0, 0, 4, 4, 2, 2], 8, **quarter),
             m_NormalSigns=ints([0, 0, 0, 1], 1)),  # fmt: skip
        case("normals_odd", m_Vertices=three, m_Normals=floats([1, 1, 2, 2, 3], 8, **quarter), m_NormalSigns=ints([1, 1, 1], 1)),
        case("normals_signs_short", m_Vertices=three, m_Normals=floats([1, 1, 2, 2, 3, 3], 8, **quarter),
             m_NormalSigns=ints([0, 1], 1)),  # fmt: skip
        case("normals_count_short", m_Vertices=floats([0, 0, 0], 8), m_Normals=floats([1, 1, 2, 2], 8, **quarter),
             m_NormalSigns=ints([0, 0], 1)),  # fmt: skip
        case("normals_nan", m_Vertices=three, m_Normals=floats([1, 2, 3, 4], 8, value_range=f32(0x7FC00007)),
             m_NormalSigns=ints([0, 1], 1)),  # fmt: skip
        case("tangents_branches", m_Vertices=floats(list(range(12)), 8), m_Tangents=floats([4, 0, 0, 0, 4, 4, 2, 2], 8, **quarter),
             m_TangentSigns=ints([0, 0, 1, 1, 0, 1, 1, 0], 1)),  # fmt: skip
        case("tangents_signs_short", m_Vertices=three, m_Tangents=floats([1, 1, 2, 2], 8, **quarter),
             m_TangentSigns=ints([0, 1, 1], 1)),  # fmt: skip
        case("tangents_odd", m_Vertices=three, m_Tangents=floats([1, 1, 2], 8, **quarter), m_TangentSigns=ints([1, 1, 1, 1], 1)),
        case("tangents_nan", m_Vertices=three, m_Tangents=floats([1, 2], 8, value_range=f32(0xFFC00009)),
             m_TangentSigns=ints([0, 0], 1)),  # fmt: skip
    ]


def skin_cases() -> list[dict]:
    """Weights in 31sts: one, two and three to a vertex, a fourth from three short of 31 (Tier C), and the loop's
    ends."""
    five = floats(list(range(15)), 8)
    return [
        case("skin_weights", m_Vertices=five, m_Weights=ints([31, 15, 16, 10, 10, 5, 0, 0, 0, 20, 5], 5),
             m_BoneIndices=ints(list(range(1, 15)), 4)),  # fmt: skip
        case("skin_past_vertices", m_Vertices=floats([0, 0, 0], 8), m_Weights=ints([31, 31], 5),
             m_BoneIndices=ints([1, 2], 4)),  # fmt: skip
        case("skin_stop", m_Vertices=five, m_Weights=ints([10, 10, 10], 5), m_BoneIndices=ints([1, 2, 3], 4)),
        case("skin_indices_short", m_Vertices=five, m_Weights=ints([31, 31, 31], 5), m_BoneIndices=ints([1], 4)),
    ]


def field_cases() -> list[dict]:
    """A vector without `m_BitSize` or `m_Range` (None, the assert), and one built as an `UnknownObject`."""
    tree = mc.tree(43, "2019.4.0f1")
    vertices = floats([1, 2, 3], 8)
    plain = {k: v for k, v in vertices.items() if k != "m_BitSize"}
    unscaled = {k: v for k, v in vertices.items() if k != "m_Range"}
    unknown = add(drop(tree, *CM, "m_Vertices", "m_BitSize"), (*CM, "m_Vertices"), leaf("int", "m_Extra"))
    return [
        case("no_bit_size", rows=drop(tree, *CM, "m_Vertices", "m_BitSize"), m_Vertices=plain),
        case("no_range", rows=drop(tree, *CM, "m_Vertices", "m_Range"), m_Vertices=unscaled),
        case("ints_no_bit_size", rows=drop(tree, *CM, "m_Triangles", "m_BitSize"),
             m_Triangles={"m_NumItems": 2, "m_Data": [1]}),  # fmt: skip
        case("unknown_vector", rows=unknown, m_Vertices=plain),
    ]


def scale_cases() -> list[dict]:
    """N vertices' positions, N normals and N signs, for N of 4, 32 and 256: tests/work.rs pins the units."""
    return [
        case(f"scale_{n}", m_Vertices=floats(seeded(n, 3 * n, 8), 8), m_Normals=floats(seeded(n + 1, 2 * n, 6), 6, step=1 / 64),
             m_NormalSigns=ints(seeded(n + 2, n, 1), 1))  # fmt: skip
        for n in (4, 32, 256)
    ]


def decode_cases() -> list[dict]:
    return values_cases() + packed_cases() + normal_cases() + skin_cases() + field_cases() + scale_cases()
