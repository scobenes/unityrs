"""B2b's measured write tiers do not promote unmeasured Editor reading."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]


class B2bWriting(unittest.TestCase):
    def test_head_writes_are_l4_replays_and_editor_reads_remain_targets(self):
        parity = (ROOT / "PARITY.md").read_text()
        section = parity.split("\n## 20. HEAD tier\n", 1)[1].split("\n## ", 1)[0]
        for item in ("SerializedFile formats 23–26 save", "`lz4hc` packer", "save of a format-9 bundle"):
            row = next(line for line in section.splitlines() if line.startswith(f"| {item} |"))
            self.assertIn("| L4 |", row)
            self.assertIn("verified P3 B2b", row)
            self.assertIn("head_save_", row)
            self.assertIn("crates/unityrs-core/tests/", row)
        for item in ("SerializedFile v23", "SerializedFile v24, v25", "SerializedFile v26", "bundle format 9"):
            row = next(line for line in section.splitlines() if line.startswith(f"| {item} |"))
            self.assertIn("target (Editor fixtures)", row)
        refusal = next(line for line in parity.splitlines()
                       if line.startswith("| SerializedFile save above format 22 |"))
        self.assertIn("above 26", refusal)
        self.assertIn("HeadTierWritesNotImplemented", refusal)

    def test_brotli_writing_names_the_reloadable_tier_c_acceptance(self):
        parity = (ROOT / "PARITY.md").read_text()
        row = next(line for line in parity.splitlines()
                   if line.startswith("| `WebFile.save` with brotli |"))
        for required in ("C (writing)", "offset 32", "decompressed bytes", "both readers", "webfile_save.rs"):
            self.assertIn(required, row)
        design = (ROOT / "docs/DESIGN.md").read_text()
        writing = design.split("| Tier C (writing) | acceptance |", 1)[1].split("\n\n", 1)[0]
        self.assertIn("`WebFile.save` with brotli", writing)
        self.assertIn("offset 32", writing)
