"""The TextureFormat matrix of PARITY.md section 17, against the oracle.

`ThisParityFile` reads the matrix with the standard library alone. `AgainstTheOracle`
needs UnityPy 1.25.2 (the oracle image) and is skipped elsewhere.

`AgainstTheOracle` builds every cell of every row itself (`texture_cells.expected_row`) and
requires the matrix to hold exactly that, so a change to any cell fails a test. No builder
reads PARITY.md: `test_no_cell_is_built_from_this_file` builds the rows again in a new
process, from a copy of it with every cell changed. The cells come:
- from UnityPy: the value; the decoder (`CONV_TABLE`); the mode and the result of
  `parse_image_data` on real-sized data, with the source lines the result cites; the
  padding, Switch, Xbox and crunch gates, observed by running `parse_image_data` with
  stand-ins for the decoders, the deswizzle and the crunch unpackers; and what
  `image_to_texture2d` writes;
- from the pixel cases (`texture_cases.py`): the conversion, and whether a row works, is
  wrong or is unverified;
- from `RESULT_NOTES` and `RAISE_REASONS` (`texture_cells.py`): the words a row adds to its
  result, each shown by a test here;
- the tier from the result, and the status: pinned by this test, implemented in the phase
  `DESIGN.md`'s P1 list gives.

The pixel cases are built from each format's layout, not from UnityPy: raw
formats from a byte pattern, block formats from solid-colour blocks whose decoded
colour the format's specification fixes (one colour per block, so block order and
UnityPy's vertical flip show too). A row marked `works` must decode to that
reference; a row marked `wrong` must decode to the wrong pixels the case states and
not to the reference; a row marked `unverified` has no reference, and its pixels are
pinned as UnityPy returns them. A crunched row is decoded with the crunch unpacker
replaced by one that returns its base format's case, so its conversion is checked
and the crunch stream itself is not.
"""

import json
import math
import random
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from texture_cases import PIXEL_CASES, RGB48_PIXELS, H, W, rgb48  # noqa: E402
from texture_cells import (  # noqa: E402
    COLUMNS,
    GOBS,
    ORACLE,
    P1,
    SWITCH_BLOB,
    base_name,
    decode,
    expected_row,
    real_sized,
    stand_in_unpackers,
    tier_of,
    unit,
)

import inventory  # noqa: E402
import references  # noqa: E402

if ORACLE:
    import texture2ddecoder
    from PIL import Image
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.helpers import TextureSwizzler


PARITY = inventory.DEFAULT_PARITY
NEEDS_ORACLE = "needs UnityPy 1.25.2 (the oracle image)"

RESULT = re.compile(r"(works|wrong|unverified|raises|crunch)\b")


def first_span(cell):
    match = re.search(r"`([^`]+)`", cell)
    return match.group(1) if match else None


