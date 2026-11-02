"""scripts/third_party_licenses_check.py on the repository's own about.toml and deny.toml, and on edits of them.

Standard library only (tomllib, Python 3.11 or later).
"""

import copy
import sys
import tomllib
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(ROOT / "tools" / "parity"))

import child  # noqa: E402
import scratch  # noqa: E402
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
        ok = child.run([sys.executable, str(script), "config"], capture_output=True, text=True)
        self.assertEqual((ok.returncode, ok.stderr), (0, ""))
        with scratch.folder("unityrs-licenses-") as tmp:
            about = Path(tmp) / "about.toml"
            about.write_text(
                (ROOT / "about.toml").read_text().replace('accepted = ["0BSD"]', 'accepted = ["0BSD", "GPL-3.0-only"]')
            )
            bad = child.run(
                [sys.executable, str(script), "config", str(about), str(ROOT / "deny.toml")],
                capture_output=True,
                text=True,
            )
            self.assertEqual(bad.returncode, 1)
            self.assertIn("GPL-3.0-only", bad.stderr)
        self.assertEqual(child.run([sys.executable, str(script)], capture_output=True).returncode, 2)


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


RULE = "-" * 80
BANNER = "=" * 80
LICENCE = "Copyright (c) 2020 Example Author\n\nPermission is granted, on condition the notice is kept.\n"
HEADER = "// Ported from libx 1.0 (BSD-2-Clause): lib/x.c. See THIRD_PARTY_LICENSES.\n"


def write_tree(
    root: Path,
    crate_licence: str,
    header: str = HEADER,
    section_licence: str = "BSD-2-Clause",
    licence_file: bool = True,
):
    """A workspace of one crate whose src/x.rs ports `libx 1.0`, and its about.hbs."""
    (root / "crates" / "demo" / "src").mkdir(parents=True)
    (root / "Cargo.toml").write_text('[workspace.package]\nlicense = "MIT OR Apache-2.0"\n')
    (root / "crates" / "demo" / "Cargo.toml").write_text(f'[package]\nname = "demo"\nlicense = "{crate_licence}"\n')
    (root / "crates" / "demo" / "src" / "x.rs").write_text(header + "//! The port.\npub fn x() {}\n")
    (root / "crates" / "demo" / "src" / "table.rs").write_text("// @generated by a tool; do not edit.\n")
    if licence_file:
        (root / "crates" / "demo" / "LICENSE-X").write_text(LICENCE)
    (root / "about.hbs").write_text(
        f"Notices\n\n{BANNER}\nPorted code\n{BANNER}\n\n{RULE}\nlibx 1.0\nLicence: {section_licence}.\n\n"
        f"crates/demo/src/x.rs translates lib/x.c; crates/demo/src/table.rs is generated.\n\n{LICENCE}\n"
        f"{RULE}\nData provenance\ncrates/demo/src/none.rs is not a port.\n\n{BANNER}\nCrates\n{BANNER}\n"
    )


class PortTests(unittest.TestCase):
    def problems(self, crate_licence="(MIT OR Apache-2.0) AND BSD-2-Clause", **kw):
        with scratch.folder("unityrs-ports-") as tmp:
            write_tree(Path(tmp), crate_licence, **kw)
            return T.port_problems(Path(tmp))

    def test_the_repository_agrees(self):
        self.assertEqual(T.port_problems(ROOT), [])
        # Not vacuous: the repository's ports beyond UnityPy are seen, each with its section.
        sources = {s for p in (ROOT / "crates").rglob("*.rs") for s, _ in T.port_headers(p.read_text())}
        self.assertTrue({"liblz4 1.9.4", "CPython 3.11", "UnityPy 1.25.2"} <= sources, sources)
        titles = [t for t, _, _ in T.ported_sections((ROOT / "about.hbs").read_text())]
        self.assertTrue(any(t.startswith("liblz4 1.9.4") for t in titles), titles)
        self.assertTrue(any(t.startswith("CPython 3.11") for t in titles), titles)

    def test_a_consistent_port_passes(self):
        self.assertEqual(self.problems(), [])

    def test_each_missing_link_is_named(self):
        cases = {
            "its crate's license does not name": dict(crate_licence="MIT OR Apache-2.0"),
            "but about.hbs gives MIT": dict(section_licence="MIT"),
            "which no section of about.hbs's ported code names": dict(header=HEADER.replace("libx 1.0", "liby 2.0")),
            "lists it, but its header does not name that source": dict(header="// A file of our own.\n"),
            "none of its LICENSE-* files holds a text": dict(licence_file=False),
        }
        for message, kw in cases.items():
            with self.subTest(message):
                out = self.problems(**kw)
                self.assertTrue(any(message in p for p in out), out)

    def test_the_command_line(self):
        script = SCRIPTS / "third_party_licenses_check.py"
        ok = child.run([sys.executable, str(script), "ports"], capture_output=True, text=True)
        self.assertEqual((ok.returncode, ok.stderr), (0, ""))
        with scratch.folder("unityrs-ports-") as tmp:
            write_tree(Path(tmp), "MIT OR Apache-2.0")
            bad = child.run([sys.executable, str(script), "ports", tmp], capture_output=True, text=True)
        self.assertEqual(bad.returncode, 1)
        self.assertIn("crates/demo/src/x.rs: ported under BSD-2-Clause", bad.stderr)


if __name__ == "__main__":
    unittest.main()
