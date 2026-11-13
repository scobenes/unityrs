"""`tools/fixtures/meshdecode.py --tpk` writes to no path, and enforces it: the TPK's trees and rows exist only in
memory and in a pipe (docs/plans/core.md section 9, Q2). The script installs its own audit hook before it imports
UnityPy (`NoWrites`), which raises `PermissionError("--tpk writes to no path: <event>")` before any write or spawn the
module docs list, and fails the run at its end when it refused one; these cases check that enforcement. The guard is
against accidental writes and leaks, not code written to evade it (the module docs).

- **The hook refuses what it should** (`TheHookRefuses`, in this process, the hook never installed here): every
  event the module docs list, the exemption's edges (a file in the folder, a folder beside it or named otherwise, a
  `dir_fd`), a second or altered replay Popen, and an event whose arguments it cannot read.
- **It comes first** (`TheHookComesFirst`): in the script's syntax tree the `--tpk` block, with `setrlimit` and
  `addaudithook`, comes before the first UnityPy import and after only an allowlist of imports, nested ones and any
  `__import__` or `import_module` call included, with bytecode writes stopped before `import scratch`; a `--tp` run
  exits 2 (no abbreviation runs the pass without the hook).
- **The real runs pass under it**: the cargo branch against a stub `cargo` (first on `PATH`, written by the case, no
  build), which reads its standard input to its end and requires the fixture's 526 cases by name, the replay's argv,
  `UNITYRS_MESHDECODE_TPK_STDIN`, the checkout as its working folder, no `UNITYRS_MESHDECODE_TPK_VERBOSE` (the case
  sets it, the script clears it) and child.run's own per-run folder as its `UNITYRS_TEST_SCRATCH`, and prints the
  replay's count line, libtest's ok line and the core-dump limit it inherited (0). That run goes without `-B`, with
  `PYTHONPYCACHEPREFIX` an owned folder, which must hold no bytecode of this repository's modules after. The
  `--no-cargo` branch passes too, and so does a run handed no scratch folder (an empty `UNITYRS_TEST_SCRATCH`),
  whose per-run folder sits in the root the script makes itself (`scratch.own_root()`), in its `TMPDIR`.
- **The stub's other modes fail the run**: `skip` (the ok line, no count line), `short` (`read 488`), `fail`
  (`0 passed`), `exit` (the count and ok lines, exit 101) and `nook` (exit 0, the count line, no ok line), each with
  "--tpk: the crate's replay failed or did not run"; `kill` (the stub SIGKILLs itself, so child.run's second Popen is
  refused) with "was killed (SIGKILL)", and a `cargo` that cannot be executed with "could not run (PermissionError)"
  and no word of a kill.
- **Planted refusals**: `UNITYRS_MESHDECODE_TPK_PLANT` names one write or spawn the script attempts after its TPK pass
  (`plant()`, the one test-only path in the script): an `atexit` write, a non-daemon thread's, `sqlite3.connect` to a
  file, `os.setxattr` (Linux), a `TemporaryFile`, a write under `target/`, a raw `os.open(O_CREAT)`, a second
  `subprocess.Popen`, and a refusal the script catches with its standard error closed before the exit check. Each
  must fail the run, with the hook's message where standard error is open, and leave no file.

Every run starts from an empty working folder with empty `TMPDIR` and `UNITYRS_TEST_SCRATCH` folders the case owns,
all three empty after, with no standard input and a timeout. The hook cannot see a native write or the replay's own
process (the module docs); `git status --porcelain -uall --ignored` (`target/` aside), the same before and after, is
the backstop here, as CI's checks are around its real step, and `TheReplayOpensNoFile` holds the replay's source to
an allowlist. CI's log is public, so a failure here prints counts and the script's own fixed lines only (a run that
times out or is killed on every try is recorded by a fixed reason), never a TPK-derived value or a path the runs
made. The runs need UnityPy, so the oracle image (CI's parity-py job, which fails a skip); they run at once, a
worker each, and the cases read their results. Standard library only besides.
"""

import ast
import concurrent.futures
import importlib.util
import json
import os
import re
import socket
import stat
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parents[1]
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import child  # noqa: E402
import scratch  # noqa: E402

