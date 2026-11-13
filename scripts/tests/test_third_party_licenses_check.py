"""scripts/third_party_licenses_check.py on the repository's own about.toml and deny.toml, and on edits of them.

Standard library only (tomllib, Python 3.11 or later).
"""

import copy
import re
import shutil
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

    def test_target_filters_cover_the_same_platforms(self):
        targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"]
        about, deny = copy.deepcopy(ABOUT), copy.deepcopy(DENY)
        about["targets"] = targets
        deny["graph"] = {"targets": [{"triple": t} for t in reversed(targets)]}
        self.assertEqual(self.problems(about, deny), [])
        for change in (
            lambda a, d: a.pop("targets"),
            lambda a, d: d.pop("graph"),
            lambda a, d: d["graph"]["targets"].pop(),
            lambda a, d: d["graph"]["targets"][0].update(triple="x86_64-pc-windows-msvc"),
        ):
            changed_about, changed_deny = copy.deepcopy(about), copy.deepcopy(deny)
            change(changed_about, changed_deny)
            with self.subTest(about=changed_about.get("targets"), graph=changed_deny.get("graph")):
                self.assertIn("keep target filters equal", "\n".join(self.problems(changed_about, changed_deny)))

    def test_whole_graph_bans_have_no_graph_filters_and_mirror_the_policy(self):
        bans = {"bans": copy.deepcopy(DENY["bans"])}
        self.assertEqual(T.bans_problems(DENY, bans), [])
        self.assertEqual(T.bans_problems(DENY, load("deny-bans.toml")), [])
        for graph in (
            {"targets": [{"triple": "x86_64-unknown-linux-gnu"}]},
            {"exclude": ["openssl"]},
            {"no-default-features": True},
        ):
            with self.subTest(graph=graph):
                out = T.bans_problems(DENY, {**bans, "graph": graph})
                self.assertIn("must not filter the dependency graph", "\n".join(out))
        for change in (
            lambda b: b["bans"]["deny"].pop(),
            lambda b: b["bans"].update(**{"multiple-versions-include-dev": False}),
            lambda b: b["bans"]["deny"][-1].update(**{"deny-multiple-versions": False}),
        ):
            changed = copy.deepcopy(bans)
            change(changed)
            with self.subTest(bans=changed):
                self.assertIn("keep bans equal", "\n".join(T.bans_problems(DENY, changed)))

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
        about["ring"]["accepted"] = ["LGPL-2.1-or-later"]
        self.assertIn("ring: about.toml accepts ['LGPL-2.1-or-later']", "\n".join(self.problems(about=about)))

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
            "about lacks it": lambda a, d: a.pop("ring"),
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

    def test_the_fuzz_workspace_copies_the_shared_entries(self):
        fuzz = load("fuzz/deny.toml")
        shared = T.lock_names(ROOT / "Cargo.lock") & T.lock_names(ROOT / "fuzz" / "Cargo.lock")
        self.assertTrue({"unityrs-texture", "liblzma-sys", "r-efi", "unityrs-core"} <= shared)
        self.assertNotIn("libfuzzer-sys", shared)
        self.assertEqual(T.fuzz_problems(DENY, fuzz, shared), [])

        def texture(kind):
            return lambda d: d["licenses"][kind].remove(
                next(e for e in d["licenses"][kind] if e["crate"] == "unityrs-texture")
            )

        def rehash(d):
            entry = next(e for e in d["licenses"]["clarify"] if e["crate"] == "unityrs-texture")
            entry["license-files"][-1]["hash"] ^= 1

        changes = {
            "a shared clarification missing (the review's case)": texture("clarify"),
            "a shared exception missing": texture("exceptions"),
            "a hash of its own": rehash,
            "another allow order": lambda d: d["licenses"]["allow"].reverse(),
            "an exception the root lacks": lambda d: d["licenses"]["exceptions"].append(
                {"crate": "libc", "allow": ["NCSA"]}
            ),
        }
        for label, change in changes.items():
            changed = copy.deepcopy(fuzz)
            change(changed)
            with self.subTest(label):
                self.assertTrue(T.fuzz_problems(DENY, changed, shared))
        # The fuzz crate's own (libfuzzer-sys, which the root never builds) is its own business.
        changed = copy.deepcopy(fuzz)
        changed["licenses"]["exceptions"].append({"crate": "libfuzzer-sys", "allow": ["Zlib"]})
        self.assertEqual(T.fuzz_problems(DENY, changed, shared), [])

    def test_a_member_s_clarification_is_its_own_license(self):
        self.assertEqual(T.member_problems(ABOUT, ROOT), [])
        about = copy.deepcopy(ABOUT)
        about["unityrs-texture"]["clarify"]["license"] = "(MIT OR Apache-2.0) AND MIT AND MIT-CMU AND BSD-3-Clause"
        out = "\n".join(T.member_problems(about, ROOT))
        self.assertIn("unityrs-texture: about.toml clarifies it as", out)
        # A dependency's clarification is not held to a member's manifest.
        about = copy.deepcopy(ABOUT)
        about["ring"]["clarify"]["license"] = "MIT"
        self.assertEqual(T.member_problems(about, ROOT), [])

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

    def test_each_member_licence_file_is_listed_under_crates(self):
        self.assertEqual(T.notice_problems((ROOT / "THIRD_PARTY_LICENSES").read_text(encoding="utf-8"), ROOT), [])
        crates_part = f"{BANNER}\nCrates\n{BANNER}\n"
        with scratch.folder("unityrs-notices-") as tmp:
            write_tree(Path(tmp), "(MIT OR Apache-2.0) AND BSD-2-Clause")
            listed = f"Ported code\n\n{crates_part}\n{OVERVIEW}BSD 2-Clause License\n\n{LICENCE}"
            self.assertEqual(T.notice_problems(listed, Path(tmp)), [])
            # cargo-about did not recognise LICENSE-X and listed the template: the file's text is
            # in the ported-code part only, or nowhere.
            for generated in (
                f"Ported code\n\n{LICENCE}\n{crates_part}\n{OVERVIEW}Copyright (c) 2019 Template\n",
                f"{crates_part}\n{OVERVIEW}BSD 2-Clause License\n",
                f"Ported code\n\n{LICENCE}\n",
            ):
                with self.subTest(generated):
                    out = T.notice_problems(generated, Path(tmp))
                    self.assertEqual(len(out), 1, out)
                    self.assertTrue(
                        out[0].startswith(("crates/demo/LICENSE-X: its text is not", "no Crates part")), out
                    )

    def test_a_generated_tables_licence_is_listed_under_crates(self):
        # Review round 12: a table generated from data under a licence of its own (the Unicode
        # Character Database's) must find that licence's text in the generated file.
        crates_part = f"{BANNER}\nCrates\n{BANNER}\n"
        with scratch.folder("unityrs-notices-") as tmp:
            write_tree(Path(tmp), "(MIT OR Apache-2.0) AND BSD-2-Clause", extra={"crates/demo/src/data.rs": DATA})
            unicode = "- Unicode License v3 (Unicode-3.0)\n"
            for overview, want in (
                (OVERVIEW, ["crates/demo/src/data.rs: generated from UCD 14.0.0 under Unicode-3.0, which is not"]),
                (OVERVIEW.replace("\n\n", f"\n{unicode}\n"), []),
                # Only the overview counts: the id in the text below it does not.
                (f"{OVERVIEW}Used by:\n{unicode}", ["crates/demo/src/data.rs: generated from UCD 14.0.0"]),
            ):
                with self.subTest(overview):
                    text = f"Ported code\n\n{crates_part}\n{overview}BSD 2-Clause License\n\n{LICENCE}"
                    out = T.notice_problems(text, Path(tmp))
                    self.assertEqual(len(out), len(want), out)
                    for problem, start in zip(out, want, strict=True):
                        self.assertTrue(problem.startswith(start), out)