class ThisParityFile(unittest.TestCase):
    """What PARITY.md says, read without the oracle."""

    def setUp(self):
        text = PARITY.read_text(encoding="utf-8")
        self.rows = inventory.texture_rows(text)
        self.header = next(header for header, _ in inventory.tables(text) if header[:2] == ["value", "format"])

    def test_the_matrix_has_one_row_per_texture_format(self):
        self.assertEqual(self.header, COLUMNS)
        self.assertEqual(len(self.rows), 78)
        self.assertEqual(len({row["value"] for row in self.rows.values()}), 78)

    def test_tier_b_has_the_21_formats_design_names(self):
        tier_b = sorted(name for name, row in self.rows.items() if row["tier"] == "B")
        signed = [name for name in self.rows if name.endswith("_SIGNED") and not name.startswith("EAC")]
        expected = ["BC6H", "RHalf", "RGHalf", "RFloat", "RGFloat", "RGBAFloat", "RGBFloat"]
        expected += signed + ["RG16", "RG32", "R16", "RGBA64", "RGB9e5Float", "YUY2"]
        self.assertEqual(len(signed), 8)
        self.assertEqual(tier_b, sorted(expected))
        self.assertEqual(len(tier_b), 21)
        self.assertEqual(self.rows["ARGBFloat"]["tier"], "U", "raises, and no reference can be named")

    def test_every_tier_b_row_names_its_reference(self):
        """Section 17.1 has one row per Tier B format, in the mode `references` returns, and nothing else."""
        text = PARITY.read_text(encoding="utf-8")
        header, body = next(
            (h, b) for h, b in inventory.tables(text) if h[:3] == ["format", "stored per pixel (little-endian)", "mode"]
        )
        self.assertEqual(header, ["format", "stored per pixel (little-endian)", "mode", "reference", "test", "status"])
        listed = {first_span(cells[0]): cells for cells in body}
        tier_b = {name for name, row in self.rows.items() if row["tier"] == "B"}
        self.assertEqual(sorted(listed), sorted(tier_b))
        self.assertEqual(sorted(listed), sorted(references.TIER_B))
        for name, cells in listed.items():
            with self.subTest(name):
                self.assertEqual(cells[2], references.TIER_B[name])
                self.assertTrue(cells[3].strip() and cells[3] != "—")
                self.assertEqual(cells[4], "planned: divergence test against `references.decode`")
        self.assertEqual(
            sorted(references.UNSUPPORTED),
            [
                n
                for n, r in self.rows.items()
                if r["tier"] == "U" and RESULT.match(r["UnityPy 1.25.2"]).group(1) == "raises"
            ],
        )

    def test_p1_formats_are_the_five_design_names(self):
        p1 = sorted(name for name, row in self.rows.items() if row["status"] == "pinned P0; impl P1")
        self.assertEqual(p1, P1)

    def test_tier_and_status_follow_the_result(self):
        for name, row in self.rows.items():
            with self.subTest(name):
                kind = RESULT.match(row["UnityPy 1.25.2"]).group(1)
                self.assertEqual(row["tier"], tier_of(name, kind))
                self.assertEqual(row["status"], "pinned P0; impl P1" if name in P1 else "pinned P0; impl P3")


def matrix_errors(rows, expected):
    """(format, column) for every cell of `rows` that differs from `expected`."""
    return [
        (name, column) for name, cells in expected.items() for column in cells if rows[name][column] != cells[column]
    ]


def change_cells(text, columns):
    """`text` with " changed" added to the named columns' cells in every row of the matrix."""
    lines = text.splitlines(keepends=True)
    header = next(i for i, line in enumerate(lines) if line.startswith("| value | format |"))
    end = next(i for i in range(header, len(lines)) if not lines[i].startswith("|"))
    for i in range(header + 2, end):
        cells = [cell.strip() for cell in lines[i].rstrip("\n").strip("|").split(" | ")]
        cells = [cell + " changed" if column in columns else cell for column, cell in zip(COLUMNS, cells, strict=True)]
        lines[i] = "| " + " | ".join(cells) + " |\n"
    return "".join(lines)


def rebuilt_elsewhere(parity_text):
    """Every row as `expected_row` builds it in a new process, from a copy of tools/parity whose PARITY.md
    is `parity_text`, so a builder that read PARITY.md, even at import, would build from that text."""
    with tempfile.TemporaryDirectory(prefix="unityrs-matrix-") as root:
        copy = Path(root) / "tools" / "parity"
        shutil.copytree(HERE, copy, ignore=shutil.ignore_patterns("__pycache__"))
        (Path(root) / "PARITY.md").write_text(parity_text, encoding="utf-8")
        script = (
            "import json, test_texture_matrix as t\n"
            "print(json.dumps({f.name: t.expected_row(f) for f in t.TextureFormat}))\n"
        )
        result = subprocess.run(
            [sys.executable, "-B", "-c", script], cwd=copy / "tests", capture_output=True, text=True, check=True
        )
    return json.loads(result.stdout)


