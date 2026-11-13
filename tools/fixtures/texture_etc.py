"""The ETC and EAC cases of unityrs-texture's oracle fixtures, which tools/fixtures/texture.py records into
crates/unityrs-texture/tests/fixtures/etc.json (tests/etc_oracle.rs replays them).

UnityPy 1.25.2 decodes ETC_RGB4, ETC2_RGB, ETC2_RGBA1, ETC2_RGBA8, EAC_R, EAC_R_SIGNED, EAC_RG and EAC_RG_SIGNED with
texture2ddecoder 1.0.5 (export/Texture2DConverter.py:405-432). texture2ddecoder checks neither the data's length nor a
side: data short of the padded blocks, or a side that padding leaves below 0 (-4 and below), makes it read, and write,
past its buffers, so no case here has either; unityrs refuses both (PARITY.md section 19.3). Every other case is:

- **sizes**: each format at every size class of texture.py, from SplitMix64 data of exactly mip 0's bytes;
- **lengths**: data one byte long, a mip chain long, and twice as long, which texture2ddecoder ignores past mip 0;
- **sides**: a side of 0 (the empty image, whatever the data), and the negative sides padding makes 0, with data and
  without (UnityPy's empty-data check comes first);
- **modes**: blocks built here for every block mode, each at one block (4x4), at odd sizes (6x7, 13x5) and at 16
  blocks (16x16): ETC1's individual and differential modes, and the differential mode whose delta leaves 0 to 255,
  which ETC1 wraps; ETC2's five modes (individual, differential, T, H and planar); ETC2 A1's four (differential, T, H,
  planar), each with the opaque bit set and clear, so with punch-through pixels; ETC2 A8's alpha with a multiplier of 0
  and not, at every modifier table and at bases 0 and 255, over each colour mode; EAC's with a multiplier of 0 and
  not, at every table, and at bases 0, 127, 128 and 255 (-128 signed);
- **minimum base**: signed EAC blocks of base -128 with every index 7 (table 0's +14), at a multiplier of 0 and of
  1, where texture2ddecoder computes with -128 and the Khronos Data Format Specification 1.3, sections 22.7 and
  22.8, with -127, beside the same blocks at -127; for EAC_RG_SIGNED in either channel;
- **platforms**: an Xbox 360 texture (no ETC or EAC format is byte-swapped) and a Switch one whose blob names one GOB
  per block (not swizzled): decoded as on any other platform;
- **limits**: Pillow's raw decoder reading texture2ddecoder's BGRA output at a padded width past its cap, 67,108,857
  (padded to 67,108,860), which raises MemoryError after texture2ddecoder has decoded the zeros; and its crop's
  decompression-bomb check at 59 x 3,033,169, as for DXT1 (texture.py's limit cases).

A block is drawn from SplitMix64 bytes seeded by its case's name, with the bits that choose its mode set, or redrawn
until they do (`colour_block`), and recorded as hex. Standard library only: a module texture.py imports.
"""

from __future__ import annotations

import hashlib

M64 = (1 << 64) - 1
# The formats, by TextureFormat value, and the bytes a 4x4 block of each holds.
ETC_RGB4, EAC_R, EAC_R_SIGNED, EAC_RG, EAC_RG_SIGNED, ETC2_RGB, ETC2_RGBA1, ETC2_RGBA8 = 34, 41, 42, 43, 44, 45, 46, 47
NAMES = {
    ETC_RGB4: "ETC_RGB4",
    EAC_R: "EAC_R",
    EAC_R_SIGNED: "EAC_R_SIGNED",
    EAC_RG: "EAC_RG",
    EAC_RG_SIGNED: "EAC_RG_SIGNED",
    ETC2_RGB: "ETC2_RGB",
    ETC2_RGBA1: "ETC2_RGBA1",
    ETC2_RGBA8: "ETC2_RGBA8",
}
BLOCK = {f: 16 if f in (EAC_RG, EAC_RG_SIGNED, ETC2_RGBA8) else 8 for f in NAMES}
SIZES = [
    (1, 1), (2, 2), (3, 5), (4, 4), (7, 9), (5, 3), (8, 8), (6, 13), (12, 12), (17, 1), (1, 17),
    (33, 2), (2, 33), (31, 33), (64, 4), (4, 64), (100, 60), (128, 128), (257, 3),
]  # fmt: skip
MODE_SIZES = [(4, 4), (6, 7), (13, 5), (16, 16)]
XBOX360, SWITCH = 11, 38