SCRIPT = ROOT / "tools" / "fixtures" / "meshdecode.py"
FIXTURE = ROOT / "crates" / "unityrs-meshdecode" / "tests" / "fixtures" / "process.json"
ORACLE = ROOT / "crates" / "unityrs-meshdecode" / "tests" / "oracle.rs"
HAS_UNITYPY = importlib.util.find_spec("UnityPy") is not None
FAILED_REPLAY = "--tpk: the crate's replay failed or did not run"
REFUSED = "--tpk writes to no path: "
# Each plant, and the event the hook refuses it at (None: standard error is closed, so no message shows).
PLANTS = {"atexit": "open", "thread": "open", "sqlite": "sqlite3.connect", "tempfile": "open", "target": "open",
          "os.open": "open", "popen": "subprocess.Popen", "quiet": None}  # fmt: skip
if hasattr(os, "setxattr"):  # Linux
    PLANTS["setxattr"] = "os.setxattr"
# The files the plants would make, were they let.
PLANTED = ["atexit.txt", "thread.txt", "cache.sqlite", "raw.txt", "quiet.txt"]
MODES = ["skip", "short", "fail", "exit", "nook"]

# The stub `cargo`: the replay's side of the hand-off, without a build (module docs).
STUB = r"""
import json, os, resource, signal, sys

want = ["test", "--locked", "-p", "unityrs-meshdecode", "--test", "oracle", "--", "--exact",
        "the_cases_on_the_tpk_s_trees_answer_as_unitypy_does", "--nocapture"]
assert sys.argv[1:] == want, "the replay's argv"
assert os.environ.get("UNITYRS_MESHDECODE_TPK_STDIN") == "1"
assert "UNITYRS_MESHDECODE_TPK_VERBOSE" not in os.environ, "an inherited verbose flag reached the replay"
assert os.path.realpath(os.getcwd()) == os.path.realpath(os.environ["MESHDECODE_TPK_ROOT"]), "the working folder"
own = os.path.abspath(os.environ["UNITYRS_TEST_SCRATCH"])
assert os.path.isdir(own) and os.path.basename(own).startswith("unityrs-child-"), "the per-run folder"
where = os.environ["MESHDECODE_TPK_SCRATCH"]
if where == "own":  # the script's own root, in its TMPDIR
    root = os.path.dirname(own)
    assert os.path.basename(root).startswith("unityrs-test-"), "the script's own root"
    assert os.path.dirname(root) == os.path.abspath(os.environ["TMPDIR"]), "the own root's place"
else:
    assert os.path.dirname(own) == os.path.abspath(where), "the per-run folder's place"
body = json.loads(sys.stdin.read())
with open(os.environ["MESHDECODE_TPK_FIXTURE"], encoding="utf-8") as f:
    names = [c["name"] for c in json.load(f)["cases"]]
cases = body["cases"]
assert [c["name"] for c in cases] == names, "the cases handed over are not the fixture's"
assert all(c["tree"] in body["trees"] for c in cases)
mode = os.environ["MESHDECODE_TPK_STUB"]
if mode == "kill":  # as the OOM killer would: child.run runs it again, a second Popen the hook refuses
    os.kill(os.getpid(), signal.SIGKILL)
if mode != "skip":
    print(f"read {len(cases) - (mode == 'short')} cases from standard input")
if mode == "fail":
    print("test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 5 filtered out")
    sys.exit(101)
if mode != "nook":
    print("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out")
soft, hard = resource.getrlimit(resource.RLIMIT_CORE)
print(f"stub cargo: core limit {soft} {hard}")
sys.exit(101 if mode == "exit" else 0)
"""


def status() -> str:
    """The checkout's `git status`, `target/` aside (module docs)."""
    inside = child.run(["git", "rev-parse", "--is-inside-work-tree"], cwd=ROOT, stdout=child.PIPE, stderr=child.PIPE,
                       text=True)  # fmt: skip
    if inside.returncode != 0 or inside.stdout.strip() != "true":
        raise AssertionError(
            "git cannot read the checkout: in a container worktree, set GIT_DIR and GIT_WORK_TREE to its repository "
            "and work tree"
        )
    return child.run(
        ["git", "status", "--porcelain", "-uall", "--ignored", "--", ".", ":(exclude)target"],
        cwd=ROOT, check=True, stdout=child.PIPE, stderr=child.DEVNULL, text=True,
    ).stdout  # fmt: skip


