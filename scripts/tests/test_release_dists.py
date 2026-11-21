"""scripts/release_dists.py: the release workflow's distribution set, on synthetic wheels and sdists.

Needs `packaging`, as the script does (scripts/requirements-tests.txt).
"""

import contextlib
import io
import sys
import tarfile
import unittest
import zipfile
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(ROOT / "tools" / "parity"))

import release_dists as R  # noqa: E402
import scratch  # noqa: E402

PLATFORMS = {
    "linux-x86_64": "manylinux_2_28_x86_64",
    "linux-aarch64": "manylinux_2_28_aarch64",
    "macos-arm64": "macosx_11_0_arm64",
    "macos-x86_64": "macosx_10_12_x86_64",
}


def wheel(folder: Path, platform: str, version="1.0.0", meta=None, abi="cp310-abi3", tags=None) -> Path:
    """A wheel named for `platform`, whose WHEEL file lists `tags` (by default the one its name gives)."""
    path = folder / f"unityrs_py-{version}-{abi}-{platform}.whl"
    path.parent.mkdir(parents=True, exist_ok=True)
    tags = [f"{abi}-{platform}"] if tags is None else tags
    record = "Wheel-Version: 1.0\nGenerator: synthetic\nRoot-Is-Purelib: false\n" + "".join(f"Tag: {t}\n" for t in tags)
    with zipfile.ZipFile(path, "w") as z:
        z.writestr(f"unityrs_py-{version}.dist-info/METADATA", f"Metadata-Version: 2.4\nName: unityrs-py\nVersion: {meta or version}\n")
        z.writestr(f"unityrs_py-{version}.dist-info/WHEEL", record)
    return path


def sdist(folder: Path, version="1.0.0", meta=None) -> Path:
    path = folder / f"unityrs_py-{version}.tar.gz"
    path.parent.mkdir(parents=True, exist_ok=True)
    data = f"Metadata-Version: 2.4\nName: unityrs-py\nVersion: {meta or version}\n".encode()
    with tarfile.open(path, "w:gz") as t:
        info = tarfile.TarInfo(f"unityrs_py-{version}/PKG-INFO")
        info.size = len(data)
        t.addfile(info, io.BytesIO(data))
    return path


def release(root: Path) -> list[Path]:
    files = [wheel(root / f"wheel-{name}", tag) for name, tag in PLATFORMS.items()]
    return files + [sdist(root / "sdist")]


class ReleaseDistsTest(unittest.TestCase):
    def test_the_full_set_passes_and_is_gathered(self):
        with scratch.folder("unityrs-release-") as tmp:
            release(tmp / "artifacts")
            args = ["--version", "1.0.0", str(tmp / "artifacts"), str(tmp / "dist")]
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                self.assertEqual(R.main(args), 0)
                self.assertEqual(len(list((tmp / "dist").iterdir())), 5)
                self.assertEqual(R.main(args), 2)
            self.assertIn("5 distributions of unityrs_py 1.0.0", out.getvalue())
            self.assertIn("DIST must not exist", err.getvalue())

    def test_each_wrong_set_is_named(self):
        with scratch.folder("unityrs-release-") as tmp:
            cases = {
                "a missing target": (lambda r: r[0].unlink(), "linux x86_64: 0 wheels"),
                "a missing sdist": (lambda r: r[4].unlink(), "sdist: 0"),
                "a duplicate": (lambda r: wheel(tmp / "a" / "extra", PLATFORMS["macos-arm64"]), "in more than one"),
                "a second wheel of a target": (
                    lambda r: wheel(tmp / "a" / "extra", "macosx_14_0_arm64"),
                    "macOS arm64: 2 wheels",
                ),
                "another version": (lambda r: (r[1].unlink(), wheel(r[1].parent, PLATFORMS["linux-aarch64"], "1.0.1")),
                                    "version 1.0.1, not 1.0.0"),
                "metadata of another version": (lambda r: (r[4].unlink(), sdist(r[4].parent, meta="0.0.1")),
                                                "its metadata gives version 0.0.1"),
                "another ABI": (lambda r: (r[2].unlink(), wheel(r[2].parent, PLATFORMS["macos-arm64"], abi="cp311-cp311")),
                                "not cp310-abi3"),
                "an unknown platform": (lambda r: wheel(tmp / "a" / "extra", "win_amd64"), "are not exactly one target's"),
                "a compressed tag set": (
                    lambda r: (r[2].unlink(), wheel(r[2].parent, "macosx_11_0_arm64.win_amd64")),
                    "macosx_11_0_arm64.win_amd64 are not exactly one target's",
                ),
                "WHEEL tags other than the name's": (
                    lambda r: (r[0].unlink(), wheel(r[0].parent, PLATFORMS["linux-x86_64"],
                                                    tags=["cp310-abi3-manylinux_2_28_aarch64"])),
                    "its WHEEL file lists tags ['cp310-abi3-manylinux_2_28_aarch64']",
                ),
                "an extra WHEEL tag": (
                    lambda r: (r[0].unlink(), wheel(r[0].parent, PLATFORMS["linux-x86_64"],
                                                    tags=["cp310-abi3-manylinux_2_28_x86_64", "cp310-abi3-win_amd64"])),
                    "its WHEEL file lists tags",
                ),
                "a stray file": (lambda r: (tmp / "a" / "sdist" / "notes.txt").write_text("x"), "neither a unityrs_py"),
            }
            for label, (change, message) in cases.items():
                with self.subTest(label):
                    for path in sorted((tmp / "a").rglob("*"), reverse=True):
                        path.unlink() if path.is_file() else path.rmdir()
                    change(release(tmp / "a"))
                    files = sorted(p for p in (tmp / "a").rglob("*") if p.is_file())
                    found = R.problems(files, "1.0.0")
                    self.assertTrue(any(message in p for p in found), (message, found))

    def test_versions_compare_after_pep_440_normalisation(self):
        with scratch.folder("unityrs-release-") as tmp:
            files = [wheel(tmp / f"wheel-{name}", tag, "1.0.1rc1") for name, tag in PLATFORMS.items()]
            files.append(sdist(tmp / "sdist", "1.0.1rc1"))
            self.assertEqual(R.problems(files, "v1.0.1-rc.1"), [])
            self.assertEqual(R.problems(files, "1.0.1-rc.1"), [])
            self.assertTrue(any("not 1.0.1" in p for p in R.problems(files, "1.0.1")))
            self.assertTrue(any("not not-a-version" in p for p in R.problems(files, "not-a-version")))


if __name__ == "__main__":
    unittest.main()
