"""The encoder verifier rejects archive, manifest and vendored-source drift."""
import hashlib
import io
import sys
import tarfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(SCRIPTS.parent / "tools" / "parity"))
import encoder_vendor as E
import scratch


class VerifyTests(unittest.TestCase):
    def setUp(self):
        self.root = scratch.for_case(self, "unityrs-encoder-vendor-")
        self.vendor = self.root / E.VENDOR
        (self.vendor / "codec").mkdir(parents=True)
        self.source = b"/* pristine source */\n"
        (self.vendor / "codec/core.cpp").write_bytes(self.source)
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode="w:gz") as tar:
            member = tarfile.TarInfo("release/src/core.cpp")
            member.size = len(self.source)
            tar.addfile(member, io.BytesIO(self.source))
        self.archive = out.getvalue()
        self.entries = {"release.tar.gz": hashlib.sha256(self.archive).hexdigest(),
                        "release/src/core.cpp": hashlib.sha256(self.source).hexdigest()}
        self.write_manifest()

    def write_manifest(self):
        (self.vendor / "codec.sha256").write_text("".join(
            f"{sha}  {name}\n" for name, sha in self.entries.items()), encoding="utf-8")

    def problems(self):
        return E.problems(self.archive, "release.tar.gz", "release/src/", "codec", self.root)

    def test_pristine_bytes_pass(self):
        self.assertEqual(self.problems(), [])

    def test_changed_vendored_bytes_fail(self):
        (self.vendor / "codec/core.cpp").write_bytes(b"changed")
        self.assertIn("vendored bytes differ", "\n".join(self.problems()))

    def test_extra_file_fails(self):
        (self.vendor / "codec/extra.h").write_bytes(b"extra")
        self.assertIn("vendored file is not in the manifest", "\n".join(self.problems()))

    def test_missing_file_fails(self):
        (self.vendor / "codec/core.cpp").unlink()
        self.assertIn("vendored bytes differ", "\n".join(self.problems()))

    def test_archive_and_member_hashes_are_checked(self):
        self.entries = {name: "0" * 64 for name in self.entries}
        self.write_manifest()
        found = "\n".join(self.problems())
        self.assertIn("archive SHA-256 differs", found)
        self.assertIn("archive member SHA-256 differs", found)

    def test_missing_member_and_wrong_prefix_fail(self):
        self.entries["release/src/absent.h"] = "0" * 64
        self.entries["elsewhere/core.cpp"] = "0" * 64
        self.write_manifest()
        found = "\n".join(self.problems())
        self.assertIn("exactly one regular member", found)
        self.assertIn("outside release/src/", found)

    def test_symlink_cannot_replace_a_source_file(self):
        path = self.vendor / "codec/core.cpp"
        path.rename(self.root / "original")
        path.symlink_to(self.root / "original")
        self.assertIn("vendored bytes differ", "\n".join(self.problems()))

    def test_manifest_refuses_duplicate_and_unsafe_paths(self):
        for name in ["/absolute", "../outside"]:
            with self.assertRaises(ValueError):
                E.manifest("0" * 64 + "  " + name)
        with self.assertRaises(ValueError):
            E.manifest(("0" * 64 + "  same\n") * 2)

    def test_separately_sourced_notices_are_hash_checked(self):
        (self.vendor / "codec.notices.sha256").write_text(
            "0" * 64 + "  LICENSE-CODEC\n", encoding="utf-8")
        self.assertIn("upstream notice SHA-256 differs", "\n".join(self.problems()))


if __name__ == "__main__":
    unittest.main()