def said(ran) -> str:
    """What a failure may print of a run: its exit status, its line counts and the script's own fixed lines."""
    own = [line for line in ran.stderr.splitlines() if line.startswith(("--tpk", "PermissionError: --tpk"))]
    return f"exit {ran.returncode}, {len(ran.stdout.splitlines())} lines out, {len(ran.stderr.splitlines())} err: {own}"


def outcome(future):
    """A run's result, or, where it timed out or SIGKILL ended it on every try, a fixed reason in its place: the
    exception's text holds the command and its output."""
    try:
        return future.result()
    except child.TimeoutExpired:
        reason = "timed out"
    except child.SystemKilled:
        reason = "was killed by SIGKILL on every try"
    return child.CompletedProcess([], -1, "", f"--tpk test: the run {reason}\n")


def files_under(folder: Path) -> int:
    return sum(len(files) + len(dirs) for _, dirs, files in os.walk(folder))


@unittest.skipUnless(HAS_UNITYPY, "needs UnityPy (the oracle image)")
class TheTpkPassWritesToNoPath(unittest.TestCase):
    """Every run at once (`setUpClass`); each case reads its own."""

    @classmethod
    def setUpClass(cls):
        top = scratch.for_class(cls)
        bin_ = top / "bin"
        bin_.mkdir()
        cargo = bin_ / "cargo"
        cargo.write_text(f"#!{sys.executable}\n{STUB}")
        cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
        cls.pycache = top / "pycache"
        cls.pycache.mkdir()
        noexec = top / "noexec"  # a cargo that cannot be executed, alone on PATH
        noexec.mkdir()
        (noexec / "cargo").write_text("not a program\n")
        # key: (stub mode, plant, script arguments, interpreter flags)
        runs = {"ok": ("ok", None, (), ()), "no-cargo": ("ok", None, ("--no-cargo",), ("-B",)),
                "own-root": ("ok", None, (), ("-B",)),
                "abbreviated": ("ok", None, ("--no-cargo",), ("-B",))}  # fmt: skip
        runs |= {mode: (mode, None, (), ("-B",)) for mode in [*MODES, "kill"]}
        runs["noexec"] = ("ok", None, (), ("-B",))
        runs |= {f"plant {p}": ("ok", p, (), ("-B",)) for p in PLANTS}
        cls.before = status()
        cls.folders, jobs = {}, {}
        for key, (mode, plant, args, flags) in runs.items():
            here = top / key.replace(" ", "-")
            folders = {name: here / name for name in ("cwd", "tmp", "scratch")}
            for folder in folders.values():
                folder.mkdir(parents=True)
            env = dict(os.environ, TMPDIR=str(folders["tmp"]), UNITYRS_TEST_SCRATCH=str(folders["scratch"]),
                       PATH=f"{bin_}{os.pathsep}{os.environ.get('PATH', '')}", UNITYRS_MESHDECODE_TPK_VERBOSE="1",
                       MESHDECODE_TPK_ROOT=str(ROOT), MESHDECODE_TPK_SCRATCH=str(folders["scratch"]),
                       MESHDECODE_TPK_FIXTURE=str(FIXTURE), MESHDECODE_TPK_STUB=mode)  # fmt: skip
            for name in ("UNITYRS_MESHDECODE_TPK_PLANT", "PYTHONPYCACHEPREFIX", "PYTHONDONTWRITEBYTECODE"):
                env.pop(name, None)
            if plant:
                env["UNITYRS_MESHDECODE_TPK_PLANT"] = plant
            if key == "ok":
                env["PYTHONPYCACHEPREFIX"] = str(cls.pycache)
            if key == "noexec":
                env["PATH"] = str(noexec)
            if key == "own-root":  # an empty one: child.run then hands the script none (child.py's _env)
                env[scratch.VAR] = ""
                env["MESHDECODE_TPK_SCRATCH"] = "own"
            tpk = "--tp" if key == "abbreviated" else "--tpk"
            cls.folders[key] = folders
            jobs[key] = ([sys.executable, *flags, str(SCRIPT), tpk, *args], folders["cwd"], env)
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(jobs)) as pool:
            futures = {
                key: pool.submit(child.run, argv, cwd=cwd, env=env, stdin=child.DEVNULL, stdout=child.PIPE,
                                 stderr=child.PIPE, text=True, timeout=1800)  # fmt: skip
                for key, (argv, cwd, env) in jobs.items()
            }
            cls.ran = {key: outcome(f) for key, f in futures.items()}
        cls.after = status()

    def run_of(self, key: str):
        ran = self.ran[key]
        for name, folder in self.folders[key].items():
            self.assertEqual(files_under(folder), 0, f"{key}: its {name} folder is not empty")
        self.assertFalse((ROOT / "target" / "tpk-plant.json").exists(), "a planted file under target/")
        return ran

    def test_the_checkout_is_as_it_was(self):
        before, after = self.before.splitlines(), self.after.splitlines()
        n = len(set(before) ^ set(after))
        self.assertTrue(before == after, f"git status changed: {n} lines differ")

    def test_the_cargo_branch_hands_the_cases_over_under_the_hook(self):
        ran = self.run_of("ok")
        self.assertEqual(ran.returncode, 0, said(ran))
        n = len(json.loads(FIXTURE.read_text())["cases"])
        self.assertEqual(n, 526)
        self.assertTrue(ran.stdout.startswith("--tpk: 526 cases on the TPK's trees, 0 outcomes differ\n"), said(ran))
        self.assertTrue(f"read {n} cases from standard input\n" in ran.stdout, said(ran))
        self.assertTrue("test result: ok. 1 passed;" in ran.stdout, said(ran))
        self.assertTrue("stub cargo: core limit 0 0\n" in ran.stdout, said(ran))
        self.assertFalse(REFUSED in ran.stderr, said(ran))

    def test_the_run_without_b_writes_no_bytecode_of_the_repository(self):
        self.run_of("ok")
        mirrored = self.pycache / str(ROOT).lstrip(os.sep)
        self.assertEqual(files_under(mirrored) if mirrored.exists() else 0, 0, "bytecode of the repository's modules")

    def test_the_no_cargo_branch_passes_under_the_hook(self):
        ran = self.run_of("no-cargo")
        self.assertEqual(ran.returncode, 0, said(ran))
        count = r"--tpk: \d+ cases on the TPK's trees, 0 outcomes differ\n"
        self.assertTrue(re.fullmatch(count, ran.stdout), said(ran))
        self.assertFalse(REFUSED in ran.stderr, said(ran))

    def test_a_run_with_no_parent_scratch_makes_and_removes_its_own_root(self):
        ran = self.run_of("own-root")
        self.assertEqual(ran.returncode, 0, said(ran))
        self.assertTrue("test result: ok. 1 passed;" in ran.stdout, said(ran))
        self.assertFalse(REFUSED in ran.stderr, said(ran))

    def test_an_abbreviated_flag_runs_nothing(self):
        ran = self.run_of("abbreviated")
        self.assertEqual(ran.returncode, 2, said(ran))
        self.assertTrue(ran.stdout == "", said(ran))

    def test_a_replay_that_skips_reads_short_fails_or_ends_otherwise_fails_the_run(self):
        for mode in MODES:
            with self.subTest(mode=mode):
                ran = self.run_of(mode)
                self.assertEqual(ran.returncode, 1, said(ran))
                self.assertTrue(FAILED_REPLAY in ran.stderr, said(ran))
                self.assertFalse(REFUSED in ran.stderr, said(ran))

    def test_a_killed_replay_is_not_run_again_and_says_so(self):
        ran = self.run_of("kill")
        self.assertEqual(ran.returncode, 1, said(ran))
        killed = "--tpk: the crate's replay was killed (SIGKILL) and is not run again under --tpk\n"
        self.assertTrue(killed in ran.stderr, said(ran))

    def test_a_replay_that_cannot_start_is_no_kill(self):
        ran = self.run_of("noexec")
        self.assertEqual(ran.returncode, 1, said(ran))
        self.assertTrue("--tpk: the crate's replay could not run (PermissionError)\n" in ran.stderr, said(ran))
        self.assertFalse("SIGKILL" in ran.stderr, said(ran))

    def test_each_planted_write_or_spawn_is_refused_and_fails_the_run(self):
        for plant, event in PLANTS.items():
            with self.subTest(plant=plant):
                ran = self.run_of(f"plant {plant}")
                self.assertEqual(ran.returncode, 1, said(ran))
                if event is not None:
                    self.assertTrue(f"PermissionError: {REFUSED}{event}\n" in ran.stderr, said(ran))
                    line = rf"^--tpk: refused [0-9]+ writes to a path or spawns \({re.escape(event)}\); the run fails$"
                    self.assertTrue(re.search(line, ran.stderr, re.M), said(ran))
                tmp = self.folders[f"plant {plant}"]["tmp"]
                for name in PLANTED:
                    self.assertFalse((tmp / name).exists(), f"{plant}: a planted file")


