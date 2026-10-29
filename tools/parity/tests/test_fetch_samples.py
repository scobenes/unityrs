"""fetch_unitypy_samples.py on synthetic pins and a fake server: no network, no game data.

The redirect test runs two HTTP servers on the loopback interface, and the
deadline tests in TrickleTests one whose body trickles in, so that urllib's
real response object is what is read. The last test checks the real pins
against the samples test_dump.py verifies. Standard library only.
"""

import contextlib
import hashlib
import http.server
import io
import json
import os
import shutil
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

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
    "url": F.URL_PREFIX + "{commit}/tests/samples/{name}",
    "files": [pin("a", BODY_A), pin("b", BODY_B)],
}
URL_A = F.sample_url(PINS, "a")
URL_B = F.sample_url(PINS, "b")


NO_PROXY = {"no_proxy": "*", "NO_PROXY": "*"}


class Response(io.BytesIO):
    """A response body that records how many bytes were read from it, at most `step` bytes a read.

    The fetcher reads with read1, as it must on a real response (see TrickleTests).
    """

    def __init__(self, data, server):
        super().__init__(data)
        self.server = server

    def read(self, n=-1):
        raise AssertionError("read(n) waits for all n bytes on a real response; the fetcher reads with read1")

    def read1(self, n=-1):
        if self.server.step:
            n = min(n, self.server.step) if n >= 0 else self.server.step
        out = super().read1(n)
        self.server.bytes_read += len(out)
        if self.server.tick:
            self.server.now += self.server.tick
        return out


class Server:
    """A fake urlopen serving `bodies` by URL; the first `failures` calls raise `error`.

    With `step` and `tick` it trickles: each read returns at most `step` bytes
    and moves its clock (`now`, read through `clock`) on by `tick` seconds.
    """

    def __init__(self, bodies, failures=0, error=None, step=0, tick=0):
        self.bodies, self.failures = bodies, failures
        self.error = error or ConnectionResetError("connection reset by peer")
        self.step, self.tick, self.now = step, tick, 0.0
        self.calls, self.bytes_read = [], 0

    def __call__(self, url, timeout):
        self.calls.append(url)
        if self.failures:
            self.failures -= 1
            raise self.error
        return Response(self.bodies[url], self)

    def clock(self):
        return self.now


class FetchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-samples-"))
        self.addCleanup(shutil.rmtree, self.tmp)
        self.dest = self.tmp / "samples"
        self.dest.mkdir()
        self.sleeps = []

    def fetch(self, server, dest=None, pins=PINS):
        """main()'s exit status and what it wrote on stderr."""
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            code = F.main(
                [str(dest or self.dest)], opener=server, sleep=self.sleeps.append, clock=server.clock, pins=pins
            )
        return code, err.getvalue()

    def refused(self, server, dest=None, pins=PINS):
        """main()'s usage-error exit (2) and its message, for arguments or pins it refuses."""
        with contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit) as cm:
            F.main([str(dest or self.dest)], opener=server, sleep=self.sleeps.append, clock=server.clock, pins=pins)
        self.assertEqual(cm.exception.code, 2)
        return err.getvalue()

    def assert_holds(self, names, folder=None):
        self.assertEqual(sorted(p.name for p in (folder or self.dest).iterdir()), sorted(names))

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
        self.assert_holds(["a", "b"])

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

    def test_a_trickling_body_is_cut_off_at_the_deadline(self):
        # One byte a read, each read a second apart: every read is quick, but
        # the body would take len(BODY_A) seconds in all.
        self.assertGreater(len(BODY_A), F.DEADLINE_S)
        server = Server({URL_A: BODY_A, URL_B: BODY_B}, step=1, tick=1.0)
        code, err = self.fetch(server)
        self.assertEqual(code, 1)
        self.assertIn(f"not downloaded within {F.DEADLINE_S} s", err)
        self.assertEqual(server.calls, [URL_A], "the deadline covers the retries too")
        self.assertLessEqual(server.bytes_read, F.DEADLINE_S + 1)
        self.assert_holds([])

    def test_the_deadline_bounds_the_retries(self):
        # Each failed attempt costs 200 s: the second one starts after the deadline.
        class Slow(Server):
            def __call__(self, url, timeout):
                self.now += 200
                return super().__call__(url, timeout)

        server = Slow({URL_A: BODY_A, URL_B: BODY_B}, failures=F.ATTEMPTS)
        code, err = self.fetch(server)
        self.assertEqual(code, 1)
        self.assertIn(f"not downloaded within {F.DEADLINE_S} s", err)
        self.assertLess(len(server.calls), F.ATTEMPTS)

    def test_refuses_a_folder_inside_the_repository(self):
        server = Server({URL_A: BODY_A, URL_B: BODY_B})
        inside = HERE / "samples-must-not-land-here"
        self.assertIn("refusing to put the samples inside the repository", self.refused(server, inside))
        self.assertFalse(inside.exists())
        self.assertEqual(server.calls, [])

    def test_refuses_a_destination_that_is_a_file(self):
        (self.tmp / "file").write_bytes(b"")
        self.assertIn("is not a folder", self.refused(Server({}), self.tmp / "file"))

    # ------------------------------------------------------------- the pins

    def test_refuses_names_that_are_not_plain_file_names(self):
        for name in ("../escaped", str(self.tmp / "escaped"), "sub/escaped", "..", ".", ".hidden", "a\0b", ""):
            with self.subTest(name=name):
                pins = {**PINS, "files": [pin(name, BODY_A)]}
                server = Server({F.sample_url(pins, name): BODY_A})
                self.assertIn("is not a plain file name", self.refused(server, pins=pins))
                self.assertEqual(server.calls, [])
                self.assert_holds(["samples"], self.tmp)
                self.assert_holds([])

    def test_refuses_pins_off_the_media_host_or_malformed(self):
        bad = {
            "http": {**PINS, "url": "http://media.githubusercontent.com/media/K0lb3/UnityPy/{commit}/{name}"},
            "other host": {**PINS, "url": "https://samples.invalid/{commit}/{name}"},
            "file": {**PINS, "url": "file:///tmp/{name}"},
            "lookalike host": {**PINS, "url": "https://media.githubusercontent.com.invalid/{commit}/{name}"},
            "no name": {**PINS, "url": F.URL_PREFIX + "{commit}/x"},
            "commit": {**PINS, "commit": "../../x"},
            "short commit": {**PINS, "commit": "c" * 39},
            "sha256": {**PINS, "files": [{**pin("a", BODY_A), "sha256": "A" * 64}]},
            "bytes": {**PINS, "files": [{**pin("a", BODY_A), "bytes": 0}]},
            "twice": {**PINS, "files": [pin("a", BODY_A), pin("a", BODY_A)]},
            "no files": {**PINS, "files": []},
        }
        for label, pins in bad.items():
            with self.subTest(label):
                server = Server({})
                self.assertIn("refusing the pins", self.refused(server, pins=pins))
                self.assertEqual(server.calls, [])

    # ----------------------------------------------------- what DEST holds

    def test_refuses_anything_in_the_folder_but_the_pinned_samples(self):
        victim = self.tmp / "victim"
        victim.write_bytes(b"untouched")
        cases = {
            "an unpinned file": lambda: (self.dest / "extra_bundle").write_bytes(BODY_B),
            "a symlink to a correct copy": lambda: os.symlink(self.tmp / "copy_a", self.dest / "a"),
            "a folder": lambda: (self.dest / "a").mkdir(),
            "a stale temporary file": lambda: (self.dest / ".a.x1y2.partial").write_bytes(b""),
            # The temporary name the fetcher once used, pointing outside DEST.
            "a planted .partial symlink": lambda: os.symlink(victim, self.dest / "a.partial"),
        }
        (self.tmp / "copy_a").write_bytes(BODY_A)
        for label, plant in cases.items():
            with self.subTest(label):
                shutil.rmtree(self.dest)
                self.dest.mkdir()
                (self.dest / "b").write_bytes(BODY_B[:-1] + b"?")  # wrong, so a fetch would write it
                plant()
                server = Server({URL_A: BODY_A, URL_B: BODY_B})
                code, err = self.fetch(server)
                self.assertEqual(code, 1)
                self.assertIn("may hold only the pinned samples", err)
                self.assertEqual(server.calls, [])
                self.assertEqual(victim.read_bytes(), b"untouched")
                self.assertEqual((self.dest / "b").read_bytes(), BODY_B[:-1] + b"?")

    def test_the_temporary_file_is_created_exclusively(self):
        # A symlink already at the temporary name is never written through:
        # the name is taken, so the fetcher picks another.
        victim = self.tmp / "victim"
        victim.write_bytes(b"untouched")
        dest = self.dest

        class Planting(Server):
            """Plants the symlink while the download runs, after the folder was checked."""

            def __call__(self, url, timeout):
                if url == URL_A:
                    os.symlink(victim, dest / ".a.fixed.partial")
                return super().__call__(url, timeout)

        server = Planting({URL_A: BODY_A, URL_B: BODY_B})
        names = iter(["fixed", "other", "another"])
        with mock.patch.object(F.tempfile, "_get_candidate_names", return_value=names):
            code, _ = self.fetch(server)
        self.assertEqual(code, 0)
        self.assertEqual(victim.read_bytes(), b"untouched")
        self.assertEqual((self.dest / "a").read_bytes(), BODY_A)
        self.assertFalse((self.dest / "a").is_symlink())
        self.assertTrue((self.dest / ".a.fixed.partial").is_symlink())

    # ------------------------------------------------------------ redirects

    def test_a_redirect_is_refused_without_a_retry(self):
        server = Server({URL_A: BODY_A, URL_B: BODY_B}, failures=1, error=F.FetchError("redirected (302) to x"))
        code, err = self.fetch(server)
        self.assertEqual(code, 1)
        self.assertIn("redirected (302)", err)
        self.assertEqual((server.calls, self.sleeps), ([URL_A], []))

    def test_the_opener_follows_no_redirect(self):
        hits = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                hits.append((self.server.server_address[1], self.path))
                if self.path == "/sample":
                    self.send_response(302)
                    self.send_header("Location", f"http://127.0.0.1:{other.server_address[1]}/elsewhere")
                    self.send_header("Content-Length", "0")
                else:
                    self.send_response(200)
                    self.send_header("Content-Length", "1")
                self.end_headers()
                if self.path != "/sample":
                    self.wfile.write(b"x")

            def log_message(self, *args):
                pass

        servers = [http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) for _ in range(2)]
        pinned, other = servers
        for s in servers:
            threading.Thread(target=s.serve_forever, daemon=True).start()
            self.addCleanup(s.server_close)
            self.addCleanup(s.shutdown)
        with mock.patch.dict(os.environ, NO_PROXY), self.assertRaises(F.FetchError) as cm:
            F.urlopen(f"http://127.0.0.1:{pinned.server_address[1]}/sample", timeout=10)
        self.assertIn("redirected (302)", str(cm.exception))
        self.assertEqual(hits, [(pinned.server_address[1], "/sample")])

    def test_the_pins_are_the_samples_the_tests_check(self):
        pins = F.load_pins()
        self.assertEqual(pins, json.loads(F.PINS.read_text(encoding="utf-8")))
        F.check_pins(pins)
        self.assertEqual(pins["commit"], "417d59522a998fdee9a9c2b29fe35f3063222457")
        self.assertEqual({p["name"]: p["sha256"] for p in pins["files"]}, SAMPLES)
        self.assertEqual(
            F.sample_url(pins, "banner_1"),
            "https://media.githubusercontent.com/media/K0lb3/UnityPy/"
            "417d59522a998fdee9a9c2b29fe35f3063222457/tests/samples/banner_1",
        )


