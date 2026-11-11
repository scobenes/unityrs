"""scripts/zlib_vendor.py on tarballs built here from the repository's vendored zlib, and on edits of them.

The real tarball is not in the repository; `verify` on it is run by hand (the script's docstring). Standard library
only.
"""

import contextlib
import hashlib
import io
import shutil
import sys
import tarfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(ROOT / "tools" / "parity"))

import scratch  # noqa: E402
import zlib_vendor as Z  # noqa: E402

VENDOR = ROOT / Z.VENDOR


def tarball(members: dict[str, bytes]) -> bytes:
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:gz") as tar:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return out.getvalue()


def vendored() -> dict[str, bytes]:
    """The vendored files, by their names in the tarball."""
    _, members = Z.manifest((VENDOR / Z.MANIFEST).read_text(encoding="utf-8"))
    return {name: (VENDOR / name).read_bytes() for name in members}


class VerifyTests(unittest.TestCase):
    def test_the_manifest_lists_the_tarball_and_the_vendored_files(self):
        sha, members = Z.manifest((VENDOR / Z.MANIFEST).read_text(encoding="utf-8"))
        self.assertEqual(sha, "b3a24de97a8fdbc835b9833169501030b8977031bcb54b3b3ac13740f846ab30")
        self.assertEqual(
            sorted(members), sorted(Z.PREFIX + p.name for p in (VENDOR / "zlib-1.2.13").iterdir() if p.is_file())
        )

    def test_a_tarball_of_the_vendored_files_differs_only_in_its_own_hash(self):
        found = Z.problems(tarball(vendored()))
        self.assertEqual(len(found), 1, found)
        self.assertIn("the tarball's sha256 is", found[0])

    def test_a_changed_or_missing_member_is_named(self):
        members = vendored()
        members["zlib-1.2.13/deflate.c"] += b"\n"
        del members["zlib-1.2.13/zlib.h"]
        found = Z.problems(tarball(members))[1:]
        got = hashlib.sha256(members["zlib-1.2.13/deflate.c"]).hexdigest()
        listed = Z.manifest((VENDOR / Z.MANIFEST).read_text(encoding="utf-8"))[1]["zlib-1.2.13/deflate.c"]
        self.assertEqual(
            found,
            [
                f"the tarball's zlib-1.2.13/deflate.c has sha256 {got}, not {listed}",
                f"{Z.VENDOR / 'zlib-1.2.13/deflate.c'} is not the tarball's zlib-1.2.13/deflate.c",
                "the tarball has no zlib-1.2.13/zlib.h",
            ],
        )

    def test_a_changed_vendored_file_is_named(self):
        with scratch.folder("unityrs-zlib-vendor-") as tmp:
            shutil.copytree(VENDOR, tmp / Z.VENDOR, symlinks=True)
            (tmp / Z.VENDOR / "zlib-1.2.13/trees.c").write_bytes(b"/* changed */\n")
            found = Z.problems(tarball(vendored()), root=tmp)[1:]
        self.assertEqual(found, [f"{Z.VENDOR / 'zlib-1.2.13/trees.c'} is not the tarball's zlib-1.2.13/trees.c"])

    def test_usage(self):
        with contextlib.redirect_stderr(io.StringIO()) as err:
            self.assertEqual(Z.main([]), 2)
            self.assertEqual(Z.main(["check", "x"]), 2)
        self.assertIn("verify TARBALL", err.getvalue())


if __name__ == "__main__":
    unittest.main()
