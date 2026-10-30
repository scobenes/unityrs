"""PARITY.md and DESIGN.md agree on what a tier covers where a zero side, an image mode, a missing characterisation
or a walk past a UnityCN block decides it (standard library).

Each guard reads both files as text. The behaviour each one keeps the wording to is pinned by the row tests the cells
name (`test_rows_images.py`, `test_rows_textures.py`, `test_rows_internals.py`).
"""

import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from test_parity_rows import UNSUPPORTED_RULE  # noqa: E402

import inventory  # noqa: E402

ROOT = HERE.parent.parent
SENTENCE_END = re.compile(r"(?<=\.)\s+(?=[A-Z`])")


def sentences(text, skip_items=False):
    """(line, sentence) for every sentence of every table cell or line of `text`; with `skip_items`, a table row's
    first cell, which names the item, is left out."""
    for line in text.splitlines():
        cells = line.split(" | ")
        if skip_items and line.startswith("| "):
            cells = cells[1:]
        for cell in cells:
            for sentence in SENTENCE_END.split(cell):
                yield line, sentence


def rows_by_tier(text):
    """(cells, tier) for every row of a table of `text` with a tier and a status column."""
    for header, rows in inventory.tables(text):
        if {"tier", "status"} <= set(header):
            for cells in rows:
                yield cells, cells[header.index("tier")]