@unittest.skipUnless(ORACLE, NEEDS_ORACLE)
class AgainstTheOracle(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rows = inventory.texture_rows(PARITY.read_text(encoding="utf-8"))
        cls.expected = {fmt.name: expected_row(fmt) for fmt in TextureFormat}

    def test_texture_matrix_matches_the_oracle(self):
        """Every cell of every row equals what the oracle and the pixel cases make it (section 17)."""
        for name, cells in self.expected.items():
            for column, cell in cells.items():
                with self.subTest(name, column=column):
                    self.assertEqual(self.rows[name][column], cell)

    def test_no_cell_is_built_from_this_file(self):
        """Every cell comes from the oracle and the pixel cases, never from PARITY.md: in a fresh process whose
        PARITY.md has every cell of the matrix changed but the format names, the builders build the same rows,
        and every changed cell then differs from them."""
        self.assertEqual(sorted(next(iter(self.expected.values()))), sorted(COLUMNS))
        self.assertEqual(matrix_errors(self.rows, self.expected), [])
        text = PARITY.read_text(encoding="utf-8")
        others = [column for column in COLUMNS if column != "format"]
        changed = change_cells(text, others)
        everything = sorted((name, column) for name in self.rows for column in others)
        self.assertEqual(sorted(matrix_errors(inventory.texture_rows(changed), self.expected)), everything)
        self.assertEqual(rebuilt_elsewhere(changed), self.expected)
        # The format cells are compared too (keyed by the original names, since the keys change with them).
        renamed = inventory.texture_rows(change_cells(text, ["format"]))
        rows = dict(zip(self.rows, renamed.values(), strict=True))
        self.assertEqual(sorted(matrix_errors(rows, self.expected)), sorted((name, "format") for name in self.rows))
        # The change Codex's review made by hand: DXT1's Switch block size.
        rows = {key: dict(cells) for key, cells in self.rows.items()}
        rows["DXT1"]["gates"] = rows["DXT1"]["gates"].replace("sw 8×4", "sw —")
        self.assertEqual(matrix_errors(rows, self.expected), [("DXT1", "gates")])

    def test_pixels_match_the_matrix(self):
        """Each `works` row decodes to the layout's reference, each `wrong` row to its stated wrong pixels."""
        for fmt in TextureFormat:
            row = self.rows[fmt.name]
            result = RESULT.match(row["UnityPy 1.25.2"]).group(1)
            with self.subTest(fmt.name, result=result):
                if result == "raises":
                    self.assertNotIn(fmt.name, PIXEL_CASES)
                    continue
                self.assertIn(base_name(fmt.name), PIXEL_CASES, "every format that returns an image has a pixel case")
                case = PIXEL_CASES[base_name(fmt.name)][1]()
                got = self.decode_case(fmt, case.data)
                self.check_case(result, case, got, row["mode"])

    def decode_case(self, fmt, data, platform=None, blob=None):
        """Decode `data`; for a crunched format, the unpacker hands it on as the base format's data."""
        calls, patches = stand_in_unpackers()
        for patch in patches:
            patch.start()
        try:
            image = decode(data, fmt, platform=platform, blob=blob)
        finally:
            for patch in reversed(patches):
                patch.stop()
        return image.mode, list(image.getdata())

    def check_case(self, result, case, got, mode):
        if result in ("works", "crunch"):
            self.assertIsNone(case.unitypy, "a row with a stated wrong result is not `works`")
            self.assertEqual(case.mode, mode)
            self.assertEqual(got, (case.mode, case.pixels))
        elif result == "wrong":
            self.assertIsNotNone(case.pixels, "`wrong` needs a reference to be wrong against")
            self.assertEqual(case.unitypy[0], mode)
            self.assertEqual(got, case.unitypy)
            self.assertNotEqual(got, (case.mode, case.pixels))
        else:
            self.assertIsNone(case.pixels, "an `unverified` row has no reference")
            self.assertEqual(case.unitypy[0], mode)
            self.assertEqual(got, case.unitypy)

    def test_switch_and_xbox_data_decode_as_the_gates_say(self):
        """Each format with a pixel case, its data swizzled with the gates cell's block size (Switch) or
        byte-swapped (XBOX360), decodes on that platform to what it decodes to elsewhere."""
        for fmt in TextureFormat:
            row = self.rows[fmt.name]
            result = RESULT.match(row["UnityPy 1.25.2"]).group(1)
            if result == "raises":
                continue
            gates = row["gates"]
            sw = re.search(r"sw (\d+)×(\d+)(?:, read as `(\w+)`)?", gates)
            with self.subTest(fmt.name, platform="Switch"):
                if sw is None:
                    self.assertIn("sw —", gates)
                    with self.assertRaises(KeyError):
                        decode(real_sized(fmt), fmt, platform=BuildTarget.Switch, blob=SWITCH_BLOB)
                else:
                    read_as = TextureFormat[sw.group(3)] if sw.group(3) else fmt
                    case = PIXEL_CASES[read_as.name][1]()
                    swizzled = swizzle(case.data, read_as, int(sw.group(1)), int(sw.group(2)))
                    got = self.decode_case(fmt, swizzled, BuildTarget.Switch, SWITCH_BLOB)
                    read_as_result = RESULT.match(self.rows[read_as.name]["UnityPy 1.25.2"]).group(1)
                    self.check_case(read_as_result, case, got, self.rows[read_as.name]["mode"])
            with self.subTest(fmt.name, platform="XBOX360"):
                case = PIXEL_CASES[base_name(fmt.name)][1]()
                data = case.data
                if "xbox" in gates.split("; "):
                    data = bytes(b for i in range(0, len(data), 2) for b in data[i : i + 2][::-1])
                got = self.decode_case(fmt, data, BuildTarget.XBOX360)
                self.check_case(result, case, got, row["mode"])

    def test_half_values_outside_a_byte_raise(self):
        """RHalf and RGBAHalf: what RESULT_NOTES and RAISE_REASONS say beyond the 0x30 run."""

        def halves(fmt, values, pixels=W * H):
            per_pixel = {"RHalf": 1, "RGBAHalf": 4}[fmt.name]
            data = struct.pack(f"<{pixels * per_pixel}e", *(values * (pixels * per_pixel))[: pixels * per_pixel])
            return decode(data, fmt)

        for fmt in (TextureFormat.RHalf, TextureFormat.RGBAHalf):
            # Three times RHalf's data reaches Pillow's missing mode R; RGBAHalf has enough of its own.
            enough = 3 * W * H if fmt == TextureFormat.RHalf else W * H
            for value, error in ((1.0, ValueError), (-1 / 256, ValueError), (float("nan"), ValueError)):
                with self.subTest(fmt.name, value=value), self.assertRaises(error):
                    halves(fmt, [value], enough)
            with self.subTest(fmt.name, value="inf"), self.assertRaises(OverflowError):
                halves(fmt, [float("inf")], enough)
            for value in (255 / 256, -1 / 512):  # int(h * 256) still 0-255
                with self.subTest(fmt.name, value=value):
                    if fmt == TextureFormat.RHalf:
                        with self.assertRaisesRegex(ValueError, "^unrecognized image mode$"):
                            halves(fmt, [value], enough)
                    else:
                        self.assertEqual(halves(fmt, [value]).mode, "RGB")
        # Real-sized RHalf data with a 1.0 first: the ValueError comes before the data runs out.
        with self.assertRaises(ValueError):
            halves(TextureFormat.RHalf, [1.0])

    def test_array_8bit_targets_have_references(self):
        """Section 19.2's 8-bit integer arrays decode as the 8-bit format UnityPy maps the UNorm or SNorm graphics
        format of the same channels to, and each of those is Tier A or has a section 17.1 reference."""
        from UnityPy.enums.GraphicsFormat import GRAPHICS_TO_TEXTURE_MAP, GraphicsFormat

        targets = {}
        for graphics in GraphicsFormat:
            if graphics.name.startswith("R8") and graphics.name.endswith(("_UInt", "_SInt")):
                norm = graphics.name.replace("_UInt", "_UNorm").replace("_SInt", "_SNorm")
                targets[graphics.name] = GRAPHICS_TO_TEXTURE_MAP[GraphicsFormat[norm]].name
        self.assertEqual(
            targets,
            {
                "R8_UInt": "R8",
                "R8G8_UInt": "RG16",
                "R8G8B8_UInt": "RGB24",
                "R8G8B8A8_UInt": "RGBA32",
                "R8_SInt": "R8_SIGNED",
                "R8G8_SInt": "RG16_SIGNED",
                "R8G8B8_SInt": "RGB24_SIGNED",
                "R8G8B8A8_SInt": "RGBA32_SIGNED",
            },
        )
        for target in targets.values():
            with self.subTest(target):
                row = self.rows[target]
                self.assertTrue(row["tier"] == "A" or target in references.TIER_B, row["tier"])

    def test_rgb48_reference_is_the_high_byte(self):
        """Section 19.2's RGB48 reference: each channel's high byte, what Pillow's `RGB;16L` gives."""
        case = rgb48()
        image = Image.frombytes("RGB", (W, H), case.data, "raw", "RGB;16L").transpose(Image.Transpose.FLIP_TOP_BOTTOM)
        self.assertEqual(list(image.getdata()), case.pixels)
        rounded = [tuple(round(v * 255 / 65535) for v in pixel) for pixel in RGB48_PIXELS]
        self.assertNotEqual(rounded, [tuple(v >> 8 for v in pixel) for pixel in RGB48_PIXELS])


class TierBReferences(unittest.TestCase):
    """Section 17.1's references hold the values it states (standard library; the BC6H libraries need the oracle)."""

    def one(self, name, data, width=1, height=1):
        mode, pixels = references.decode(name, data, width, height)
        return mode, pixels[0] if width * height == 1 else pixels

    def test_tier_b_references_hold_their_stated_values(self):
        # Half and float: clamped to [0, 1], times 255, rounded half up; NaN gives 0.
        cases = {0.5: 128, 1.0: 255, 2.0: 255, float("inf"): 255, -1.0: 0, -0.0: 0, float("nan"): 0, 1 / 255: 1}
        for value, byte in cases.items():
            with self.subTest(value=value):
                self.assertEqual(self.one("RHalf", struct.pack("<e", value)), ("RGB", (byte, 0, 0)))
                self.assertEqual(self.one("RFloat", struct.pack("<f", value)), ("RGB", (byte, 0, 0)))
        self.assertEqual(self.one("RGHalf", struct.pack("<2e", 0.25, 0.75)), ("RGB", (64, 191, 0)))
        self.assertEqual(self.one("RGBFloat", struct.pack("<3f", 0, 0.5, 1)), ("RGB", (0, 128, 255)))
        self.assertEqual(self.one("RGBAFloat", struct.pack("<4f", 1, 0, 0.5, 0.25)), ("RGBA", (255, 0, 128, 64)))
        # Unsigned: the high byte. Signed: offset by 2^(n-1), then the high byte.
        self.assertEqual(self.one("R16", struct.pack("<H", 0xFF00)), ("RGB", (255, 0, 0)))
        self.assertEqual(self.one("R16", struct.pack("<H", 0x00FF)), ("RGB", (0, 0, 0)))
        self.assertEqual(self.one("RG16", bytes([7, 200])), ("RGB", (7, 200, 0)))
        self.assertEqual(
            self.one("RGBA64", struct.pack("<4H", 0xFFFF, 0x8000, 0x7FFF, 0)), ("RGBA", (255, 128, 127, 0))
        )
        self.assertEqual(self.one("RGB24_SIGNED", struct.pack("<3b", -128, 0, 127)), ("RGB", (0, 128, 255)))
        self.assertEqual(self.one("RGBA32_SIGNED", struct.pack("<4b", -1, 1, -127, 64)), ("RGBA", (127, 129, 1, 192)))
        self.assertEqual(self.one("R8_SIGNED", struct.pack("<b", -128)), ("RGB", (0, 0, 0)))
        self.assertEqual(self.one("RG16_SIGNED", struct.pack("<2b", 0, 127)), ("RGB", (128, 255, 0)))
        self.assertEqual(self.one("RGB48_SIGNED", struct.pack("<3h", -32768, 0, 32767)), ("RGB", (0, 128, 255)))
        self.assertEqual(self.one("R16_SIGNED", struct.pack("<h", -1)), ("RGB", (127, 0, 0)))
        self.assertEqual(self.one("RG32_SIGNED", struct.pack("<2h", 256, -256)), ("RGB", (129, 127, 0)))
        self.assertEqual(self.one("RG32", struct.pack("<2H", 0x1234, 0xABCD)), ("RGB", (0x12, 0xAB, 0)))
        self.assertEqual(self.one("RGBA64_SIGNED", struct.pack("<4h", 0, 0, 0, 32767)), ("RGBA", (128, 128, 128, 255)))
        # RGB9e5Float: exponent 15, red 256: 256 * 2^(15 - 24) = 0.5.
        self.assertEqual(self.one("RGB9e5Float", struct.pack("<I", 15 << 27 | 256)), ("RGB", (128, 0, 0)))
        self.assertEqual(references.rgb9e5_to_floats(16 << 27 | 256 << 18 | 128 << 9 | 1), (2**-8, 0.5, 1.0))
        # YUY2: BT.601 limited range; one pair of pixels shares U and V.
        pair = self.one("YUY2", bytes([16, 128, 235, 128]), width=2)
        self.assertEqual(pair, ("RGB", [(0, 0, 0), (255, 255, 255)]))
        self.assertEqual(references.yuv_to_rgb(81, 90, 240), (255, 0, 0))  # BT.601 red
        with self.assertRaises(NotImplementedError):
            references.decode("YUY2", bytes(4), 1, 1)
        with self.assertRaises(NotImplementedError):
            references.decode("ARGBFloat", bytes(16), 1, 1)
        # Every image comes back flipped top to bottom.
        mode, pixels = references.decode("R16", struct.pack("<2H", 0x0100, 0x0200), 1, 2)
        self.assertEqual(pixels, [(2, 0, 0), (1, 0, 0)])

    def test_short_data_raises_for_every_reference(self):
        # What a texture holds: its pixels, or BC6H's whole 4x4 blocks.
        self.assertEqual(references.needed_bytes("RGBAFloat", 3, 2), 96)
        self.assertEqual(references.needed_bytes("YUY2", 4, 2), 16)
        self.assertEqual(references.needed_bytes("RGB9e5Float", 2, 2), 16)
        self.assertEqual(references.needed_bytes("BC6H", 5, 4), 32)
        for name in sorted(references.TIER_B):
            width, height = (4, 2) if name == "YUY2" else (5, 3)  # an odd width is Tier U for YUY2
            needed = references.needed_bytes(name, width, height)
            with self.subTest(name):
                # Exactly enough decodes, and bytes past that are ignored.
                full = references.decode(name, bytes(needed), width, height)
                self.assertEqual(len(full[1]), width * height)
                self.assertEqual(references.decode(name, bytes(needed + 16), width, height), full)
                # One byte short, or none: an error, never pixels for bytes that do not exist.
                for short in (needed - 1, 0):
                    with self.assertRaisesRegex(ValueError, "^not enough image data"):
                        references.decode(name, bytes(short), width, height)

    def test_bc6h_reference_is_the_specification(self):
        """BC6H's reference decodes as the Khronos Data Format Specification 1.3, section 20.2, says."""

        def block(e0, e1, index=0):
            # Mode 3 (mode bits 00011): one subset, 10-bit endpoints stored as they are; texel 0, the anchor, keeps
            # 3 index bits and the others 4, and every texel takes `index`.
            bits, shift = 0b00011, 5
            for channel in (*e0, *e1):
                bits |= channel << shift
                shift += 10
            bits |= (index & 7) << shift
            for texel in range(1, 16):
                bits |= index << (shift + 3 + 4 * (texel - 1))
            return bits.to_bytes(16, "little")

        def half(endpoint):  # the unsigned unquantize of a 10-bit endpoint, and the final 31/64
            return ((((endpoint << 16) + 0x8000) >> 10) * 31) >> 6

        self.assertEqual([hex(half(e)) for e in (400, 488, 500)], ["0x307f", "0x3b27", "0x3c9b"])
        mode, pixels = references.decode("BC6H", block((400, 488, 500), (0, 0, 0)), 4, 4)
        self.assertEqual((mode, set(pixels)), ("RGB", {(36, 228, 255)}))
        # Cropped from whole 4x4 blocks, as UnityPy pads every BC format.
        self.assertEqual(
            references.decode("BC6H", block((400, 488, 500), (0, 0, 0)), 3, 2), ("RGB", [(36, 228, 255)] * 6)
        )

        # An endpoint equal to 2^(bits-1) unquantizes by the formula: 512 of 10 bits to 32800, not 0xFFFF. With
        # weight 34 (index 8) towards 0: (32800 * 30 + 32) >> 6 = 15375, times 31/64 = 0x1d17, 0.00477, red 1.
        self.assertEqual(references.bc6h_unquantize(512, 10), 32800)
        self.assertEqual(references.bc6h_unquantize(1023, 10), 0xFFFF)
        reviewer = block((512, 100, 100), (0, 0, 0), 8)
        texels = references.bc6h_block(reviewer)
        self.assertEqual(texels[1:], [(0x1D17, 0x05B4, 0x05B4)] * 15)
        self.assertEqual(texels[0], (0x3E0F, 0x0C2B, 0x0C2B))  # the anchor's 3 bits hold 0: endpoint 0
        mode, pixels = references.decode("BC6H", reviewer, 4, 4)
        self.assertEqual(pixels[12:], [(255, 0, 0), (1, 0, 0), (1, 0, 0), (1, 0, 0)])  # flipped: texel 0 is last
        # Equation 2 rounds (+ 32): weight 4 (index 1) from 0 towards 1023 (0xFFFF) gives (0xFFFF * 4 + 32) >> 6
        # = 4096, times 31/64 = 0x7c0; without the + 32, 4095 and 0x7bf.
        ramp = block((0, 0, 0), (1023, 1023, 1023), 1)
        self.assertEqual(references.bc6h_block(ramp)[1], (0x7C0,) * 3)
        self.assertEqual(references.bc6h_block(ramp, rounding=0)[1], (0x7BF,) * 3)

    def test_bc6h_libraries_depart_from_the_specification(self):
        """Section 19.2: texture2ddecoder 1.0.5 and Pillow 9.5.0 each equal section 20.2's decoding with one
        departure, on random blocks of every mode, and the departure shows on the reviewer's block."""
        if not ORACLE:
            self.skipTest("needs texture2ddecoder 1.0.5 and Pillow 9.5.0 (the oracle image)")

        def bimg_unquantize(value, bits):  # texture2ddecoder's src/Texture2DDecoder/bcn.cpp, unsigned
            if bits >= 15:
                return value
            if value == 0:
                return 0
            if value == 1 << (bits - 1):  # section 20.2 tests (1 << bits) - 1
                return 0xFFFF
            return ((value << 15) + 0x4000) >> (bits - 1)

        def as_bytes(texels, convert):
            return [tuple(convert(references.half_bits_to_float(h)) for h in texel) for texel in texels]

        def truncate(value):  # Pillow's bc6_clamp: clamped to [0, 1], then (UINT8)(value * 255.0f)
            return 255 if value > 1 else math.floor(value * 255)

        def t2d(data):
            bgra = texture2ddecoder.decode_bc6(data, 4, 4)
            return [(bgra[i + 2], bgra[i + 1], bgra[i]) for i in range(0, 64, 4)]

        def pillow(data):
            return list(Image.frombytes("RGB", (4, 4), data, "bcn", 6).getdata())

        rng = random.Random(29)
        modes = sorted(references.BC6H_MODES) + [19, 23, 27, 31]  # the last four are reserved
        differs = {"texture2ddecoder": 0, "Pillow": 0}
        for mode in modes:
            for _ in range(60):
                width = 2 if mode < 2 else 5
                data = (rng.getrandbits(128) >> width << width | mode).to_bytes(16, "little")
                spec = as_bytes(references.bc6h_block(data), references.float_to_byte)
                with self.subTest(mode=mode, data=data.hex()):
                    self.assertEqual(
                        t2d(data), as_bytes(references.bc6h_block(data, bimg_unquantize), references.float_to_byte)
                    )
                    self.assertEqual(pillow(data), as_bytes(references.bc6h_block(data, rounding=0), truncate))
                differs["texture2ddecoder"] += t2d(data) != spec
                differs["Pillow"] += pillow(data) != spec
        self.assertTrue(all(differs.values()), differs)
        # The reviewer's block: red endpoint 0 at 512 of 10 bits, endpoint 1 at 0, index 8 (texel 1 onwards).
        bits = 0b00011 | 512 << 5 | 100 << 15 | 100 << 25
        for texel in range(1, 16):
            bits |= 8 << (65 + 3 + 4 * (texel - 1))
        data = bits.to_bytes(16, "little")
        self.assertEqual(as_bytes(references.bc6h_block(data), references.float_to_byte)[1], (1, 0, 0))
        self.assertEqual(t2d(data)[1], (195, 0, 0))
        self.assertEqual(pillow(data)[1], (1, 0, 0))


def swizzle(data, fmt, block_w, block_h):
    """`data`, an 8x8 texture of `fmt`, laid out at the size the Switch path pads to and swizzled."""
    unit_w, unit_h, size = unit(fmt)
    padded_w, padded_h = TextureSwizzler.get_padded_texture_size(W, H, block_w, block_h, GOBS)
    across, down = -(-W // unit_w), -(-H // unit_h)
    padded_across, padded_down = padded_w // unit_w, padded_h // unit_h
    linear = bytearray(padded_across * padded_down * size)
    for row in range(down):
        source = data[row * across * size : (row + 1) * across * size]
        linear[row * padded_across * size : row * padded_across * size + len(source)] = source
    return bytes(TextureSwizzler.swizzle(linear, padded_w, padded_h, block_w, block_h, GOBS))


if __name__ == "__main__":
    unittest.main()