def load_script():
    """meshdecode.py as a module, without `--tpk`, so its hook is never installed in this process."""
    spec = importlib.util.spec_from_file_location("meshdecode_under_test", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@unittest.skipUnless(HAS_UNITYPY, "needs UnityPy (the oracle image)")
class TheHookRefuses(unittest.TestCase):
    """`NoWrites` called as the interpreter calls a hook, in this process, which never installs it."""

    @classmethod
    def setUpClass(cls):
        assert "--tpk" not in sys.argv[1:]
        cls.md = load_script()
        scratch.for_class(cls)  # this process's scratch root, so the exemption has one to match
        cls.root = Path(os.path.abspath(scratch.root()))

    def verdicts(self, nw, cases):
        for i, (event, args, refused) in enumerate(cases):
            with self.subTest(case=i, event=event):
                before = len(nw.refused)
                if refused:
                    with self.assertRaisesRegex(PermissionError, re.escape(REFUSED + event)):
                        nw(event, args)
                    self.assertEqual(nw.refused[before:], [event])
                else:
                    nw(event, args)
                    self.assertEqual(len(nw.refused), before)

    def test_every_listed_event_is_refused(self):
        unix, inet = SimpleNamespace(family=socket.AF_UNIX), SimpleNamespace(family=socket.AF_INET)
        w = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
        cases = [
            ("open", ("/x/f", "w", w), True), ("open", ("/x/f", "a", os.O_WRONLY | os.O_APPEND), True),
            ("open", ("/x/f", "r+", os.O_RDWR), True), ("open", ("/x/f", None, os.O_WRONLY | os.O_CREAT), True),
            ("open", ("/x/f", "x", os.O_WRONLY | os.O_CREAT | os.O_EXCL), True), ("open", ("/x/f", "r", 0), False),
            ("open", (b"/x/f", "wb", w), True), ("open", (Path("/x/f"), "w", w), True), ("open", (7, "w", w), False),
            ("os.mkdir", ("/x/d", 0o777, -1), True), ("os.rename", ("/a", "/b", -1, -1), True),
            ("os.replace", ("/a", "/b", -1, -1), True), ("os.link", ("/a", "/b", -1, -1), True),
            ("os.symlink", ("/a", "/b", -1), True), ("os.remove", ("/a", -1), True), ("os.rmdir", ("/a", -1), True),
            ("os.truncate", ("/a", 0), True), ("os.setxattr", ("/a", "user.x", b"", 0, True), True),
            ("os.removexattr", ("/a", "user.x", True), True), ("os.utime", ("/a", None, None, -1), True),
            ("os.chmod", ("/a", 0o600, -1), True), ("os.chown", ("/a", 0, 0, -1), True),
            ("os.chflags", ("/a", 0), True), ("os.lchflags", ("/a", 0), True),
            ("tempfile.mkstemp", ("/tmp/x",), True), ("tempfile.mkdtemp", ("/tmp/x",), True),
            ("shutil.copyfile", ("/a", "/b"), True), ("shutil.rmtree", ("/a", None), True),
            ("shutil.make_archive", ("/a", "zip", None, None), True),
            ("sqlite3.connect", (":memory:",), True), ("sqlite3.connect", ("/x.db",), True),
            ("syslog.openlog", ("x", 0, 8), True), ("syslog.syslog", (6, "x"), True),
            ("socket.bind", (unix, "/x.sock"), True), ("socket.bind", (unix, b"/x.sock"), True),
            ("socket.bind", (unix, bytearray(b"/x.sock")), True), ("socket.bind", (unix, "\0abstract"), True),
            ("socket.connect", (unix, "/dev/log"), True), ("socket.sendto", (unix, "/dev/log"), True),
            ("socket.sendmsg", (unix, "/dev/log"), True), ("socket.sendmsg", (unix, None), True),
            ("socket.sendmsg", (inet, None), False),
            ("socket.bind", (inet, ("127.0.0.1", 0)), False), ("socket.connect", (inet, ("127.0.0.1", 9)), False),
            ("os.system", (b"true",), True), ("os.exec", ("/bin/true", ["true"], None), True),
            ("os.posix_spawn", ("/bin/true", ["true"], None), True), ("os.spawn", (0, "/bin/true", ["t"], None), True),
            ("os.fork", (), True), ("os.forkpty", (), True), ("import", ("json", None, [], [], []), False),
        ]
        self.verdicts(self.md.NoWrites(), cases)

    def test_the_exemption_is_child_run_s_folder_alone(self):
        root, md = self.root, self.md
        own = root / "unityrs-child-abcd_123"
        own_root = Path(md.NoWrites().own_root)
        cases = [
            ("os.mkdir", (str(own), 0o700, -1), False), ("tempfile.mkdtemp", (str(own),), False),
            ("os.chmod", (str(own), 0o700, -1), False), ("os.rmdir", (str(own), -1), False),
            ("os.mkdir", (own, 0o700, None), False), ("os.mkdir", (os.fsencode(own), 0o700, -1), False),
            ("os.mkdir", (str(own), 0o700, 3), True), ("os.rmdir", (str(own), 3), True),
            ("os.chmod", (str(own), 0o700, 3), True), ("os.mkdir", ("unityrs-child-abcd_123", 0o700, 3), True),
            ("os.mkdir", (str(root / "unityrs-child-abc"), 0o700, -1), True),
            ("os.mkdir", (str(root / "unityrs-child-abcd_1234"), 0o700, -1), True),
            ("os.mkdir", (str(root / "unityrs-other-abcd_123"), 0o700, -1), True),
            ("os.mkdir", (str(root.parent / "unityrs-child-abcd_123"), 0o700, -1), True),
            ("os.mkdir", (str(own / "unityrs-child-abcd_123"), 0o700, -1), True),
            ("os.remove", (str(own / "f"), -1), True), ("open", (str(own / "f"), "w", os.O_WRONLY | os.O_CREAT), True),
            ("os.rename", (str(own), str(root / "unityrs-child-zzzz_999"), -1, -1), True),
            ("os.utime", (str(own), None, None, -1), True), ("os.remove", (str(own), -1), True),
            # The script's own root, only where no parent named another.
            ("os.mkdir", (str(own_root), 0o700, -1), scratch.root() is not None and root != own_root),
        ]
        self.verdicts(md.NoWrites(), cases)

    def test_only_the_replay_s_one_popen_runs(self):
        md, replay = self.md, self.md.REPLAY
        nw = md.NoWrites()
        self.verdicts(nw, [("subprocess.Popen", (None, replay, md.ROOT, None), False),
                           ("os.posix_spawn", ("/usr/bin/cargo", replay, None), True),
                           ("subprocess.Popen", (None, replay, md.ROOT, None), True)])  # fmt: skip
        self.assertTrue(nw.popen)
        for args in [("/bin/sh", replay, md.ROOT, None), (None, replay, "/tmp", None), (None, replay, None, None),
                     (None, replay[:-1], md.ROOT, None), (None, " ".join(replay), md.ROOT, None),
                     ("cargo", replay, str(md.ROOT), None)]:  # fmt: skip
            fresh = md.NoWrites()
            self.verdicts(fresh, [("subprocess.Popen", args, args[0] != "cargo")])

    def test_an_event_it_cannot_read_is_refused(self):
        cases = [("open", (), True), ("subprocess.Popen", (None,), True), ("socket.bind", (object(), "/x"), True),
                 ("os.mkdir", (), True)]  # fmt: skip
        self.verdicts(self.md.NoWrites(), cases)

    def test_it_answers_the_probe_that_tpk_s_readers_ask(self):
        import meshdecode_build

        self.assertEqual(meshdecode_build.PROBE, self.md.PROBE)
        seen = []
        self.md.NoWrites()(self.md.PROBE, (seen,))
        self.assertEqual(seen, [True])
        self.assertFalse(meshdecode_build.under_no_writes(), "a hook answers the probe in this process")
        with self.assertRaises(RuntimeError):
            meshdecode_build.tpk_rows(43, (2019, 4, 0, 0, 0))


class TheHookComesFirst(unittest.TestCase):
    BEFORE = {"__future__", "os", "re", "resource", "socket", "sys", "pathlib", "scratch"}

    def test_the_tpk_block_precedes_unitypy_and_follows_only_the_allowlist(self):
        body = ast.parse(SCRIPT.read_text()).body

        def imports(node):
            if isinstance(node, ast.Import):
                return {a.name.split(".")[0] for a in node.names}
            if isinstance(node, ast.ImportFrom):
                return {(node.module or "").split(".")[0]}
            return set()

        def calls(node):
            found = (n for n in ast.walk(node) if isinstance(n, ast.Call))
            return {n.func.attr for n in found if isinstance(n.func, ast.Attribute)}

        unitypy = next(i for i, n in enumerate(body) if "UnityPy" in imports(n))
        block = next(i for i, n in enumerate(body) if isinstance(n, ast.If) and "addaudithook" in calls(n))
        self.assertLess(block, unitypy, "the --tpk block comes after UnityPy's import")
        self.assertTrue({"setrlimit", "register", "addaudithook", "_ncallbacks"} <= calls(body[block]))
        before = [n for top in body[:block] for n in ast.walk(top)]  # nested ones too
        early = set().union(*(imports(n) for n in before))
        self.assertEqual(early - self.BEFORE, set(), "imports before the --tpk block")
        dynamic = {n.func.id if isinstance(n.func, ast.Name) else n.func.attr for n in before
                   if isinstance(n, ast.Call) and isinstance(n.func, (ast.Name, ast.Attribute))}  # fmt: skip
        self.assertEqual(dynamic & {"__import__", "import_module"}, set(), "a dynamic import before the --tpk block")
        flag = next(i for i, n in enumerate(body) if "dont_write_bytecode" in ast.dump(n))
        first_repo = next(i for i, n in enumerate(body) if "scratch" in imports(n))
        self.assertLess(flag, first_repo, "bytecode writes are stopped after a module of the repository is imported")


class TheReplayOpensNoFile(unittest.TestCase):
    """The hook cannot follow into the replay's process, so its source is held to an allowlist (fixtures compiled
    in). A failure names the token alone."""

    STD = ("std::sync", "std::fmt", "std::env::var_os", "std::io::stdin", "std::io::Read", "std::collections")
    TESTKIT = {"blob", "env", "serialized"}
    ROOTS = {"std", "serde_json", "unityrs_core", "unityrs_testkit", "unityrs_meshdecode", "crate", "self", "super",
             "walk", "serialized", "env", "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize",
             "f32", "f64", "bool", "char", "str"}  # fmt: skip
    FORBIDDEN = [r"\bfs\b", r"\bprocess::", r"\bCommand\b", r"\bnet::", r"\bchild::", r"\bstack::", r"\binclude!",
                 r"#\s*\[\s*path", r"^\s*(?:pub\s+)?mod\s+\w+\s*;", r"\bextern\s+crate\b"]  # fmt: skip

    def test_the_replay_s_source_names_only_allowed_paths(self):
        text = ORACLE.read_text()
        for pattern in self.FORBIDDEN:
            with self.subTest(pattern=pattern):
                self.assertIsNone(re.search(pattern, text, re.M), f"forbidden: {pattern}")
        for path in sorted(set(re.findall(r"\bstd::[\w:]*\{?", text))):
            path = path.rstrip(":")
            ok = any(path == a or path.startswith(a + "::") for a in self.STD)
            self.assertTrue(ok, f"a std path outside the allowlist: {path[:60]}")
        for module in set(re.findall(r"\bunityrs_testkit::(\w*\{?)", text)):
            self.assertIn(module, self.TESTKIT, "a testkit module outside the allowlist")
        roots = set(re.findall(r"(?<![\w:.])([a-z_][a-z0-9_]*)::", text))
        self.assertEqual(roots - self.ROOTS, set(), "path roots outside the allowlist")
        uses = set(re.findall(r"^\s*(?:pub\s+)?use\s+(?:::)?(\w+)", text, re.M))
        self.assertEqual(uses - {"std", "serde_json", "unityrs_core", "unityrs_testkit", "unityrs_meshdecode"}, set())


if __name__ == "__main__":
    unittest.main()