class DocTiers(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.parity = (ROOT / "PARITY.md").read_text(encoding="utf-8")
        cls.design = (ROOT / "docs" / "DESIGN.md").read_text(encoding="utf-8")

    def unsupported(self):
        """DESIGN.md section 3's Tier U bullet with its list."""
        return self.design.split(f"- **{UNSUPPORTED_RULE}.**", 1)[1].split("\n- **")[0]

    def test_not_characterised_is_a_tier_u_ground(self):
        """Tier U covers a case no test has run UnityPy on as well as one that waits for a sample and a reference:
        both definitions say so, every PARITY.md sentence that calls a case not characterised files it as Tier U,
        every such DESIGN.md line sits in the Tier U list, and section 18's U list names the zero-size image on a
        Switch-swizzled texture. A row of tier — has no unityrs side, so no input to refuse: a case it calls not
        characterised is not Tier U, both definitions say so, and its sentence names no U."""
        definition = next(line for line in self.parity.splitlines() if line.startswith("| U | "))
        self.assertIn("for a case its row calls not characterised (no test has run UnityPy on it)", definition)
        self.assertIn("where the row has a unityrs side (any tier but —)", definition)
        self.assertIn("until a test does and the row takes the tier that run decides", definition)
        dash = next(line for line in self.parity.splitlines() if line.startswith("| — | "))
        self.assertIn("A case such a row calls not characterised is not Tier U", dash)
        self.assertRegex(self.design, rf"\n- \*\*{re.escape(UNSUPPORTED_RULE)}\.\*\* ")
        ground = "A case `PARITY.md` calls not characterised is one no test has run UnityPy on"
        self.assertIn(ground, self.unsupported())
        self.assertIn("A row `PARITY.md` files under tier —, Python-only with no unityrs side", self.unsupported())
        python_only = {tuple(cells) for cells, tier in rows_by_tier(self.parity) if tier == "—"}
        seen = exempt = 0
        for line, sentence in sentences(self.parity):
            if "not characterised" in sentence and line not in (definition, dash):
                with self.subTest("PARITY.md", sentence=sentence[:80]):
                    if tuple(inventory.table_cells(line) or ()) in python_only:
                        exempt += 1
                        self.assertNotRegex(sentence, r"\bU\b", "a tier — row files no case as U")
                    else:
                        seen += 1
                        self.assertRegex(sentence, r"\bU\b")
        self.assertGreaterEqual(seen, 7)
        self.assertGreaterEqual(exempt, 1)
        reading = self.design.split("**Reading**", 1)[1].split("**Writing**", 1)[0].splitlines()
        lead = reading.index(next(line for line in reading if line.startswith(f"- **{UNSUPPORTED_RULE}.**")))
        end = next(i for i in range(lead + 1, len(reading)) if reading[i].startswith("- **"))
        outside = [line[:60] for i, line in enumerate(reading) if "not characterised" in line and not lead <= i < end]
        self.assertEqual(outside, [], "a DESIGN.md line outside the Tier U list calls a case not characterised")
        self.assertGreaterEqual(sum("not characterised" in line for line in reading[lead:end]), 3)
        writing = self.parity.split("\n## 18. Writing\n", 1)[1].split("\n| item |", 1)[0]
        u_list = writing.split("the item is Tier U and unityrs refuses it:", 1)[1]
        switch = "`set_image` of a zero-size image on a Switch-swizzled texture, which is not characterised"
        self.assertIn(switch, u_list)

    def test_a_zero_side_reaches_no_packer(self):
        """Pillow's `tobytes` returns `b""` for an image with a zero side before it looks a packer up, so "No packer
        found" and `convert("RGBA")`'s refusal decide nothing for one (section 18.1). So every sentence that says an
        image mode Pillow cannot pack raises, or sends it to Tier C (writing) or U, says it of an image with no zero
        side; an item's name aside, and the generated line that lists the modes `convert("RGBA")` itself refuses."""
        rule = re.compile(
            r"No packer found|cannot pack|Pillow cannot convert|cannot convert to RGBA|it cannot convert"
            r"|convert\(\"RGBA\"\)` raises for\b(?!:)|grey PNG|grey one|\bRGB`? image\b[^.;|]*\braises\b"
            r"|^- \*\*Where it does not\*\*"
        )
        seen = 0
        for name, text in (("PARITY.md", self.parity), ("DESIGN.md", self.design)):
            for _, sentence in sentences(text, skip_items=True):
                if rule.search(sentence):
                    seen += 1
                    with self.subTest(name, sentence=sentence[:80]):
                        self.assertRegex(sentence, r"\bno zero side\b|\b4×4\b")
        self.assertGreaterEqual(seen, 12)

    def test_xbox360_refuses_no_zero_side(self):
        """An image with a zero side has no texel to swap, and the reader returns its empty image before the swap, so
        section 19.1's zero-size row takes it on XBOX360 too: every sentence that sends `set_image` on XBOX360's
        swapped formats to Tier U names an image with no zero side, and so does the 19.2 row's tier."""
        refusal = re.compile(r"\bU\b|Tier U|Unsupported|refuses")
        seen = 0
        for name, text in (("PARITY.md", self.parity), ("DESIGN.md", self.design)):
            for _, sentence in sentences(text):
                if "XBOX360" in sentence and "swap" in sentence and refusal.search(sentence):
                    seen += 1
                    with self.subTest(name, sentence=sentence[:80]):
                        self.assertIn("no zero side", sentence)
        self.assertGreaterEqual(seen, 3)
        bullets = [line for line in self.unsupported().splitlines() if "XBOX360" in line and "swap" in line]
        self.assertEqual(len(bullets), 1)
        self.assertIn("no zero side", SENTENCE_END.split(bullets[0])[0])
        row = next(line for line in self.parity.splitlines() if line.startswith("| `set_image` on XBOX360 |"))
        self.assertEqual(row.split(" | ")[2], "U for an image with no zero side (one with a zero side: section 19.1)")

    def test_a_zero_side_image_needs_its_fetch(self):
        """`Texture2D.image` fetches the data before `parse_image_data` returns its 0×0 image for a zero side (section
        14.2), so a sentence that says the image of a texture with a zero side is that 0×0 image says where the fetch
        succeeds."""
        claim = re.compile(r"(?:`Texture2D\.image`|`obj\.image`)[^.]*\bis (?:then )?a 0×0\b")
        seen = 0
        for name, text in (("PARITY.md", self.parity), ("DESIGN.md", self.design)):
            for _, sentence in sentences(text):
                if claim.search(sentence):
                    seen += 1
                    with self.subTest(name, sentence=sentence[:80]):
                        self.assertIn("where its data fetch succeeds", sentence)
        self.assertGreaterEqual(seen, 2)

    def test_no_file_for_any_zero_side(self):
        """Pillow cannot write an image with a zero side as a PNG, a texture's 0×0 image or a sprite's whose texture
        rect rounds to a zero side alike, so every clause that says unityrs writes no file for such an image names
        both."""
        seen = 0
        for _, sentence in sentences(self.parity):
            for clause in sentence.split("; "):
                if "writes no file" in clause and re.search(r"0×0|zero side", clause):
                    seen += 1
                    with self.subTest(clause=clause[:80]):
                        self.assertIn("zero side", clause)
                        self.assertIn("sprite", clause)
        self.assertGreaterEqual(seen, 3)

    def test_a_walk_past_the_terminator_is_u_for_want_of_a_sample(self):
        """UnityPyBoost reads the byte just past a UnityCN block as 0, the terminator of the bytes object it returns,
        and decrypts it there: a walk that reads that byte and stops there returns the bytes the Python loop holds when
        it raises, the block decrypted, and a walk that goes on reads and writes past the bytes object, which no run
        can pin (`test_rows_internals.py` decrypt_block_past_the_block_end). So section 12's two rows and DESIGN.md's
        Tier U bullet read that byte as 0 and file the walk past it as U for want of a sample, never as a case a test
        could characterise, and the Boost row says what it returns."""
        walk = "A; U for a walk that runs more than one byte past the block"
        decrypt = [cells[1] for cells, tier in rows_by_tier(self.parity) if tier == walk]
        self.assertEqual(len(decrypt), 2)
        bullets = [line for line in self.unsupported().splitlines() if line.startswith("  - a UnityCN block")]
        self.assertEqual(len(bullets), 1)
        for text in (*decrypt, *bullets):
            with self.subTest(text=text[:60]):
                self.assertRegex(text, r"byte just past (?:the block|it)(?: read)? as 0\b")
                self.assertIn("reads and writes memory past the bytes object", text)
                self.assertIn("no sample shows such a block", text)
                self.assertNotIn("not characterised", text)
        for cell in decrypt:
            self.assertIn("(Tier U", cell)
        boost = next(cell for cell in decrypt if cell.startswith("the C version of the loop"))
        self.assertIn("returns with no error the bytes the Python loop holds when it raises", boost)
        self.assertNotIn("returns the block with no error", boost)


if __name__ == "__main__":
    unittest.main()
