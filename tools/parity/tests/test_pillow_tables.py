"""PARITY.md sections 15.8 and 18.1 hold exactly what `pillow_tables.py` prints in the oracle image.

Needs UnityPy 1.25.2 and Pillow 9.5.0 (the oracle image); skipped elsewhere.
"""

import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import inventory  # noqa: E402
import pillow_tables  # noqa: E402

try:
    import PIL
    import UnityPy

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION and PIL.__version__ == "9.5.0"
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False


def block(text, first, last):
    """The lines of `text` from the one starting `first` through the one starting `last`."""
    lines = text.splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith(first))
    end = next(i for i in range(start, len(lines)) if lines[i].startswith(last))
    return "\n".join(lines[start : end + 1])


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 and Pillow 9.5.0 (the oracle image)")
class PillowTables(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = inventory.DEFAULT_PARITY.read_text(encoding="utf-8")

    def heading_before(self, table):
        """The last `###` heading above `table` in PARITY.md."""
        above = self.text[: self.text.index(table)]
        return [line for line in above.splitlines() if line.startswith("### ")][-1]

    def test_packers_table_is_what_pillow_does(self):
        """Section 18.1: the raw mode each target packs, and the image modes Pillow packs into each."""
        printed = pillow_tables.packers()
        self.assertEqual(block(self.text, "| raw mode |", "`Image.convert"), printed)
        self.assertEqual(self.heading_before(printed), "### 18.1 Image modes `set_image` packs")

    def test_writers_table_is_what_pillow_does(self):
        """Section 15.8: per writer, what `Image.save` does with RGBA, RGB, L and LA, what the size and the file name
        change, and what unityrs writes."""
        # The modes: a decoded texture's, and what `mask_sprite`'s `putalpha` makes of a sprite that is not RGBA.
        from PIL import Image

        made = {}
        for mode in ("RGB", "L"):
            sprite = Image.new(mode, (2, 2))
            sprite.putalpha(Image.new("1", (2, 2)))
            made[mode] = sprite.mode
        self.assertEqual(made, {"RGB": "RGBA", "L": "LA"})
        self.assertEqual(pillow_tables.SAVED_MODES, ("RGBA", "RGB", "L", "LA"))
        printed = pillow_tables.writers()
        self.assertEqual(block(self.text, "| format | extensions |", printed.splitlines()[-1][:12]), printed)
        self.assertEqual(self.heading_before(printed), "### 15.8 Image files by extension")


if __name__ == "__main__":
    unittest.main()