RULE = "-" * 80
BANNER = "=" * 80
LICENCE = "Copyright (c) 2020 Example Author\n\nPermission is granted, on condition the notice is kept.\n"
HEADER = "// Ported from libx 1.0 (BSD-2-Clause): lib/x.c. See THIRD_PARTY_LICENSES.\n"
OVERVIEW = "Licences:\n- BSD 2-Clause License (BSD-2-Clause)\n\n"
# A table generated from the ported source, and one generated from data with a licence of its own.
TABLE = (
    "// Generated by tools/gen.py from libx 1.0 (BSD-2-Clause): lib/x.h (TABLE). See THIRD_PARTY_LICENSES.\n"
    "//! The table.\n// @generated by tools/gen.py from libx 1.0; do not edit.\n// @generated end\n"
)
DATA = (
    "// Generated by tools/gen.py from UCD 14.0.0 (Unicode-3.0): its digits. See THIRD_PARTY_LICENSES.\n"
    "//! The data.\n// @generated by tools/gen.py from UCD 14.0.0; do not edit.\n// @generated end\n"
)
PROVENANCE = "crates/demo/src/data.rs: the digits of UCD 14.0.0 (Unicode-3.0),\ngenerated by tools/gen.py.\n"


def write_tree(
    root: Path,
    crate_licence: str,
    header: str = HEADER,
    section_licence: str = "BSD-2-Clause",
    licence_file: str | None = LICENCE,
    title: str = "libx 1.0 (the x library)",
    marker: str = "{{!-- notice: LICENSE-X --}}\n",
    manifest: str = "",
    table: str = TABLE,
    provenance: str = PROVENANCE,
    extra: dict[str, str] | None = None,
):
    """A workspace of one crate whose src/x.rs ports `libx 1.0`, and its about.hbs.

    The section's notice is LICENCE, after `marker`; the crate's LICENSE-X
    holds `licence_file` (none when it is None). src/table.rs holds `table`,
    which tools/gen.py generates; `extra` adds files, and `provenance` is the
    data provenance part's paragraph.
    """
    (root / "crates" / "demo" / "src").mkdir(parents=True)
    (root / "Cargo.toml").write_text('[workspace.package]\nlicense = "MIT OR Apache-2.0"\n')
    licence_line = f'license = "{crate_licence}"' if crate_licence else "license.workspace = true"
    (root / "crates" / "demo" / "Cargo.toml").write_text(f'[package]\nname = "demo"\n{licence_line}\n{manifest}')
    (root / "crates" / "demo" / "src" / "x.rs").write_text(header + "//! The port.\npub fn x() {}\n")
    (root / "crates" / "demo" / "src" / "table.rs").write_text(table)
    (root / "tools").mkdir()
    (root / "tools" / "gen.py").write_text("")
    for rel, text in (extra or {}).items():
        (root / rel).write_text(text)
    if licence_file is not None:
        (root / "crates" / "demo" / "LICENSE-X").write_text(licence_file)
    (root / "about.hbs").write_text(
        f"Notices\n\n{BANNER}\nPorted code\n{BANNER}\n\n{RULE}\n{title}\nLicence: {section_licence}.\n\n"
        f"crates/demo/src/x.rs translates lib/x.c; crates/demo/src/table.rs is generated.\n\n{marker}{LICENCE}\n"
        f"{RULE}\nData provenance\ncrates/demo/src/none.rs is not a port.\n\n{provenance}\n{BANNER}\nCrates\n{BANNER}\n"
    )


