"""fetch_unitypy_samples.py on synthetic pins and a fake server: no network, no game data.

The last test checks the real pins against the samples test_dump.py verifies.
Standard library only.
"""

import contextlib
import hashlib
import io
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import fetch_unitypy_samples as F  # noqa: E402
from test_dump import SAMPLES  # noqa: E402

BODY_A = b"UnityFS\0synthetic sample a\0" * 40
BODY_B = b"UnityFS\0synthetic sample b\0" * 7
POINTER = (
    b"version https://git-lfs.github.com/spec/v1\n"
    b"oid sha256:" + hashlib.sha256(BODY_A).hexdigest().encode() + b"\nsize " + str(len(BODY_A)).encode() + b"\n"
)


def pin(name, data):
    return {"name": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


PINS = {
    "commit": "c" * 40,
    "url": "https://samples.invalid/{commit}/{name}",
    "files": [pin("a", BODY_A), pin("b", BODY_B)],
}
URL_A = F.sample_url(PINS, "a")
URL_B = F.sample_url(PINS, "b")


class Response(io.BytesIO):
    """A response body that records how many bytes were read from it."""

    def __init__(self, data, server):
        super().__init__(data)
        self.server = server

    def read(self, n=-1):
        out = super().read(n)
        self.server.bytes_read += len(out)
        return out


class Server:
    """A fake urlopen serving `bodies` by URL; the first `failures` calls raise a network error."""

    def __init__(self, bodies, failures=0):
        self.bodies, self.failures = bodies, failures
        self.calls, self.bytes_read = [], 0

    def __call__(self, url, timeout):
        self.calls.append(url)
        if self.failures:
            self.failures -= 1
            raise ConnectionResetError("connection reset by peer")
        return Response(self.bodies[url], self)


class FetchTests(unittest.TestCase):
    def setUp(self):
        self.dest = Path(tempfile.mkdtemp(prefix="unityrs-samples-"))
        self.addCleanup(shutil.rmtree, self.dest)
        self.sleeps = []

    def fetch(self, server, dest=None):
        """main()'s exit status and what it wrote on stderr."""
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            code = F.main([str(dest or self.dest)], opener=server, sleep=self.sleeps.append, pins=PINS)
        return code, err.getvalue()

    def assert_holds(self, names):
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), sorted(names))

    def test_fetches_every_pinned_sample(self):
        server = Server({URL_A: BODY_A, URL_B: BODY_B})
        self.assertEqual(self.fetch(server), (0, ""))
        self.assertEqual(server.calls, [URL_A, URL_B])
        self.assertEqual(((self.dest / "a").read_bytes(), (self.dest / "b").read_bytes()), (BODY_A, BODY_B))
        self.assert_holds(["a", "b"])

    def test_creates_the_folder(self):
        dest = self.dest / "deeper" / "samples"
        self.assertEqual(self.fetch(Server({URL_A: BODY_A, URL_B: BODY_B}), dest)[0], 0)
        self.assertEqual((dest / "b").read_bytes(), BODY_B)

    def test_keeps_a_sample_already_there(self):
        (self.dest / "a").write_bytes(BODY_A)
        server = Server({URL_B: BODY_B})
        self.assertEqual(self.fetch(server)[0], 0)
        self.assertEqual(server.calls, [URL_B])
        # A second run downloads nothing.
        server = Server({})
        self.assertEqual(self.fetch(server)[0], 0)
        self.assertEqual(server.calls, [])

    def test_replaces_a_file_that_is_not_the_sample(self):
        (self.dest / "a").write_bytes(b"x" * len(BODY_A))  # the right size, the wrong bytes
        (self.dest / "b").write_bytes(BODY_B + b"!")  # the wrong size
        server = Server({URL_A: BODY_A, URL_B: BODY_B})
        self.assertEqual(self.fetch(server)[0], 0)
        self.assertEqual(server.calls, [URL_A, URL_B])
        self.assertEqual(((self.dest / "a").read_bytes(), (self.dest / "b").read_bytes()), (BODY_A, BODY_B))

    def test_refuses_an_lfs_pointer(self):
        code, err = self.fetch(Server({URL_A: POINTER, URL_B: BODY_B}))
        self.assertEqual(code, 1)
        self.assertIn("a: ", err)
        self.assertIn("a Git LFS pointer, not the sample", err)
        self.assert_holds([])

    def test_refuses_other_bytes(self):
        code, err = self.fetch(Server({URL_A: BODY_A[:-1] + b"?", URL_B: BODY_B}))
        self.assertEqual(code, 1)
        self.assertIn(f"not the pinned {PINS['files'][0]['sha256']}", err)
        self.assert_holds([])

    def test_refuses_a_short_body(self):
        code, err = self.fetch(Server({URL_A: BODY_A[:100], URL_B: BODY_B}))
        self.assertEqual(code, 1)
        self.assertIn(f"100 bytes, not the pinned {len(BODY_A)}", err)

    def test_reads_at_most_one_byte_past_the_pinned_size(self):
        server = Server({URL_A: BODY_A + b"\0" * (5 << 20), URL_B: BODY_B})
        code, err = self.fetch(server)
        self.assertEqual(code, 1)
        self.assertIn(f"{len(BODY_A) + 1} bytes, not the pinned {len(BODY_A)}", err)
        self.assertEqual(server.bytes_read, len(BODY_A) + 1)
        self.assert_holds([])

    def test_a_later_failure_keeps_the_samples_already_fetched(self):
        code, _ = self.fetch(Server({URL_A: BODY_A, URL_B: b"nope"}))
        self.assertEqual(code, 1)
        self.assert_holds(["a"])

    def test_retries_network_errors(self):
        server = Server({URL_A: BODY_A, URL_B: BODY_B}, failures=F.ATTEMPTS - 1)
        self.assertEqual(self.fetch(server)[0], 0)
        self.assertEqual(server.calls, [URL_A] * F.ATTEMPTS + [URL_B])
        self.assertEqual(self.sleeps, [2**i for i in range(1, F.ATTEMPTS)])

    def test_gives_up_after_the_last_attempt(self):
        server = Server({URL_A: BODY_A, URL_B: BODY_B}, failures=F.ATTEMPTS)
        code, err = self.fetch(server)
        self.assertEqual(code, 1)
        self.assertEqual(server.calls, [URL_A] * F.ATTEMPTS)
        self.assertIn(f"connection reset by peer (after {F.ATTEMPTS} attempts)", err)
        self.assert_holds([])

    def test_refuses_a_folder_inside_the_repository(self):
        server = Server({URL_A: BODY_A, URL_B: BODY_B})
        inside = HERE / "samples-must-not-land-here"
        with contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit) as cm:
            F.main([str(inside)], opener=server, sleep=self.sleeps.append, pins=PINS)
        self.assertEqual(cm.exception.code, 2)
        self.assertIn("refusing to put the samples inside the repository", err.getvalue())
        self.assertFalse(inside.exists())
        self.assertEqual(server.calls, [])

    def test_the_pins_are_the_samples_the_tests_check(self):
        pins = F.load_pins()
        self.assertEqual(pins, json.loads(F.PINS.read_text(encoding="utf-8")))
        self.assertEqual(pins["commit"], "417d59522a998fdee9a9c2b29fe35f3063222457")
        self.assertEqual({p["name"]: p["sha256"] for p in pins["files"]}, SAMPLES)
        self.assertEqual(
            F.sample_url(pins, "banner_1"),
            "https://media.githubusercontent.com/media/K0lb3/UnityPy/"
            "417d59522a998fdee9a9c2b29fe35f3063222457/tests/samples/banner_1",
        )


if __name__ == "__main__":
    unittest.main()