def seed_of(name: str) -> int:
    """texture.py's seed for a case name."""
    return int.from_bytes(hashlib.sha256(name.encode()).digest()[:8], "little") & ((1 << 63) - 1)


class Stream:
    """SplitMix64's bytes, each output little-endian (unityrs_testkit::rng::Rng)."""

    def __init__(self, seed: int):
        self.state = seed & M64
        self.buf = b""

    def take(self, n: int) -> bytes:
        while len(self.buf) < n:
            self.state = (self.state + 0x9E3779B97F4A7C15) & M64
            z = self.state
            z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & M64
            z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & M64
            z ^= z >> 31
            self.buf += z.to_bytes(8, "little")
        out, self.buf = self.buf[:n], self.buf[n:]
        return out


def needed(fmt: int, width: int, height: int) -> int:
    return ((width + 3) // 4) * ((height + 3) // 4) * BLOCK[fmt]


def mip_chain(fmt: int, width: int, height: int) -> int:
    total = 0
    while width > 1 or height > 1:
        width, height = max(1, width // 2), max(1, height // 2)
        total += needed(fmt, width, height)
    return total


def seeded(name: str, n: int) -> dict:
    return {"seed": seed_of(name), "len": n}


def mode_of(block: bytes) -> str:
    """The mode ETC2 reads a colour block in once its diff (or opaque) bit is taken as set: the differential mode, or
    where red, green or blue plus its delta leaves 0 to 255, T, H or planar, in that order."""
    for channel, mode in zip(block[:3], ("t", "h", "planar"), strict=True):
        base, delta = channel & 0xF8, ((channel << 3) & 0x18) - ((channel << 3) & 0x20)
        if not 0 <= base + delta <= 255:
            return mode
    return "differential"


def colour_block(stream: Stream, mode: str, diff_bit: int | None = None) -> bytes:
    """An 8-byte colour block in `mode`: `individual` clears the diff bit; the rest set it (or, for ETC2 A1, take
    `diff_bit` as the opaque bit) and redraw until `mode_of` gives `mode` (`wrapped`: any mode but the differential,
    which ETC1 reads as a differential block whose delta wraps)."""
    while True:
        block = bytearray(stream.take(8))
        if mode == "individual":
            block[3] &= 0xFD
            return bytes(block)
        block[3] = (block[3] & 0xFD) | (2 if diff_bit is None else diff_bit << 1)
        got = mode_of(block)
        if got == mode or (mode == "wrapped" and got != "differential"):
            return bytes(block)


def alpha_block(stream: Stream, index: int, kind: str) -> bytes:
    """An 8-byte ETC2 A8 alpha or EAC block, its modifier table `index % 16`: `zero` with a multiplier of 0, `low`
    and `high` with base 0 and 255, `base127` and `base128` with those bases, `any` as drawn."""
    block = bytearray(stream.take(8))
    multiplier = 0 if kind == "zero" else (block[1] >> 4) or 1
    block[1] = (multiplier << 4) | (index % 16)
    block[0] = {"low": 0, "high": 255, "base127": 127, "base128": 128}.get(kind, block[0])
    return bytes(block)


# Each format's block kinds: label -> a function of (stream, block index) giving one block.
ETC1_MODES = ("individual", "differential", "wrapped")
ETC2_MODES = ("individual", "differential", "t", "h", "planar")
A1_MODES = ("differential", "t", "h", "planar")
ALPHA_KINDS = ("zero", "any", "low", "high")
EAC_KINDS = ("zero", "any", "low", "high", "base127", "base128")


def kinds(fmt: int) -> dict:
    if fmt == ETC_RGB4:
        return {m: (lambda m: lambda s, i: colour_block(s, m))(m) for m in ETC1_MODES}
    if fmt == ETC2_RGB:
        return {m: (lambda m: lambda s, i: colour_block(s, m))(m) for m in ETC2_MODES}
    if fmt == ETC2_RGBA1:
        return {
            f"{m}-{'opaque' if bit else 'punch'}": (lambda m, bit: lambda s, i: colour_block(s, m, bit))(m, bit)
            for m in A1_MODES
            for bit in (1, 0)
        }
    if fmt == ETC2_RGBA8:
        return {
            f"alpha-{a}-{m}": (lambda a, m: lambda s, i: alpha_block(s, i, a) + colour_block(s, m))(a, m)
            for a in ALPHA_KINDS
            for m in ETC2_MODES
        }
    pairs = fmt in (EAC_RG, EAC_RG_SIGNED)
    return {
        k: (lambda k: lambda s, i: alpha_block(s, i, k) + (alpha_block(s, i + 7, k) if pairs else b""))(k)
        for k in EAC_KINDS
    }


def mode_cases() -> list[tuple]:
    out = []
    for fmt, name in NAMES.items():
        for label, make in kinds(fmt).items():
            for w, h in MODE_SIZES:
                case_name = f"{name}-{label}-{w}x{h}"
                stream = Stream(seed_of(case_name))
                count = needed(fmt, w, h) // BLOCK[fmt]
                data = b"".join(make(stream, i) for i in range(count))
                out.append((case_name, fmt, w, h, {"hex": data.hex()}))
    return out


def size_cases() -> list[tuple]:
    return [
        (f"{n}-{w}x{h}-exact", f, w, h, seeded(f"{n}-{w}x{h}-exact", needed(f, w, h)))
        for f, n in NAMES.items()
        for w, h in SIZES
    ]


def length_cases() -> list[tuple]:
    out = []
    for f, n in NAMES.items():
        for w, h in ((3, 5), (8, 8), (7, 9), (16, 4)):
            need = needed(f, w, h)
            for label, size in (("long1", need + 1), ("mips", need + mip_chain(f, w, h)), ("double", 2 * need + 5)):
                out.append((f"{n}-{w}x{h}-{label}", f, w, h, seeded(f"{n}-{w}x{h}-{label}", size)))
    return out


def side_cases() -> list[tuple]:
    out = []
    for f, n in NAMES.items():
        for w, h in ((0, 0), (0, 4), (4, 0), (0, -3), (-3, 0), (0, 1)):
            for label, spec in (("empty", {"hex": ""}), ("data", seeded(f"{n}-{w}x{h}-zero", 64))):
                out.append((f"{n}-{w}x{h}-zero-{label}", f, w, h, spec))
        # Sides padding makes 0 (-1 to -3): texture2ddecoder decodes nothing, and the crop raises.
        for w, h in ((-1, 4), (4, -1), (-3, -3), (-1, -1), (-2, 1), (1, -3)):
            name = f"{n}-{w}x{h}-negative"
            out.append((name, f, w, h, seeded(name, 1024)))
            out.append((f"{name}-empty", f, w, h, {"hex": ""}))
    return out


def min_base_cases() -> list[tuple]:
    out = []
    for header, label in ((0x00, "x0"), (0x10, "x1")):
        def block(base):
            return bytes([base, header]) + b"\xff" * 6
        r = [("min", block(0x80)), ("minus127", block(0x81))]
        rg = [("min-min", block(0x80) + block(0x80)), ("minus127-min", block(0x81) + block(0x80))]
        for fmt, blocks in ((EAC_R_SIGNED, r), (EAC_RG_SIGNED, rg)):
            for kind, data in blocks:
                out.append((f"{NAMES[fmt]}-base-{kind}-{label}-4x4", fmt, 4, 4, {"hex": data.hex()}))
    return out


def platform_cases() -> list[tuple]:
    out = []
    for f, n in NAMES.items():
        for label, platform, blob in (("xbox360", XBOX360, None), ("switch-one-gob", SWITCH, [0] * 12)):
            name = f"{n}-{label}"
            out.append((name, f, 7, 9, seeded(name, needed(f, 7, 9)), platform, blob))
    return out


def limit_cases() -> list[tuple]:
    w = 67_108_857
    out = [(f"ETC_RGB4-{w}x1-zeros", ETC_RGB4, w, 1, {"zeros": needed(ETC_RGB4, w, 1)})]
    w, h = 59, 3_033_169
    out.append((f"ETC_RGB4-{w}x{h}-zeros", ETC_RGB4, w, h, {"zeros": needed(ETC_RGB4, w, h)}))
    return out


def cases() -> list[tuple]:
    """Every case: (name, format, width, height, data spec[, platform, blob])."""
    return (
        size_cases() + length_cases() + side_cases() + mode_cases() + min_base_cases() + platform_cases() + limit_cases()
    )