def copy_repository(dest: Path):
    """The parts of the repository `ports` reads: about.hbs, the manifests, each crate's notice files and sources,
    and the generator of its tables."""
    shutil.copy(ROOT / "about.hbs", dest / "about.hbs")
    (dest / "tools" / "gen").mkdir(parents=True)
    shutil.copy(ROOT / "tools" / "gen" / "gen_tables.py", dest / "tools" / "gen" / "gen_tables.py")
    shutil.copy(ROOT / "Cargo.toml", dest / "Cargo.toml")
    for crate in (ROOT / "crates").iterdir():
        if not (crate / "Cargo.toml").is_file():
            continue
        to = dest / "crates" / crate.name
        shutil.copytree(crate / "src", to / "src", ignore=shutil.ignore_patterns("*.json", "*.bin"))
        for f in [crate / "Cargo.toml", *crate.glob("LICENSE-*"), *crate.glob("NOTICE-*")]:
            shutil.copy(f, to / f.name)


class SpdxTests(unittest.TestCase):
    def test_a_licence_must_be_in_every_alternative(self):
        cases = {
            "MIT": {"MIT": True},
            "MIT OR Apache-2.0": {"MIT": False, "Apache-2.0": False},
            "(MIT OR Apache-2.0) AND MIT": {"MIT": True, "Apache-2.0": False},
            "MIT OR BSD-2-Clause": {"MIT": False, "BSD-2-Clause": False},
            "(MIT OR Apache-2.0 OR BSD-2-Clause)": {"BSD-2-Clause": False},
            "(MIT OR Apache-2.0) AND MIT AND BSD-2-Clause AND PSF-2.0": {"MIT": True, "BSD-2-Clause": True},
            # AND binds tighter than OR: MIT, or BSD-2-Clause with PSF-2.0.
            "MIT OR BSD-2-Clause AND PSF-2.0": {"MIT": False, "PSF-2.0": False},
            "(MIT AND PSF-2.0) OR (Apache-2.0 AND PSF-2.0)": {"PSF-2.0": True, "MIT": False},
            "((PSF-2.0))": {"PSF-2.0": True},
            # A licence with an exception, or a later version, is another licence.
            "Apache-2.0 WITH LLVM-exception": {"Apache-2.0": False, "Apache-2.0 WITH LLVM-exception": True},
            "GPL-2.0+ AND MIT": {"GPL-2.0": False, "MIT": True},
        }
        for expression, wants in cases.items():
            tree = T.spdx_parse(expression)
            for licence, want in wants.items():
                with self.subTest(expression=expression, licence=licence):
                    self.assertIs(T.spdx_requires(tree, licence), want)

    def test_the_licences_named(self):
        tree = T.spdx_parse("(MIT OR Apache-2.0) AND PSF-2.0 AND (Apache-2.0 WITH LLVM-exception)")
        self.assertEqual(T.spdx_ids(tree), {"MIT", "Apache-2.0", "PSF-2.0", "Apache-2.0 WITH LLVM-exception"})

    def test_what_is_not_an_expression_is_refused(self):
        for expression in (
            "",
            "MIT/Apache-2.0",
            "MIT or Apache-2.0",
            "(MIT OR Apache-2.0",
            "MIT OR Apache-2.0)",
            "MIT AND",
            "AND MIT",
            "MIT WITH",
            "MIT WITH (X)",
            "MIT Apache-2.0",
        ):
            with self.subTest(expression), self.assertRaises(ValueError):
                T.spdx_parse(expression)


