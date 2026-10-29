"""scripts/third_party_licenses_check.py on the repository's own about.toml and deny.toml, and on edits of them.

Standard library only (tomllib, Python 3.11 or later).
"""

import copy
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import third_party_licenses_check as T  # noqa: E402


def load(name):
    with open(ROOT / name, "rb") as fh:
        return tomllib.load(fh)


ABOUT = load("about.toml")
DENY = load("deny.toml")


class ConfigTests(unittest.TestCase):
    def problems(self, about=None, deny=None):
        return T.config_problems(about if about is not None else ABOUT, deny if deny is not None else DENY)

    def test_the_repository_agrees(self):
        self.assertEqual(self.problems(), [])

    def test_the_allow_lists_must_match_in_order(self):
        deny = copy.deepcopy(DENY)
        deny["licenses"]["allow"] = [*deny["licenses"]["allow"][1:], deny["licenses"]["allow"][0]]
        self.assertIn("keep them equal, in the same order", "\n".join(self.problems(deny=deny)))

    def test_a_licence_accepted_for_one_crate_needs_its_exception(self):
        # A per-crate licence cargo-deny never hears of (the review's case).
        about = copy.deepcopy(ABOUT)
        about["liblzma-sys"]["accepted"] = ["0BSD", "GPL-3.0-or-later"]
        out = "\n".join(self.problems(about=about))
        self.assertIn("liblzma-sys: about.toml accepts ['0BSD', 'GPL-3.0-or-later']", out)

        about = copy.deepcopy(ABOUT)
        about["r-efi"]["accepted"] = ["LGPL-2.1-or-later"]
        self.assertIn("r-efi: about.toml accepts ['LGPL-2.1-or-later']", "\n".join(self.problems(about=about)))

    def test_an_exception_needs_the_same_licences_accepted(self):
        for change in (
            lambda d: d["licenses"]["exceptions"][0]["allow"].append("GPL-3.0-or-later"),
            lambda d: d["licenses"]["exceptions"].append({"crate": "libc", "allow": ["LGPL-2.1-or-later"]}),
            lambda d: d["licenses"]["exceptions"].append({"crate": "liblzma-sys", "allow": ["0BSD"]}),
            lambda d: d["licenses"].pop("exceptions"),
        ):
            deny = copy.deepcopy(DENY)
            change(deny)
            with self.subTest(deny["licenses"].get("exceptions")):
                self.assertTrue(self.problems(deny=deny))

    def test_each_clarification_must_be_in_both(self):
        changes = {
            "deny lacks it": lambda a, d: d["licenses"]["clarify"].pop(0),
            "about lacks it": lambda a, d: a.pop("r-efi"),
            "another expression": lambda a, d: d["licenses"]["clarify"][0].update(expression="MIT OR Apache-2.0"),
            "other files": lambda a, d: d["licenses"]["clarify"][0]["license-files"].pop(),
            "about's expression": lambda a, d: a["liblzma-sys"]["clarify"].update(license="MIT AND 0BSD"),
            "a second one": lambda a, d: d["licenses"]["clarify"].append(copy.deepcopy(d["licenses"]["clarify"][0])),
        }
        for label, change in changes.items():
            about, deny = copy.deepcopy(ABOUT), copy.deepcopy(DENY)
            change(about, deny)
            with self.subTest(label):
                self.assertIn("clarify it the same way in both", "\n".join(self.problems(about, deny)))

    def test_the_command_line(self):
        script = SCRIPTS / "third_party_licenses_check.py"
        ok = subprocess.run([sys.executable, str(script), "config"], capture_output=True, text=True)
        self.assertEqual((ok.returncode, ok.stderr), (0, ""))
        with tempfile.TemporaryDirectory() as tmp:
            about = Path(tmp) / "about.toml"
            about.write_text(
                (ROOT / "about.toml").read_text().replace('accepted = ["0BSD"]', 'accepted = ["0BSD", "GPL-3.0-only"]')
            )
            bad = subprocess.run(
                [sys.executable, str(script), "config", str(about), str(ROOT / "deny.toml")],
                capture_output=True,
                text=True,
            )
            self.assertEqual(bad.returncode, 1)
            self.assertIn("GPL-3.0-only", bad.stderr)
        self.assertEqual(subprocess.run([sys.executable, str(script)], capture_output=True).returncode, 2)


class NoticeTests(unittest.TestCase):
    def test_the_committed_notices_name_their_holders(self):
        self.assertEqual(T.notice_problems((ROOT / "THIRD_PARTY_LICENSES").read_text(encoding="utf-8")), [])

    def test_a_template_notice_fails(self):
        for line in (
            "Copyright (c) <year> <copyright holders>",
            "Copyright (c) <year> <owner>. All rights reserved.",
        ):
            with self.subTest(line):
                out = T.notice_problems(f"MIT License\n\n{line}\n\nPermission is hereby granted\n")
                self.assertEqual(len(out), 1)
                self.assertIn("line 3:", out[0])

    def test_real_notices_pass(self):
        text = "Copyright (c) [2021] [Marvin Countryman]\n   Copyright [yyyy] [name of copyright owner]\n"
        self.assertEqual(T.notice_problems(text), [])


if __name__ == "__main__":
    unittest.main()
