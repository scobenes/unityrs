"""Oracle fixtures for unityrs-texture's ASTC LDR decoder (P3, PR T2b).

Runs in the amd64 oracle image (docs/plans/rest.md decision 4) against UnityPy 1.25.2, Pillow 9.5.0 and
astc-encoder-py 0.1.12 and writes crates/unityrs-texture/tests/fixtures/astc.json, one case a line, which
`cargo test -p unityrs-texture` replays without Python (tests/astc_oracle.rs, and src/astc/oracle_tests.rs for the
footprints no TextureFormat names). Every input is built here in code; no file is read.

    scripts/dev python -E -s tools/fixtures/texture_astc.py           # write the fixture
    scripts/dev python -E -s tools/fixtures/texture_astc.py --check   # regenerate and compare

**format_cases.** Textures of the twelve ASTC LDR formats (`ASTC_RGB_NxN` and `ASTC_RGBA_NxN`), decoded by
`Texture2DConverter.get_image_from_texture2d` as tools/fixtures/texture.py's decode.json cases are (its `case`), so
through `parse_image_data`'s padding to whole blocks, `Texture2DConverter.astc` (export/Texture2DConverter.py:356-366)
with its LDR, `USE_DECODE_UNORM8` context (export/Texture2DConverter.py:369-387), the crop and the flip. They cover
blocks astcenc's encoder made from synthetic images at sizes from 1x1 to several blocks with cut edges, SplitMix64
blocks, data that is long (a mip chain), one byte short, one byte and empty, sides of 0, negative sides, an Xbox 360
and an unswizzled Switch texture, and Pillow's limits behind astcenc: a width whose padded row passes Pillow's raw
decoder's cap (67,108,857 at 4x4, padded to 67,108,860: `MemoryError` after astcenc has decoded the zeros, about
1 GB of output) and its crop's decompression-bomb check (59 x 3,033,169, about 0.7 GB), each also with one byte, where
UnityPy's own data check raises first.

A negative side that padding leaves below 0 (a side of -N and below for blocks of N) reaches astc-encoder-py as a
negative size: with one side so, it raises `ASTCError`; with both, UnityPy's data check passes once the data holds the
product's blocks, and astcenc then reads and writes past its buffers (the process dies of `SIGSEGV`). So no case has
both sides below 0 after padding with that much data; unityrs refuses every negative side (PARITY.md section 19.3).

**footprint_cases.** `Texture2DConverter.astc` called directly, as UnityPy's function, at each of astcenc's fourteen
2D footprints (4x4 to 12x12, astcenc_percentile_tables.cpp:1201-1226 at astcenc commit 631fb04), on sizes that are
not whole blocks, so astcenc's own store cuts each edge block: no padding and no flip. Per footprint: three images
astcenc's encoder made (a smooth RGBA ramp, an opaque one, and noisy tiles, which take partitions), 128 SplitMix64
blocks, and crafted blocks, each a SplitMix64 block with its header set, drawn until `classify` gives it the tags
wanted: every weight-grid layout of the block mode table the footprint takes, single and dual plane; every weight
precision; one to four partitions; the second plane on each channel; each of the 16 endpoint modes (the HDR ones are
magenta endpoints in the LDR profile) and mixed modes; LDR void-extent blocks with ordered and all-ones extents; and
each error block: a reserved block mode, a weight count or bit count out of range, a grid larger than the footprint
(where one exists), four partitions with two planes, more than 18 colour integers, a colour precision below
`QUANT_6`, a void extent with its reserved bits clear, one with empty extents, and an FP16 (HDR) void extent. Each
crafted block is decoded alone too, in both builds below: a block `classify` calls an error must decode to the error
colour of each, and one it calls valid must decode the same in both, so the tags are checked against astcenc as they
are made. `coverage` records, per footprint, every tag of its cases' blocks, and `scalar_sha256` each case's image as
the scalar build decodes it.

**Builds.** astc-encoder-py ships astcenc built for several x86-64 instruction sets and loads the best the CPU has, and
they differ in one thing: an error block (NaN in astcenc's float pipeline) is magenta, (255, 0, 255, 255), in the
scalar, SSE2 and SSE4.1 builds, as astcenc_image.cpp:423-432 means it, and white, (255, 255, 255, 255), in the AVX2
build, whose NaN test is an ordered compare, false for NaN (astcenc_vecmathlib_avx2_8.h:645-648), and whose
`min(NaN, 1.0)` is 1.0 (astcenc_vecmathlib_avx2_8.h:687-700). UnityPy's oracle runs on CPUs with AVX2, so every case
is recorded with UnityPy's converter on the AVX2 build (`decoding_with`), whichever build this host would pick, and
the script refuses to run where the CPU cannot load it; `scalar_sha256` records the scalar build's image, which
unityrs's tests replay too (PARITY.md section 19.3). The encoder is the scalar build (`astc_encoder._encoder_none`),
so the encoded blocks are the same on every host.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib
import sys
from pathlib import Path
from types import SimpleNamespace

import astc_encoder
import PIL
import UnityPy
from astc_encoder import ASTCConfigFlags, ASTCProfile, ASTCType
from astc_encoder import _encoder_none as scalar
from UnityPy.enums import TextureFormat
from UnityPy.export import Texture2DConverter

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-texture/tests/fixtures"
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import texture  # noqa: E402

TF = TextureFormat
FOOTPRINTS = [
    (4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6), (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10),
    (12, 12),
]  # fmt: skip
SIDES = [4, 5, 6, 8, 10, 12]
# An error block's colour in each build (the module docs).
WHITE = bytes([255, 255, 255, 255])
MAGENTA = bytes([255, 0, 255, 255])
# (bits, radix) of each weight and colour precision, QUANT_2 to QUANT_256 (astcenc_integer_sequence.cpp:352-374).
BTQ = [
    (1, 1), (0, 3), (2, 1), (0, 5), (1, 3), (3, 1), (1, 5), (2, 3), (4, 1), (2, 5), (3, 3), (5, 1), (3, 5), (4, 3),
    (6, 1), (4, 5), (5, 3), (7, 1), (5, 5), (6, 3), (8, 1),
]  # fmt: skip
HDR_CEMS = (2, 3, 7, 11, 14, 15)


class Rng:
    """SplitMix64 outputs, from texture.py's generator."""

    def __init__(self, seed: int):
        self.seed, self.n = seed, 0

    def next(self) -> int:
        self.n += 1
        return int.from_bytes(texture.splitmix(self.seed, 8 * self.n)[-8:], "little")

    def below(self, n: int) -> int:
        return self.next() % n

    def take(self, n: int) -> bytearray:
        return bytearray(b"".join(self.next().to_bytes(8, "little") for _ in range((n + 7) // 8))[:n])


def ise_bits(count: int, quant: int) -> int:
    """astcenc's `get_ise_sequence_bitcount` (astcenc_integer_sequence.cpp:394-433)."""
    bits, radix = BTQ[quant]
    extra = (8 * count + 4) // 5 if radix == 3 else (7 * count + 2) // 3 if radix == 5 else 0
    return count * bits + extra


def mode_fields(mode: int):
    """The block mode table (astcenc_block_sizes.cpp:36-137): (layout, x, y, dual, quant), or (layout, None, ...)
    for a reserved mode. The layout names the table's row: `lo0` to `lo3b` for nonzero low bits, `hi0` to `hi3`."""
    a, base = (mode >> 5) & 3, (mode >> 4) & 1
    high, dual = (mode >> 9) & 1, (mode >> 10) & 1
    if mode & 3:
        base |= (mode & 3) << 1
        b, row = (mode >> 7) & 3, (mode >> 2) & 3
        if row == 0:
            layout, x, y = "lo0", b + 4, a + 2
        elif row == 1:
            layout, x, y = "lo1", b + 8, a + 2
        elif row == 2:
            layout, x, y = "lo2", a + 2, b + 8
        elif mode & 0x100:
            layout, x, y = "lo3a", (b & 1) + 2, a + 2
        else:
            layout, x, y = "lo3b", a + 2, (b & 1) + 6
    else:
        base |= ((mode >> 2) & 3) << 1
        if (mode >> 2) & 3 == 0:
            return ("reserved", None, None, None, None)
        b, row = (mode >> 9) & 3, (mode >> 7) & 3
        if row == 0:
            layout, x, y = "hi0", 12, a + 2
        elif row == 1:
            layout, x, y = "hi1", a + 2, 12
        elif row == 2:
            layout, x, y = "hi2", a + 6, b + 6
            dual = high = 0
        elif a < 2:
            layout, x, y = "hi3", (6, 10)[a], (10, 6)[a]
        else:
            return ("reserved", None, None, None, None)
    return (layout, x, y, dual, base - 2 + 6 * high)


def bits(block: bytes, offset: int, count: int) -> int:
    return (int.from_bytes(block, "little") >> offset) & ((1 << count) - 1)


def put(block: bytearray, offset: int, count: int, value: int) -> None:
    whole = int.from_bytes(block, "little")
    mask = ((1 << count) - 1) << offset
    block[:] = ((whole & ~mask) | ((value << offset) & mask)).to_bytes(16, "little")


def classify(block: bytes, bw: int, bh: int) -> list[str]:
    """The tags of a block at footprint `bw` x `bh`, as astcenc's `physical_to_symbolic` reads it
    (astcenc_symbolic_physical.cpp:291-544): an `error:` tag for each error block, else its layout, precisions,
    partitions, endpoint modes and second-plane channel."""
    mode = bits(block, 0, 11)
    if mode & 0x1FF == 0x1FC:
        if bits(block, 10, 2) != 3:
            return ["error:void_reserved"]
        extents = [bits(block, 12, 13), bits(block, 25, 13), bits(block, 38, 13), bits(block, 51, 13)]
        ones = all(v == 0x1FFF for v in extents)
        if (extents[0] >= extents[1] or extents[2] >= extents[3]) and not ones:
            return ["error:void_extent"]
        if mode & 0x200:
            return ["error:void_hdr"]
        return ["void:allones" if ones else "void:ldr"]
    layout, x, y, dual, quant = mode_fields(mode)
    if x is None:
        return ["error:reserved"]
    count = x * y * (dual + 1)
    weight_bits = ise_bits(count, quant)
    if count > 64 or not 24 <= weight_bits <= 96:
        return ["error:bits"]
    if x > bw or y > bh:
        return ["error:grid"]
    tags = [f"layout:{layout}", f"quant:{quant}", "dual" if dual else "single"]
    parts = bits(block, 11, 2) + 1
    tags.append(f"partitions:{parts}")
    if dual and parts == 4:
        return [*tags, "error:dual4"]
    below, high = 128 - weight_bits, 0
    if parts == 1:
        cems = [bits(block, 13, 4)]
    else:
        high = 3 * parts - 4
        below -= high
        encoded = bits(block, 23, 6) | (bits(block, below, high) << 6)
        kind = encoded & 3
        if kind == 0:
            cems = [(encoded >> 2) & 15] * parts
            below += high
            high = 0
        else:
            cems = [
                ((((encoded >> (2 + i)) & 1) + kind - 1) << 2) | ((encoded >> (2 + parts + 2 * i)) & 3)
                for i in range(parts)
            ]
            tags.append("cem:mixed")
    integers = sum(((c >> 2) + 1) * 2 for c in cems)
    if integers > 18:
        return [*tags, "error:colors"]
    room = max(0, (111 if parts == 1 else 99) - weight_bits - high - (2 if dual else 0))
    if not any(ise_bits(integers, q) <= room for q in range(4, 21)):
        return [*tags, "error:color_quant"]
    tags += sorted({f"cem:{c}" for c in cems})
    if any(c in HDR_CEMS for c in cems):
        tags.append("cem:hdr")
    if dual:
        tags.append(f"plane2:{bits(block, below - 2, 2)}")
    return [*tags, "valid"]


def legal(bw: int, bh: int) -> list[int]:
    """The block modes whose grid fits the footprint and decodes (classify's valid layouts)."""
    out = []
    for mode in range(2048):
        layout, x, y, dual, quant = mode_fields(mode)
        if mode & 0x1FF == 0x1FC or x is None or x > bw or y > bh:
            continue
        count = x * y * (dual + 1)
        if count <= 64 and 24 <= ise_bits(count, quant) <= 96:
            out.append(mode)
    return out


def build(name: str):
    """astc-encoder-py's `name` build, as the `astc_encoder` UnityPy's converter calls, or None where the host's CPU
    cannot load it."""
    try:
        mod = importlib.import_module(f"astc_encoder.{name}")
    except ImportError:
        return None
    classes = ("ASTCConfig", "ASTCContext", "ASTCError", "ASTCImage", "ASTCSwizzle")
    enums = ("ASTCConfigFlags", "ASTCProfile", "ASTCType")
    return SimpleNamespace(
        **{c: getattr(mod, c) for c in classes}, **{e: getattr(astc_encoder, e) for e in enums}
    )


@contextlib.contextmanager
def decoding_with(namespace):
    """UnityPy's `astc` and `get_astc_context` (export/Texture2DConverter.py:356-387) on the build `namespace`."""
    saved = Texture2DConverter.astc_encoder
    Texture2DConverter.astc_encoder = namespace
    Texture2DConverter._get_astc_context.cache_clear()
    try:
        yield
    finally:
        Texture2DConverter.astc_encoder = saved
        Texture2DConverter._get_astc_context.cache_clear()


AVX2 = build("_encoder_avx2")
SCALAR = build("_encoder_none")


def astc_outcome(data: bytes, w: int, h: int, block: tuple) -> dict:
    """UnityPy's `Texture2DConverter.astc` on the data, as texture.py's `outcome` records an image or exception."""
    try:
        image = Texture2DConverter.astc(data, w, h, block)
    except Exception as e:  # noqa: BLE001 - every exception is an outcome to record
        return {"raises": type(e).__name__, "message": str(e)}
    pixels = image.tobytes()
    out = {"mode": image.mode, "size": list(image.size), "sha256": hashlib.sha256(pixels).hexdigest()}
    if w * h <= texture.HEX_PIXELS:
        out["pixels"] = pixels.hex()
    return out


def alone(block: bytes, bw: int, bh: int) -> bytes:
    return Texture2DConverter.astc(bytes(block), bw, bh, (bw, bh)).tobytes()


def scalar_decode(data: bytes, w: int, h: int, block: tuple) -> bytes:
    with decoding_with(SCALAR):
        return Texture2DConverter.astc(data, w, h, block).tobytes()


def craft(name: str, bw: int, bh: int, target: list[str], patch) -> bytes:
    """A SplitMix64 block, `patch`ed, redrawn until `classify` gives it every tag of `target` and its decode alone
    agrees: an error is white in the AVX2 build and magenta in the scalar one; any other block decodes the same in
    both, and not as all white or all magenta when it has no HDR endpoint mode."""
    rng = Rng(texture.seed_of(name))
    for _ in range(4000):
        block = rng.take(16)
        patch(block, rng)
        tags = classify(bytes(block), bw, bh)
        if not all(t in tags for t in target):
            continue
        pixels, other = alone(block, bw, bh), scalar_decode(bytes(block), bw, bh, (bw, bh))
        if any(t.startswith("error:") for t in tags):
            assert (pixels, other) == (WHITE * (bw * bh), MAGENTA * (bw * bh)), (name, tags)
        else:
            assert pixels == other, (name, tags)
            if "cem:hdr" not in tags and pixels in (WHITE * (bw * bh), MAGENTA * (bw * bh)):
                continue
        return bytes(block)
    raise AssertionError(f"no block for {name}: {target}")


def crafted(bw: int, bh: int) -> list[bytes]:
    """The crafted blocks of one footprint (the module docs' list)."""
    modes = legal(bw, bh)
    single = [m for m in modes if not (m >> 10) & 1 or mode_fields(m)[0] == "hi2"]
    dual = [m for m in modes if m not in single]

    def with_mode(pool, parts=None, cem=None):
        def patch(block, rng):
            put(block, 0, 11, pool[rng.below(len(pool))])
            if parts is not None:
                put(block, 11, 2, parts - 1)
            if cem is not None:
                put(block, 13, 4, cem)

        return patch

    def void(kind):
        def patch(block, rng):
            put(block, 0, 9, 0x1FC)
            put(block, 9, 1, kind == "hdr")
            put(block, 10, 2, 0 if kind == "reserved" else 3)
            if kind in ("allones", "hdr"):
                put(block, 12, 52, (1 << 52) - 1)
            elif kind == "ldr":
                lo_s, lo_t = rng.below(4096), rng.below(4096)
                put(block, 12, 13, lo_s)
                put(block, 25, 13, lo_s + 1 + rng.below(4095))
                put(block, 38, 13, lo_t)
                put(block, 51, 13, lo_t + 1 + rng.below(4095))
            elif kind == "extent":
                put(block, 12, 13, 0x1000 + rng.below(4096))
                put(block, 25, 13, rng.below(4096))

        return patch

    def bad(tag):
        """The non-void modes `classify` gives only the error `tag`."""
        return [
            m
            for m in range(2048)
            if m & 0x1FF != 0x1FC and classify(m.to_bytes(2, "little") + bytes(14), bw, bh) == [f"error:{tag}"]
        ]

    targets = []

    def add(target, pool, parts=None, cem=None):
        if pool:
            targets.append((target, with_mode(pool, parts, cem)))

    for layout in sorted({mode_fields(m)[0] for m in modes}):
        add([f"layout:{layout}", "single", "valid"], [m for m in single if mode_fields(m)[0] == layout], 1)
        add([f"layout:{layout}", "dual", "valid"], [m for m in dual if mode_fields(m)[0] == layout], 1)
    for q in range(12):
        add([f"quant:{q}", "valid"], [m for m in modes if mode_fields(m)[4] == q], 1)
    for p in range(1, 5):
        add([f"partitions:{p}", "single", "valid"], single, p)
    for c in range(4):
        add([f"plane2:{c}", "valid"], dual, 1)
    for p in (2, 3):
        add([f"partitions:{p}", "dual", "valid"], dual, p)
    for cem in range(16):
        add([f"cem:{cem}", "partitions:1", "valid"], single, 1, cem)
    add(["cem:mixed", "valid"], single)
    add(["cem:mixed", "cem:hdr", "valid"], single)
    for kind in ("ldr", "allones"):
        targets.append(([f"void:{kind}"], void(kind)))
    for kind in ("hdr", "reserved", "extent"):
        targets.append(([f"error:void_{kind}"], void(kind)))
    for kind in ("reserved", "bits", "grid"):
        add([f"error:{kind}"], bad(kind))
    add(["error:dual4"], dual, 4)
    add(["error:colors"], single, 4)
    add(["error:color_quant"], [m for m in modes if weight_bits(m) >= 80], 4)
    return [craft(f"astc-{bw}x{bh}-{'+'.join(t)}", bw, bh, t, patch) for t, patch in targets]


def weight_bits(mode: int) -> int:
    """The bits a valid block mode's weights take."""
    _, x, y, dual, quant = mode_fields(mode)
    return ise_bits(x * y * (dual + 1), quant)


def encode(rgba: bytes, w: int, h: int, bw: int, bh: int) -> bytes:
    """astcenc's encoder, scalar build, at UnityPy's settings: LDR, quality 100, `USE_DECODE_UNORM8`."""
    config = scalar.ASTCConfig(ASTCProfile.LDR, bw, bh, 1, 100, ASTCConfigFlags.USE_DECODE_UNORM8)
    image = scalar.ASTCImage(ASTCType.U8, w, h, 1, rgba)
    return scalar.ASTCContext(config).compress(image, scalar.ASTCSwizzle.from_str("RGBA"))


def ramp(w: int, h: int, alpha: bool) -> bytes:
    out = bytearray()
    for y in range(h):
        for x in range(w):
            a = (x * 255 // max(1, w - 1)) if alpha else 255
            out += bytes([x * 37 % 256, y * 23 % 256, (x * y * 5 + 40) % 256, a])
    return bytes(out)


def tiles(w: int, h: int, seed: int) -> bytes:
    """Tiles of 3x2 pixels of a SplitMix64 colour each, with a little noise: blocks with edges, which take
    partitions."""
    rng, colours = Rng(seed), {}
    out = bytearray()
    for y in range(h):
        for x in range(w):
            key = (x // 3, y // 2)
            if key not in colours:
                colours[key] = rng.take(4)
            out += bytes((v + rng.below(9)) % 256 for v in colours[key])
    return bytes(out)


def footprint_cases() -> list[dict]:
    out = []
    for bw, bh in FOOTPRINTS:
        tag = f"{bw}x{bh}"
        images = {
            "ramp": (2 * bw + 3, 2 * bh + 1, ramp(2 * bw + 3, 2 * bh + 1, True)),
            "opaque": (2 * bw - 1, bh + 1, ramp(2 * bw - 1, bh + 1, False)),
            "tiles": (3 * bw - 1, 2 * bh - 1, tiles(3 * bw - 1, 2 * bh - 1, texture.seed_of(f"tiles-{tag}"))),
        }
        specs = []
        for name, (w, h, rgba) in images.items():
            specs.append((f"astc-{tag}-{name}", w, h, {"hex": encode(rgba, w, h, bw, bh).hex()}))
        name = f"astc-{tag}-seeded"
        specs.append((name, 16 * bw - 1, 8 * bh - 2, {"seed": texture.seed_of(name), "len": 128 * 16}))
        blocks = crafted(bw, bh)
        rows = -(-len(blocks) // 8)
        # Eight blocks a row, the last row filled from the first blocks again.
        blocks += blocks[: 8 * rows - len(blocks)]
        specs.append((f"astc-{tag}-crafted", 8 * bw - 3, rows * bh - 1, {"hex": b"".join(blocks).hex()}))
        for name, w, h, spec in specs:
            data = texture.build(spec)
            out.append(
                {
                    "id": name,
                    "block": [bw, bh],
                    "width": w,
                    "height": h,
                    "data": spec,
                    "data_sha256": hashlib.sha256(data).hexdigest(),
                    **astc_outcome(data, w, h, (bw, bh)),
                    "scalar_sha256": hashlib.sha256(scalar_decode(data, w, h, (bw, bh))).hexdigest(),
                }
            )
    return out


def coverage(cases: list[dict]) -> dict[str, list[str]]:
    out: dict[str, set] = {}
    for c in cases:
        bw, bh = c["block"]
        data = texture.build(c["data"])
        tags = out.setdefault(f"{bw}x{bh}", set())
        for at in range(0, len(data) - 15, 16):
            tags.update(classify(data[at : at + 16], bw, bh))
    return {k: sorted(v) for k, v in out.items()}


def format_cases() -> list[dict]:
    out = []
    for i, side in enumerate(SIDES):
        for fmt in (TF(TF.ASTC_RGB_4x4 + i), TF(TF.ASTC_RGBA_4x4 + i)):
            alpha = "RGBA" in fmt.name

            def needed(w, h):
                return -(-w // side) * -(-h // side) * 16

            for w, h in ((1, 1), (side, side), (2 * side + 1, side + 2), (17, 33)):
                name = f"{fmt.name}-{w}x{h}-encoded"
                pw, ph = -(-w // side) * side, -(-h // side) * side
                data = encode(ramp(pw, ph, alpha), pw, ph, side, side)
                out.append(texture.case(name, fmt, w, h, {"hex": data.hex()}))
            name = f"{fmt.name}-31x33-seeded"
            out.append(texture.case(name, fmt, 31, 33, texture.seeded(name, needed(31, 33))))
            w, h = 7, 9
            need = needed(w, h)
            mips, mw, mh = 0, w, h
            while mw > 1 or mh > 1:
                mw, mh = max(1, mw // 2), max(1, mh // 2)
                mips += needed(mw, mh)
            for label, n in {"long1": need + 1, "mips": need + mips, "short1": need - 1, "one": 1}.items():
                name = f"{fmt.name}-{w}x{h}-{label}"
                out.append(texture.case(name, fmt, w, h, texture.seeded(name, n)))
            out.append(texture.case(f"{fmt.name}-4x4-empty", fmt, 4, 4, {"hex": ""}))
            for w, h in ((0, 4), (4, 0), (0, 0)):
                out.append(texture.case(f"{fmt.name}-{w}x{h}-zero-empty", fmt, w, h, {"hex": ""}))
                name = f"{fmt.name}-{w}x{h}-zero-data"
                out.append(texture.case(name, fmt, w, h, texture.seeded(name, 64)))
            negatives = [(-1, 4), (4, -1), (-side + 1, -1), (-side, 4), (4, -side), (-side - 1, 1), (-side, -side)]
            for w, h in negatives:
                pw, ph = w + (-w) % side, h + (-h) % side
                for label, n in (("empty", 0), ("one", 1), ("data", 4096)):
                    # Both sides below 0 after padding, with a block or more: astcenc writes past its buffer once
                    # the data holds the product's blocks, so none is decoded.
                    if pw < 0 and ph < 0 and n >= 16:
                        continue
                    name = f"{fmt.name}-{w}x{h}-negative-{label}"
                    out.append(texture.case(name, fmt, w, h, texture.seeded(name, n) if n else {"hex": ""}))
            name = f"{fmt.name}-xbox360"
            out.append(texture.case(name, fmt, 7, 9, texture.seeded(name, needed(7, 9)), platform=texture.XBOX360))
            name = f"{fmt.name}-switch-one-gob"
            spec = texture.seeded(name, needed(7, 9))
            out.append(texture.case(name, fmt, 7, 9, spec, platform=texture.SWITCH, blob=[0] * 12))
    fmt = TF.ASTC_RGBA_4x4
    for w, h, spec in (
        (67_108_856, 1, {"hex": "00"}),
        (67_108_857, 1, {"hex": "00"}),
        (67_108_857, 1, {"zeros": 16_777_215 * 16}),
        (59, 3_033_169, {"hex": "00"}),
        (59, 3_033_169, {"zeros": 15 * 758_293 * 16}),
    ):
        label = "zeros" if "zeros" in spec else "one"
        out.append(texture.case(f"{fmt.name}-{w}x{h}-{label}", fmt, w, h, spec))
    return out


def fixtures() -> dict[str, dict]:
    meta = {
        "generator": "tools/fixtures/texture_astc.py",
        "unitypy": UnityPy.__version__,
        "pillow": PIL.__version__,
        "astc_encoder": astc_encoder.__version__,
        "hex_pixels": texture.HEX_PIXELS,
    }
    with decoding_with(AVX2):
        footprints = footprint_cases()
        formats = format_cases()
    return {
        "astc.json": {
            **meta,
            "coverage": coverage(footprints),
            "footprint_cases": footprints,
            "format_cases": formats,
        }
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    found = (UnityPy.__version__, PIL.__version__, astc_encoder.__version__)
    if found != ("1.25.2", "9.5.0", "0.1.12"):
        print(f"needs UnityPy 1.25.2, Pillow 9.5.0 and astc-encoder-py 0.1.12, found {found}", file=sys.stderr)
        return 2
    if AVX2 is None:
        print("needs astc-encoder-py's AVX2 build, which this host's CPU cannot load", file=sys.stderr)
        return 2
    stale = []
    for name, body in fixtures().items():
        text = texture.dump(body)
        path = OUT / name
        if args.check:
            if not path.exists() or path.read_text() != text:
                stale.append(name)
        else:
            OUT.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    if stale:
        print("stale fixtures: " + ", ".join(stale), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