class PortTests(unittest.TestCase):
    def problems(self, crate_licence="(MIT OR Apache-2.0) AND BSD-2-Clause", **kw):
        with scratch.folder("unityrs-ports-") as tmp:
            write_tree(Path(tmp), crate_licence, **kw)
            return T.port_problems(Path(tmp))

    def assert_named(self, message, out):
        self.assertTrue(any(message in p for p in out), out)

    def test_the_repository_agrees(self):
        self.assertEqual(T.port_problems(ROOT), [])
        # Not vacuous: the repository's ports are seen, each with its section and its notice.
        sources = {s for p in (ROOT / "crates").rglob("*.rs") for s, _ in T.port_headers(p.read_text())}
        self.assertTrue({"liblz4 1.9.4", "CPython 3.11", "UnityPy 1.25.2"} <= sources, sources)
        generated = {
            p.relative_to(ROOT).as_posix(): [(h.generator, h.source, h.spdx) for h in T.source_headers(p.read_text())]
            for p in (ROOT / "crates").rglob("*.rs")
            if "// @generated by " in p.read_text()
        }
        unitypy, unicode = ("UnityPy 1.25.2", "MIT"), ("Unicode Character Database 14.0.0", "Unicode-3.0")
        gen = "tools/gen/gen_tables.py"
        self.assertEqual(
            generated,
            {
                "crates/unityrs-core/src/classid.rs": [(gen, *unitypy)],
                "crates/unityrs-core/src/typetree/commonstring.rs": [(None, *unitypy), (gen, *unitypy)],
                "crates/unityrs-core/src/tables.rs": [(gen, *unicode)],
                "crates/unityrs-dump/src/select/printable.rs": [(gen, *unicode)],
            },
        )
        self.assertEqual(set(T.data_provenance((ROOT / "about.hbs").read_text())), set(generated))
        notices = {
            s.source: [name for name, _ in s.notices] for s in T.ported_sections((ROOT / "about.hbs").read_text())
        }
        self.assertEqual(
            notices,
            {
                "UnityPy 1.25.2": ["LICENSE-UNITYPY"],
                "liblz4 1.9.4": ["LICENSE-LZ4"],
                "CPython 3.11": ["NOTICE-PSF", "LICENSE-PSF"],
                "Pillow 9.5.0": ["LICENSE-PILLOW"],
                "texture2ddecoder 1.0.5": ["LICENSE-TEXTURE2DDECODER", "LICENSE-ASSETSTUDIO", "LICENSE-MIKUNYAN"],
                "OpenBLAS 0.3.31": ["LICENSE-OPENBLAS", "LICENSE-OPENBLAS-KERNELS", "LICENSE-OPENBLAS-DSCAL"],
                "GotoBLAS2 code in OpenBLAS 0.3.31": ["LICENSE-OPENBLAS-UT"],
                "zlib 1.2.13": ["LICENSE-ZLIB"],
            },
        )

    def test_a_consistent_port_passes(self):
        self.assertEqual(self.problems(), [])
        # A title without an aside, and a crate under the port's licence alone.
        self.assertEqual(self.problems(title="libx 1.0"), [])
        self.assertEqual(self.problems(crate_licence="BSD-2-Clause"), [])

    def test_each_missing_link_is_named(self):
        cases = {
            "does not name in every alternative": dict(crate_licence="MIT OR Apache-2.0"),
            "but about.hbs gives MIT": dict(section_licence="MIT"),
            "which no section of about.hbs's ported code names": dict(header=HEADER.replace("libx 1.0", "liby 2.0")),
            "lists it, but its header does not name that source": dict(header="// A file of our own.\n"),
            "has no LICENSE-X with about.hbs's": dict(licence_file=None),
            "has no notice": dict(marker=""),
            "is not a title of the form": dict(title="libx (1.0) (the x library)"),
        }
        for message, kw in cases.items():
            with self.subTest(message):
                self.assert_named(message, self.problems(**kw))

    def test_a_generated_table_says_what_generated_it_and_from_what(self):
        # Review round 12: the table files' headers named the files they cite, and one claimed a port
        # its data is not.
        self.assertEqual(self.problems(extra={"crates/demo/src/data.rs": DATA}), [])
        region = TABLE.split("//! The table.\n")[1]
        cases = {
            "no Generated header": (
                dict(table="//! The table.\n" + region),
                "table.rs: tools/gen.py generates tables in it, but no `// Generated by tools/gen.py from",
            ),
            "a Ported header instead": (
                dict(table=HEADER + region),
                "table.rs: tools/gen.py generates tables in it, but no `// Generated by tools/gen.py from",
            ),
            "a file about.hbs lists, with neither header": (
                dict(table="//! The table.\n" + region),
                "table.rs: about.hbs's 'libx 1.0 (the x library)' lists it, but its header does not name that source",
            ),
            "a generator that does not exist": (
                dict(table=TABLE.replace("Generated by tools/gen.py", "Generated by tools/nope.py")),
                "table.rs: its header says tools/nope.py generates it, but no region of it is that file's",
            ),
            "no region": (
                dict(table=TABLE.split("// @generated")[0]),
                "table.rs: its header says tools/gen.py generates it, but no region of it is that file's",
            ),
            "a section's source under another licence": (
                dict(table=TABLE.replace("(BSD-2-Clause)", "(MIT)", 1)),
                "table.rs: ported from libx 1.0 under MIT, but about.hbs gives BSD-2-Clause",
            ),
            "data no paragraph names": (
                dict(extra={"crates/demo/src/data.rs": DATA}, provenance=""),
                "data.rs: generated from UCD 14.0.0 (Unicode-3.0), which neither a section of about.hbs's",
            ),
            "data under another licence than its paragraph's": (
                dict(extra={"crates/demo/src/data.rs": DATA}, provenance=PROVENANCE.replace("3.0", "DFS-2016")),
                "data.rs: about.hbs's data provenance names it, but no `Generated by` header of it agrees",
            ),
            "a paragraph about a file with no Generated header": (
                dict(extra={"crates/demo/src/data.rs": DATA.split("\n", 1)[1]}),
                "data.rs: about.hbs's data provenance names it, but no `Generated by` header of it agrees",
            ),
        }
        for label, (kw, message) in cases.items():
            with self.subTest(label):
                self.assert_named(message, self.problems(**kw))

    def test_a_licence_offered_only_as_a_choice_does_not_hold_a_port(self):
        # Review round 11: a set of words read "MIT OR BSD-2-Clause" as naming BSD-2-Clause.
        for licence in (
            "MIT OR BSD-2-Clause",
            "(MIT OR Apache-2.0 OR BSD-2-Clause)",
            "MIT OR BSD-2-Clause AND Apache-2.0",
            "BSD-2-Clause WITH LLVM-exception",
            None,  # the workspace's own, "MIT OR Apache-2.0"
        ):
            with self.subTest(licence):
                self.assert_named("does not name in every alternative", self.problems(crate_licence=licence))
        self.assert_named("does not parse", self.problems(crate_licence="MIT/BSD-2-Clause"))

    def test_a_header_names_its_source_exactly(self):
        # Review round 11: any prefix of a title passed, a version's prefix included.
        for source in ("libx 1", "libx 1.", "libx", "l", "libx 1.0 (the x library)", "libx 1.0.1"):
            with self.subTest(source):
                out = self.problems(header=HEADER.replace("libx 1.0", source))
                self.assert_named("which no section of about.hbs's ported code names", out)
                self.assert_named("lists it, but its header does not name that source", out)

    def test_a_crate_carries_the_notice_of_what_it_holds(self):
        # Review round 11: an MIT port's crate shipped no copy of its notice; an MIT notice is held too.
        mit = dict(
            header=HEADER.replace("(BSD-2-Clause)", "(MIT)"),
            table=TABLE.replace("(BSD-2-Clause)", "(MIT)"),
            section_licence="MIT",
            crate_licence="(MIT OR Apache-2.0) AND MIT",
        )
        self.assertEqual(self.problems(**mit), [])
        self.assert_named("has no LICENSE-X with about.hbs's", self.problems(**mit, licence_file=None))
        other = "Copyright (c) 2026 Someone Else\n\nPermission is granted, on condition the notice is kept.\n"
        for kw in (mit, {}):
            with self.subTest(kw):
                self.assert_named("has a LICENSE-X that is not about.hbs's", self.problems(**kw, licence_file=other))
                self.assert_named(
                    "has a LICENSE-X that is not about.hbs's",
                    self.problems(**kw, licence_file=LICENCE.split("\n\n")[1]),
                )
        self.assert_named("sets include or exclude", self.problems(manifest='include = ["src/**", "Cargo.toml"]\n'))
        self.assert_named("sets include or exclude", self.problems(manifest='exclude = ["LICENSE-*"]\n'))

    def test_every_notice_of_a_section_is_carried(self):
        # A summary of changes the licence asks for, beside the licence: both are the crate's.
        changes = "Changes made to libx: rewritten in Rust.\n"
        marker = f"{{{{!-- notice: NOTICE-X --}}}}\n{changes}\n{{{{!-- notice: LICENSE-X --}}}}\n"
        with scratch.folder("unityrs-ports-") as tmp:
            write_tree(Path(tmp), "(MIT OR Apache-2.0) AND BSD-2-Clause", marker=marker)
            sections = T.ported_sections((Path(tmp) / "about.hbs").read_text())
            self.assertEqual([(n, t.strip()) for n, t in sections[0].notices][0], ("NOTICE-X", changes.strip()))
            self.assert_named("has no NOTICE-X with about.hbs's", T.port_problems(Path(tmp)))
            (Path(tmp) / "crates" / "demo" / "NOTICE-X").write_text("Changes made to libx: none.\n")
            self.assert_named("has a NOTICE-X that is not about.hbs's", T.port_problems(Path(tmp)))
            (Path(tmp) / "crates" / "demo" / "NOTICE-X").write_text(changes)
            self.assertEqual(T.port_problems(Path(tmp)), [])
            (Path(tmp) / "crates" / "demo" / "LICENSE-X").unlink()
            self.assert_named("has no LICENSE-X with about.hbs's", T.port_problems(Path(tmp)))

    def test_the_repository_without_a_notice_or_a_licence_term(self):
        # The review's cases on a copy of this repository.
        def tpk(root):
            return root / "crates" / "unityrs-tpk"

        cases = {
            "unityrs-tpk on the workspace licence": (
                lambda root: (tpk(root) / "Cargo.toml").write_text(
                    re.sub(r"(?m)^license = .*$", "license.workspace = true", (tpk(root) / "Cargo.toml").read_text())
                ),
                "crates/unityrs-tpk/src/lib.rs: ported under MIT, which its crate's license ('MIT OR Apache-2.0')",
            ),
            "unityrs-tpk without UnityPy's notice": (
                lambda root: (tpk(root) / "LICENSE-UNITYPY").unlink(),
                "crates/unityrs-tpk: it holds UnityPy 1.25.2's code, but has no LICENSE-UNITYPY",
            ),
            "unityrs-cli without the summary of changes to CPython's code": (
                lambda root: (root / "crates" / "unityrs-cli" / "NOTICE-PSF").unlink(),
                "crates/unityrs-cli: it holds CPython 3.11's code, but has no NOTICE-PSF",
            ),
        }
        for label, (change, message) in cases.items():
            with self.subTest(label), scratch.folder("unityrs-ports-") as tmp:
                copy_repository(Path(tmp))
                self.assertEqual(T.port_problems(Path(tmp)), [])
                change(Path(tmp))
                self.assert_named(message, T.port_problems(Path(tmp)))

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