class TrickleTests(unittest.TestCase):
    """download() against a real HTTP server on the loopback interface, whose body trickles in.

    On http.client's response, read(n) waits until all n bytes are in, and
    each receive waits up to the socket's timeout; a fake response does
    neither. These check that the deadline bounds the wall time anyway.
    """

    def serve(self, pieces):
        """A loopback URL whose body, of Content-Length the pieces' total, comes as [(wait_s, bytes)] in turn."""
        stop = threading.Event()

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Length", str(sum(len(data) for _, data in pieces)))
                self.end_headers()
                try:
                    for wait_s, data in pieces:
                        if stop.wait(wait_s):
                            return
                        self.wfile.write(data)
                except OSError:
                    pass  # the client gave up

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.addCleanup(stop.set)  # cleanups run last first: the handler stops before the server
        return f"http://127.0.0.1:{server.server_address[1]}/sample", sum(len(data) for _, data in pieces)

    def download(self, url, size, **limits):
        """download()'s FetchError and the seconds it took, with TIMEOUT_S and DEADLINE_S set to `limits`."""
        with (
            mock.patch.multiple(F, **limits),
            mock.patch.dict(os.environ, NO_PROXY),
            self.assertRaises(F.FetchError) as cm,
        ):
            t0 = time.monotonic()
            try:
                F.download(url, size, F.urlopen, sleep=lambda s: None)
            finally:
                elapsed = time.monotonic() - t0
        return str(cm.exception), elapsed

    def test_a_trickling_body_is_cut_off_at_the_deadline(self):
        # A byte every 0.2 s: each read gets data well within TIMEOUT_S, but the body takes 6 s.
        url, size = self.serve([(0.2, b"x")] * 30)
        said, elapsed = self.download(url, size, TIMEOUT_S=10, DEADLINE_S=1)
        self.assertIn("not downloaded within 1 s", said)
        self.assertLess(elapsed, 2.5, "read(n) would wait for the whole body")

    def test_a_waiting_read_ends_at_the_deadline(self):
        # The second byte comes at 2 s; the third read then waits with 0.5 s left
        # before the deadline. A socket timeout of TIMEOUT_S (2 s) would let it
        # wait until 4 s; set to the time left, it ends at 2.5 s.
        url, size = self.serve([(0, b"x"), (2, b"y"), (6, b"z")])
        said, elapsed = self.download(url, size, TIMEOUT_S=2, DEADLINE_S=2.5)
        self.assertIn("not downloaded within 2.5 s", said)
        self.assertGreaterEqual(elapsed, 2.4)
        self.assertLess(elapsed, 3.25, "the read waited past the deadline")


if __name__ == "__main__":
    unittest.main()
