"""Synthetic legacy CRN streams and UnityPy texture oracle fixtures.

No encoder or external input is used. ``--native PATH`` writes CRNs and known
BC blocks using only the standard library; adding ``--check`` compares without
writing. With neither option, the pinned oracle records Texture2D.image from
synthetic SerializedFiles in tests/fixtures/crunch.json. Unity 2017.2 selects
legacy Crunch; Unity 2017.3 selects the fork for DXT, and crunched ETC always
uses the fork. ``--unity-native PATH`` writes the fork's known blocks.

Format rules below cite crn_decomp.h from the texture2ddecoder 1.0.5 sdist
(src/Texture2DDecoder/crunch, the legacy implementation). The legacy codec has
static canonical Huffman models, not adaptive Huffman models. Stateful paths
covered here are chunk-layout reservoir refills and endpoint/selector deltas.
The writer is original code describing that format, not an encoder port.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-texture/tests/fixtures/crunch.json"
NATIVE = ROOT / "crates/unityrs-texture/tests/fixtures/crunch_native.json"
# texture2ddecoder 1.0.5 sdist, crn_decomp.h:3068-3083.
ORDER = (17, 18, 19, 20, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15, 16)
# texture2ddecoder 1.0.5 sdist, crn_decomp.h:3636-3652.
TILES = ((0, 0, 0, 0), (0, 0, 1, 1), (0, 1, 0, 1), (0, 0, 1, 2),
         (1, 2, 0, 0), (0, 1, 0, 2), (1, 0, 2, 0), (0, 1, 2, 3))
# texture2ddecoder 1.0.5 sdist, crn_decomp.h:3355-3358.
COLOR_LINEAR = (0, 2, 3, 1)
ALPHA_LINEAR = (0, 2, 3, 4, 5, 6, 7, 1)


class Bits:
    """MSB-first fields (texture2ddecoder 1.0.5 sdist, crn_decomp.h:3187-3264)."""

    def __init__(self):
        self.bits = []

    def put(self, value: int, size: int):
        assert 0 <= value < 1 << size
        self.bits.extend((value >> i) & 1 for i in range(size - 1, -1, -1))

    def finish(self) -> bytes:
        bits = self.bits + [0] * (-len(self.bits) % 8)
        return bytes(sum(bits[i + j] << (7 - j) for j in range(8)) for i in range(0, len(bits), 8))


class Model:
    """Canonical codes; original writer, inverse of crn_decomp.h:3085-3165
    (texture2ddecoder 1.0.5 sdist). Run packing exercises all four run symbols.
    """

    def __init__(self, lengths):
        self.lengths = list(lengths)
        self.codes = {}
        code = 0
        previous = 0
        for length, symbol in sorted((n, s) for s, n in enumerate(lengths) if n):
            code <<= length - previous
            self.codes[symbol] = (code, length)
            code += 1
            previous = length
        # A singleton uses one one-bit code (the decoder permits that tree).
        assert code == 1 << previous or self.lengths == [1], "complete Huffman tree required"

    @classmethod
    def uniform(cls, count):
        if count == 1:
            return cls([1])
        width = max(1, (count - 1).bit_length())
        return cls([width] * (1 << width))

    @classmethod
    def sparse(cls, symbols):
        symbols = sorted(symbols)
        assert len(symbols) == 4
        lengths = [0] * (symbols[-1] + 1)
        for symbol in symbols:
            lengths[symbol] = 2
        return cls(lengths)

    def emit(self, out, symbol):
        code, length = self.codes[symbol]
        out.put(code, length)

    def write(self, out):
        # 8192's total_bits is 14. Secondary model's 21 lengths follow ORDER.
        out.put(len(self.lengths), 14)
        secondary = Model([4] * 11 + [5] * 10)
        out.put(21, 5)
        for symbol in ORDER:
            out.put(secondary.lengths[symbol], 3)
        i = 0
        while i < len(self.lengths):
            value = self.lengths[i]
            end = i + 1
            while end < len(self.lengths) and self.lengths[end] == value:
                end += 1
            count = end - i
            if value == 0:
                while count >= 3:
                    large = count >= 11
                    run = min(count, 138 if large else 10)
                    secondary.emit(out, 18 if large else 17)
                    out.put(run - (11 if large else 3), 7 if large else 3)
                    count -= run
            else:
                secondary.emit(out, value)
                count -= 1
                while count >= 3:
                    large = count >= 7
                    run = min(count, 70 if large else 6)
                    secondary.emit(out, 20 if large else 19)
                    out.put(run - (7 if large else 3), 6 if large else 2)
                    count -= run
            for _ in range(count):
                secondary.emit(out, value)
            i = end


def packed_selectors(values, mapping, width):
    return sum(mapping[value] << (i * width) for i, value in enumerate(values)).to_bytes(2 * width, "little")


def palettes(count):
    """Streams and independently constructed BC palette entries. Deltas wrap
    at 5/6/8 bits and 2/3 selector bits (texture2ddecoder 1.0.5 sdist,
    crn_decomp.h:3839-4104). Both endpoint orders and all selectors occur.
    """
    streams, entries = [], []
    for alpha in (False, True):
        out = Bits()
        models = [Model.sparse((0, 1, 10, 255))] if alpha else [
            Model.sparse((0, 1, 10, 31)), Model.sparse((0, 1, 10, 63))]
        for model in models:
            model.write(out)
        current = [0] * (2 if alpha else 6)
        endpoints = []
        for i in range(count):
            for channel in range(len(current)):
                mask = 255 if alpha else (63 if channel % 3 == 1 else 31)
                # 0, maximum and +1 repeatedly cross the modular boundary.
                delta = (mask, 1, 0, 1)[(i + channel) % 4]
                models[0 if alpha or channel % 3 != 1 else 1].emit(out, delta)
                current[channel] = (current[channel] + delta) & mask
            if alpha:
                endpoints.append(bytes(current))
            else:
                a, b, c, d, e, f = current
                endpoints.append(struct.pack("<HH", c | b << 5 | a << 11, f | e << 5 | d << 11))
        streams.append(out.finish())
        entries.append(endpoints)
        out = Bits()
        maximum = 7 if alpha else 3
        radix = 2 * maximum + 1
        model = Model.uniform(radix * radix)
        model.write(out)
        current = [0] * 16
        selectors = []
        for i in range(count):
            for pair in range(8):
                da = (-maximum, maximum, 0, 1)[(i + pair) % 4]
                db = (maximum, -maximum, 1, 0)[(i + pair) % 4]
                model.emit(out, da + maximum + radix * (db + maximum))
                current[2 * pair] = (current[2 * pair] + da) & maximum
                current[2 * pair + 1] = (current[2 * pair + 1] + db) & maximum
            selectors.append(packed_selectors(current, ALPHA_LINEAR if alpha else COLOR_LINEAR, 3 if alpha else 2))
        streams.append(out.finish())
        entries.append(selectors)
    return streams, entries


def level(width, height, alpha, count, models, palettes, phase, selector_deltas=None):
    """Encode all layouts in serpentine chunk order, including skipped edge
    blocks' selector deltas (texture2ddecoder 1.0.5 sdist,
    crn_decomp.h:4143-4470). Expected blocks come directly from palette values.
    """
    bx, by = (width + 3) // 4, (height + 3) // 4
    cx, cy = (bx + 1) // 2, (by + 1) // 2
    coords = [(x, y) for y in range(cy) for x in (range(cx) if y % 2 == 0 else range(cx - 1, -1, -1))]
    layouts = [(i + phase) % 8 for i in range(len(coords))]
    out = Bits()
    prev = [0] * 4
    blocks = [None] * (bx * by)
    tick = 0
    positions = [0] * 4

    def index(kind):
        nonlocal tick
        delta = (0, count - 1, 1 if count > 1 else 0, count // 2)[tick % 4]
        tick += 1
        if selector_deltas is not None and kind in selector_deltas:
            delta = selector_deltas[kind][positions[kind]]
            positions[kind] += 1
        models[kind + 1].emit(out, delta)
        prev[kind] += delta
        if prev[kind] >= count:
            prev[kind] -= count
        return prev[kind]

    for i, (x, y) in enumerate(coords):
        if i % 3 == 0:
            group = layouts[i:i + 3] + [0, 0]
            models[0].emit(out, group[0] | group[1] << 3 | group[2] << 6)
        tiles = TILES[layouts[i]]
        ce, ae = [], []
        if alpha:
            ae = [palettes[2][index(2)] for _ in range(max(tiles) + 1)]
        ce = [palettes[0][index(0)] for _ in range(max(tiles) + 1)]
        for pos, tile in enumerate(tiles):
            a = index(3) if alpha else None
            c = index(1)
            xx, yy = x * 2 + pos % 2, y * 2 + pos // 2
            if xx < bx and yy < by:
                blocks[yy * bx + xx] = (ae[tile] + palettes[3][a] if alpha else b"") + ce[tile] + palettes[1][c]
    return out.finish(), {"width": width, "height": height, "blocks": b"".join(blocks).hex()}


def crc16(data):
    """CRN CRC16 (texture2ddecoder 1.0.5 sdist, crn_decomp.h:2098-2119)."""
    crc = 0xFFFF
    for byte in data:
        q = byte ^ (crc >> 8)
        r = (q >> 4) ^ q
        crc = ((crc << 8) ^ r ^ (r << 5) ^ (r << 12)) & 0xFFFF
    return crc ^ 0xFFFF


def build(width, height, alpha=False, count=3, mips=1, long_codes=False, phase=0, selector_deltas=None):
    """Header, palette descriptors and level offsets: texture2ddecoder 1.0.5
    sdist, crn_decomp.h:272-320. All fields are big-endian packed integers.
    """
    streams, entries = palettes(count)
    # A complete 17-symbol ladder puts symbol 16 at length 16 (slow Huffman path).
    delta = Model(list(range(1, 17)) + [16]) if long_codes else Model.uniform(count)
    if selector_deltas is not None:
        # Symbol 2 must be encodable even with only one palette entry.
        delta = Model.uniform(max(count, 3))
    models = [Model.uniform(512)] + [delta] * 4
    tables = Bits()
    for model in models[:5 if alpha else 3]:
        model.write(tables)
    pieces = streams if alpha else streams[:2]
    pieces = pieces + [tables.finish()]
    levels = []
    w, h = width, height
    for mip in range(mips):
        stream, expected = level(w, h, alpha, count, models, entries, phase + mip, selector_deltas)
        pieces.append(stream)
        levels.append(expected)
        w, h = max(1, w // 2), max(1, h // 2)
    size = 70 + 4 * mips
    offsets = []
    for piece in pieces:
        offsets.append(size)
        size += len(piece)
    header = bytearray(b"Hx" + (70 + 4 * mips).to_bytes(2, "big") + bytes(2))
    payload = b"".join(pieces)
    header += size.to_bytes(4, "big") + crc16(payload).to_bytes(2, "big")
    header += struct.pack(">HHBBB", width, height, mips, 1, 2 if alpha else 0) + bytes(14)
    for i in range(4):
        if i < (4 if alpha else 2):
            header += offsets[i].to_bytes(3, "big") + len(pieces[i]).to_bytes(3, "big") + count.to_bytes(2, "big")
        else:
            header += bytes(8)
    table = 4 if alpha else 2
    header += len(pieces[table]).to_bytes(2, "big") + offsets[table].to_bytes(3, "big")
    for offset in offsets[table + 1:]:
        header += offset.to_bytes(4, "big")
    assert len(header) == 70 + 4 * mips
    header[4:6] = crc16(header[6:]).to_bytes(2, "big")
    return bytes(header) + payload, levels


def cases():
    out = []
    for alpha in (False, True):
        for width, height, count, mips, long_codes in (
            (1, 1, 1, 1, False), (3, 5, 3, 1, False), (4, 4, 3, 1, False),
            (7, 9, 3, 1, False), (32, 24, 17, 1, True),
            (33, 29, 17, 6, True), (17, 33, 3, 5, False),
        ):
            name = f"{'DXT5' if alpha else 'DXT1'}Crunched-{width}x{height}-p{count}-m{mips}"
            data, levels = build(width, height, alpha, count, mips, long_codes)
            out.append({"id": name, "format": 29 if alpha else 28, "width": width, "height": height,
                        "palette_count": count, "long_codes": long_codes, "data": {"hex": data.hex()},
                        "data_sha256": hashlib.sha256(data).hexdigest(), "levels": levels})
    return out


def truncated_cases():
    """Nonempty regions zero-extend (crn_decomp.h:3166-3280).

    Blocks below were checked with the legacy 1.0.5 crnd_unpack_begin /
    crnd_unpack_level reference, without crnd_validate_file (like UnityPy).
    """
    out = []
    for kind in ("last-byte", "mid-level", "mid-table"):
        width, height = (17, 17) if kind == "mid-level" else (1, 1)
        data, levels = build(width, height, count=1)
        data = bytearray(data)
        if kind == "mid-table":
            # The final model's second code length lies in the removed byte.
            # Its zero-extended length is 0: [1, 1] becomes [1, 0], still
            # decoding symbol 0. This cuts inside a Huffman model, not padding.
            tables = Bits()
            for n, total in ((20, 1), (21, 1), (19, 2)):
                tables.put(total, 14)
                tables.put(n, 5)
                for symbol in ORDER[:n]:
                    tables.put(1 if symbol in (0, 1) else 0, 3)
                for _ in range(total):
                    tables.put(1, 1)
            start = int.from_bytes(data[67:70], "big")
            end = int.from_bytes(data[70:74], "big")
            table = tables.finish()[:-1]
            assert len(table) == 30
            data[start:end] = table
            data[65:67] = len(table).to_bytes(2, "big")
            data[70:74] = (start + len(table)).to_bytes(4, "big")
        else:
            start = int.from_bytes(data[70:74], "big")
            data = data[:start + 1] if kind == "mid-level" else data[:-1]
        data[6:10] = len(data).to_bytes(4, "big")
        # All palette-index deltas are singleton zero codes. Every chunk
        # layout therefore produces the same known block after truncation.
        block = bytes.fromhex(levels[0]["blocks"][:16])
        levels[0]["blocks"] = (block * (((width + 3) // 4) * ((height + 3) // 4))).hex()
        out.append({"id": f"DXT1Crunched-truncated-{kind}", "format": 28,
                    "width": width, "height": height, "palette_count": 1,
                    "long_codes": False, "data": {"hex": data.hex()},
                    "data_sha256": hashlib.sha256(data).hexdigest(), "levels": levels})
    return out


def data_size_cases():
    """Retain all bytes while under-declaring data_size by three.

    The last level uses init's buffer size, not the header field
    (texture2ddecoder 1.0.5 sdist crn_decomp.h:3672-3687, :3690-3703).
    The 7x9 case is Codex's 188-byte/185-declared repro; 43x9 reconstructs
    the reviewer's equivalent with this builder, without embedding its CRN.
    Oracle regeneration gets the expected blocks from unpack_crunch below.
    """
    out = []
    for width, height in ((7, 9), (43, 9)):
        data, levels = build(width, height, count=3)
        data = bytearray(data)
        data[6:10] = (len(data) - 3).to_bytes(4, "big")
        if width == 7:
            assert len(data) == 188
            assert levels[0]["blocks"].startswith("20f8e10fc823c823")
        out.append({"id": f"DXT1Crunched-underdeclared-{width}x{height}", "format": 28,
                    "width": width, "height": height, "palette_count": 3,
                    "long_codes": False, "data": {"hex": data.hex()},
                    "data_sha256": hashlib.sha256(data).hexdigest(), "levels": levels})
    return out


def edge_block_cases():
    """unpack_dxt1/unpack_dxt5 advance skipped selectors without fetching
    them (texture2ddecoder 1.0.5 sdist crn_decomp.h:4270-4303, :4429-4470).
    Each chosen selector index goes 0, 1 (outside its one-entry palette),
    0, 0 after single subtraction. Layout 7 also gives the invisible blocks
    their own endpoints, which the reference DOES fetch unconditionally.
    """
    out = []
    for alpha, kinds in ((False, (1,)), (True, (1,)), (True, (3,)), (True, (1, 3))):
        deltas = {kind: (0, 2, 0, 0) for kind in kinds}
        data, levels = build(1, 1, alpha, count=1, phase=7, selector_deltas=deltas)
        label = "-".join("color" if kind == 1 else "alpha" for kind in kinds)
        out.append({"id": f"{'DXT5' if alpha else 'DXT1'}Crunched-skipped-{label}-selector",
                    "format": 29 if alpha else 28, "width": 1, "height": 1,
                    "palette_count": 1, "long_codes": False, "data": {"hex": data.hex()},
                    "data_sha256": hashlib.sha256(data).hexdigest(), "levels": levels})
    return out


def descriptor_cases():
    """Ignored alpha ranges: crunch/crn_decomp.h:3824-3839 gates on endpoints."""
    out = []
    for label, descriptors in (("endpoint", (2,)), ("selector", (3,)), ("both", (2, 3))):
        data, levels = build(1, 1, False, count=1)
        data = bytearray(data)
        for i in descriptors:
            # Endpoints remain absent; selectors advertise one ignored entry.
            data[33+8*i:41+8*i] = bytes.fromhex(
                'ffffff0000000000' if i == 2 else 'ffffff0000000001')
        data[4:6] = crc16(data[6:74]).to_bytes(2, 'big')
        out.append({"id": f"DXT1Crunched-ignored-alpha-{label}-range", "format": 28,
                    "width": 1, "height": 1, "palette_count": 1, "long_codes": False,
                    "data": {"hex": data.hex()}, "data_sha256": hashlib.sha256(data).hexdigest(),
                    "levels": levels})
    return out


def serialized(case):
    """A hand-built Texture2D typetree and object, never a saved asset.

    The minimal fields include Texture2D's required generated-class members;
    the legacy 2017.2 version selects legacy Crunch in the oracle converter.
    """
    sys.path.insert(0, str(ROOT / "tools/parity/tests"))
    import synth as S

    fields = [("string", "m_Name"), ("int", "m_Width"), ("int", "m_Height"),
              ("int", "m_CompleteImageSize"), ("int", "m_TextureFormat"), ("int", "m_MipCount"),
              ("int", "m_ImageCount"), ("int", "m_TextureDimension"), ("int", "m_LightmapFormat"),
              ("int", "m_ColorSpace")]
    settings = S.Node("GLTextureSettings", "m_TextureSettings", [S.Node(t, n) for t, n in
                      (("int", "m_FilterMode"), ("int", "m_Aniso"), ("float", "m_MipBias"))])
    tree = S.Node("Texture2D", "Base", [S.Node(t, n) for t, n in fields] +
                  [S.Node("bool", "m_IsReadable", meta=0x4000), settings,
                   S.Node("TypelessData", "image data")])
    raw = bytes.fromhex(case["data"]["hex"])
    body = S.s32("Synthetic Crunch — texture") + struct.pack(
        "<9i", case["width"], case["height"], len(raw), case["format"], len(case["levels"]), 1, 2, 0, 0)
    body += bytes((1, 0, 0, 0)) + struct.pack("<iif", 1, 1, 0) + struct.pack("<i", len(raw)) + raw
    return S.serialized_file([S.Type(28, tree)], [S.Obj(1, 0, body)], unity_version=b"2017.3.0f1" if case.get("unity") else b"2017.2.0f1")


def fixture(native=False):
    from texture_unitycrunch import cases as unity_cases
    unity = unity_cases()
    rows = cases()
    truncated = truncated_cases()
    underdeclared = data_size_cases()
    edges = edge_block_cases()
    descriptors = descriptor_cases()
    doc = {"generator": "tools/fixtures/texture_crunch.py", "schema": 1, "cases": rows, "truncated_cases": truncated,
           "data_size_cases": underdeclared, "edge_block_cases": edges, "descriptor_cases": descriptors}
    if native:
        return doc
    doc["unity_cases"] = unity
    import PIL
    import UnityPy
    import texture2ddecoder

    found = (UnityPy.__version__, PIL.__version__, texture2ddecoder.__version__)
    if found != ("1.25.2", "9.5.0", "1.0.5"):
        raise RuntimeError(f"needs UnityPy 1.25.2, Pillow 9.5.0 and texture2ddecoder 1.0.5; found {found}")
    doc.update(unitypy=found[0], pillow=found[1], texture2ddecoder=found[2])
    for case in underdeclared + edges + descriptors:
        blocks = texture2ddecoder.unpack_crunch(bytes.fromhex(case["data"]["hex"]))
        assert blocks.hex() == case["levels"][0]["blocks"], case["id"]
        case["levels"][0]["blocks"] = blocks.hex()
    # Verify every fork mip through the actual pinned wrapper, whose Python
    # binding always asks for level zero. Repoint level zero to each mip;
    # checksums are ignored by unpack_begin, as by UnityPy's native wrapper.
    for case in unity:
        raw = bytes.fromhex(case["data"]["hex"])
        for index, mip in enumerate(case["levels"]):
            data = bytearray(raw)
            start = raw[70+4*index:74+4*index]
            if index+1 < len(case["levels"]):
                end = int.from_bytes(raw[74+4*index:78+4*index], "big")
                data = data[:end]
            data[6:10] = len(data).to_bytes(4, "big")
            data[12:16] = struct.pack(">HH", mip["width"], mip["height"])
            data[16] = 1
            data[70:74] = start
            blocks = texture2ddecoder.unpack_unity_crunch(bytes(data))
            assert blocks.hex() == mip["blocks"], (case["id"], index)
    for case in rows + truncated + underdeclared + edges + descriptors + unity:
        data = serialized(case)
        case["file"] = data.hex()
        env = UnityPy.Environment()
        sf = env.load_file(data, name="synthetic-crunch.assets")
        image = sf.objects[1].read().image
        pixels = image.tobytes()
        case.update(mode=image.mode, size=list(image.size), pixels=pixels.hex(),
                    sha256=hashlib.sha256(pixels).hexdigest())
    return doc


def dump(doc):
    return json.dumps(doc, ensure_ascii=False, indent=2) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="regenerate and compare; write nothing")
    outputs = parser.add_mutually_exclusive_group()
    outputs.add_argument("--unity-native", type=Path, metavar="PATH", help="write synthetic Unity fork CRNs and expected blocks")
    outputs.add_argument("--native", type=Path, metavar="PATH", help="write only synthetic CRNs and expected BC blocks")
    args = parser.parse_args()
    path = args.unity_native or args.native or OUT
    try:
        if args.unity_native is not None:
            from texture_unitycrunch import cases as unity_cases
            text = dump({"schema": 1, "cases": unity_cases()})
        else:
            text = dump(fixture(native=args.native is not None))
    except (ImportError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        return 2
    if args.check:
        if not path.exists() or path.read_text(encoding="utf-8") != text:
            print(f"stale fixture: {path}", file=sys.stderr)
            return 1
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
