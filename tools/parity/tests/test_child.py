"""child.py and scratch.py, the two rules every Python test and tool here keeps, and the scans that hold every
Python file in the repository to them: a child the system kills from outside is run again, never read as an
answer (review round 9: two sites read a kill as one); a test's scratch is removed whatever happens to the
test or its child (review round 9: a failing setUpClass, and a child that dies, left theirs). Standard library
only.
"""

import ast
import os
import signal
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import child  # noqa: E402
import scratch  # noqa: E402

PY = sys.executable
# What a child of these tests runs first: the tools on its path, the marks folder its argv[1] names.
BOOT = f"import os, signal, sys; sys.path.insert(0, {str(HERE)!r}); import scratch\n"


def marked(n, last):
    """A Python child that marks its run in the folder argv[1] names; on each of its first `n` runs SIGKILL
    then ends it (it sends it to itself, as the OOM killer would: a helper process sending it would outlive the
    child), and after them it runs `last`."""
    return BOOT + (
        "marks = sys.argv[1]\n"
        "runs = len(os.listdir(marks))\n"
        "open(os.path.join(marks, str(runs)), 'w').close()\n"
        f"if runs < {n}:\n"
        "    os.kill(os.getpid(), signal.SIGKILL)\n"
        f"{last}\n"
    )


def runs(marks):
    return len(os.listdir(marks))


class RunTests(unittest.TestCase):
    def setUp(self):
        self.marks = scratch.for_case(self, "unityrs-marks-")

    def test_a_child_killed_from_outside_once_is_run_again(self):
        proc = child.run([PY, "-c", marked(1, "print('done'); sys.exit(3)"), self.marks], capture_output=True)
        self.assertEqual((proc.returncode, proc.stdout, runs(self.marks)), (3, b"done\n", 2))

    def test_a_child_killed_on_all_but_its_last_run_comes_to_a_result(self):
        n = child.KILLED_RUNS - 1
        proc = child.run([PY, "-c", marked(n, "sys.exit(0)"), self.marks], capture_output=True, text=True)
        self.assertEqual((proc.returncode, runs(self.marks)), (0, n + 1))

    def test_a_child_killed_on_every_run_fails_saying_the_system_killed_it(self):
        n = child.KILLED_RUNS
        with self.assertRaises(child.SystemKilled) as cm:
            child.run([PY, "-c", marked(n, "sys.exit(0)"), self.marks], capture_output=True)
        self.assertEqual(runs(self.marks), n)
        self.assertIn(f"killed by SIGKILL on each of its {n} runs", str(cm.exception))
        self.assertIn("the system ended it from outside (the OOM killer, or a person)", str(cm.exception))

    def test_every_other_ending_is_the_first_runs_result(self):
        for last, code in (
            ("sys.exit(0)", 0),
            ("sys.exit(101)", 101),
            ("os.abort()", -signal.SIGABRT),
            ("os.kill(os.getpid(), signal.SIGSEGV)", -signal.SIGSEGV),
            ("os.kill(os.getpid(), signal.SIGTERM)", -signal.SIGTERM),
        ):
            with self.subTest(last):
                marks = scratch.make("unityrs-marks-", within=self.marks)
                code_ = "import resource; resource.setrlimit(resource.RLIMIT_CORE, (0, 0))\n" + last
                proc = child.run([PY, "-c", marked(0, code_), marks], capture_output=True)
                self.assertEqual((proc.returncode, runs(marks)), (code, 1))

    def test_a_timeout_is_the_callers_kill_and_never_run_again(self):
        with self.assertRaises(child.subprocess.TimeoutExpired):
            child.run([PY, "-c", marked(0, "import time; time.sleep(60)"), self.marks], timeout=2)
        self.assertLessEqual(runs(self.marks), 1, "run again")

    def test_check_raises_as_subprocess_run_does(self):
        with self.assertRaises(child.subprocess.CalledProcessError):
            child.run([PY, "-c", "import sys; sys.exit(2)"], check=True)

    def test_a_stream_a_second_run_would_reuse_is_refused(self):
        with open(self.marks / "out", "wb") as fh, self.assertRaises(TypeError):
            child.run([PY, "-c", "pass"], stdout=fh)


class ForkedTests(unittest.TestCase):
    def test_a_fork_killed_from_outside_is_run_again_and_its_own_ending_is_the_result(self):
        marks = scratch.for_case(self, "unityrs-marks-")

        def killed_once():
            n = len(os.listdir(marks))
            (marks / str(n)).touch()
            os.kill(os.getpid(), signal.SIGKILL if n < 1 else signal.SIGTERM)

        status = child.forked(killed_once)
        self.assertEqual((os.WTERMSIG(status), runs(marks)), (signal.SIGTERM, 2))

        def always():
            os.kill(os.getpid(), signal.SIGKILL)

        with self.assertRaises(child.SystemKilled):
            child.forked(always)


class RunAllTests(unittest.TestCase):
    def setUp(self):
        self.tmp = scratch.for_case(self, "unityrs-run-all-")

    def job(self, key, code):
        marks = scratch.make("unityrs-marks-", within=self.tmp)
        return key, [PY, "-c", code, str(marks)], self.tmp / f"{key}.out"

    def test_a_kill_from_outside_is_run_again_and_the_callers_own_stop_is_the_result(self):
        jobs = [
            self.job("again", marked(1, "print('second'); sys.exit(0)")),
            self.job("stopped", marked(0, "import time; time.sleep(60)")),
        ]
        ends = child.run_all(jobs, 2, stop=lambda proc: proc.args == jobs[1][1] and runs(jobs[1][1][3]) > 0)
        self.assertEqual(ends["again"], child.Ending(0, False))
        self.assertEqual(ends["stopped"], child.Ending(-signal.SIGKILL, True))
        self.assertEqual((self.tmp / "again.out").read_text(), "second\n", "the second run's output alone")
        self.assertEqual([runs(j[1][3]) for j in jobs], [2, 1])

    def test_a_child_killed_on_every_run_fails_and_leaves_nothing_running(self):
        sleeper = self.job("sleeper", BOOT + "print(os.getpid(), file=open(sys.argv[1] + '/pid', 'w'), flush=True)\n"
                           "import time; time.sleep(60)\n")
        # The other waits (60 s at most) until the sleeper has said who it is, then is killed on every run.
        wait = f"import time\nfor _ in range(6000):\n    if os.listdir({sleeper[1][3]!r}): break\n    time.sleep(0.01)\n"
        always = self.job("always", marked(9, "").replace("marks = sys.argv[1]\n", wait + "marks = sys.argv[1]\n"))
        with self.assertRaises(child.SystemKilled):
            child.run_all([sleeper, always], 2)
        self.assertEqual(runs(always[1][3]), child.KILLED_RUNS)
        pid = (Path(sleeper[1][3]) / "pid").read_text().strip()
        self.assertFalse(os.path.exists(f"/proc/{pid}"), "the other child was left running")
        self.assertEqual([n for n in os.listdir(scratch._root) if n.startswith("unityrs-child-")], [])


class ScratchTests(unittest.TestCase):
    def test_a_child_that_dies_mid_write_leaves_nothing(self):
        """The parent removes the folder it gave a child, however the child ends: a child that aborts, or that
        the system kills, runs none of its own cleanups (review round 9: a probe's folder outlived its child)."""
        marks = scratch.for_case(self, "unityrs-marks-")
        code = BOOT + (
            "marks = sys.argv[1]\n"
            "runs = len(os.listdir(marks))\n"
            "folder = scratch.make('unityrs-mid-write-')\n"
            "with open(os.path.join(marks, str(runs)), 'w') as fh:\n"
            "    fh.write(f'{folder}\\n{os.environ[scratch.VAR]}')\n"
            "with open(folder / 'half', 'wb') as fh:\n"
            "    fh.write(b'x' * 4096)\n"
            "    fh.flush()\n"
            "    os.kill(os.getpid(), signal.SIGKILL if runs < 1 else signal.SIGABRT)\n"
        )
        proc = child.run([PY, "-c", code, marks], capture_output=True)
        self.assertEqual(proc.returncode, -signal.SIGABRT, proc.stderr)
        written = [(marks / str(n)).read_text().split("\n") for n in (0, 1)]
        self.assertEqual(len({w[1] for w in written}), 2, "each run has a folder of its own")
        for folder, given in written:
            self.assertTrue(folder.startswith(given), (folder, given))
            self.assertFalse(os.path.exists(given), given)

    def own_root_child(self, last):
        """A child with no parent's folder, so it makes a root of its own, holding `folder`: (its root, the
        rest of its stdout, the run)."""
        env = {**os.environ, scratch.VAR: ""}
        code = BOOT + "folder = scratch.make('unityrs-own-')\nroot = scratch._root\nprint(root, flush=True)\n" + last
        proc = child.run([PY, "-c", code], env=env, capture_output=True, text=True)
        root, *rest = proc.stdout.split("\n")
        self.addCleanup(scratch.remove_tree, root)  # left there, on this test's failure
        return Path(root), rest, proc

    def test_a_process_that_dies_with_no_parent_folder_has_its_root_swept(self):
        root, _, proc = self.own_root_child("os.abort()")
        self.assertEqual(proc.returncode, -signal.SIGABRT)
        self.assertRegex(root.name, r"^unityrs-test-[0-9]+-[0-9]+$")
        self.assertEqual(root.parent, Path(scratch.tempfile.gettempdir()))
        scratch.sweep()
        self.assertFalse(root.exists(), "a dead process's root outlived a sweep")

    def test_the_root_goes_with_its_last_folder(self):
        root, rest, proc = self.own_root_child(
            "other = scratch.make()\nscratch.remove(folder)\nprint(root.exists())\n"
            "scratch.remove(other)\nprint(root.exists())\n"
        )
        self.assertEqual(rest, ["True", "False", ""], proc.stderr)
        self.assertFalse(root.exists())

    def test_a_failing_case_or_class_leaves_nothing(self):
        """A case's folder goes with its cleanups and a class's with its class cleanups, which unittest runs
        when the test or setUpClass fails (review round 9: tearDownClass never runs after a failing
        setUpClass, and test_g3b's folder stayed)."""
        module = (
            "import unittest\n"
            "class Case(unittest.TestCase):\n"
            "    def test_fails(self):\n"
            "        print(scratch.for_case(self), flush=True)\n"
            "        self.fail('as designed')\n"
            "class Class(unittest.TestCase):\n"
            "    @classmethod\n"
            "    def setUpClass(cls):\n"
            "        print(scratch.for_class(cls), flush=True)\n"
            "        raise AssertionError('as designed')\n"
            "    def test_never(self):\n"
            "        pass\n"
            "unittest.main(argv=['x'])\n"
        )
        root, rest, proc = self.own_root_child("scratch.remove(folder)\n" + module)
        self.assertEqual(proc.returncode, 1, proc.stderr)
        folders = [Path(p) for p in rest[:2]]
        self.assertEqual(len(rest), 3, proc.stdout)
        for folder in folders:
            self.assertTrue(str(folder).startswith(str(root)), folder)
            self.assertFalse(folder.exists(), folder)
        self.assertFalse(root.exists())

    def test_remove_tree_takes_a_chain_past_the_recursion_limit_and_a_closed_folder(self):
        base = scratch.for_case(self, "unityrs-deep-")
        deepest = str(base)
        for _ in range(sys.getrecursionlimit() + 100):
            deepest = os.path.join(deepest, "d")
            os.mkdir(deepest)
        os.chmod(deepest, 0)
        scratch.remove_tree(base)
        self.assertFalse(base.exists())


# ---------------------------------------------------------------- the scans

SKIP = {".git", "target", "__pycache__", "node_modules", ".venv"}
# UnityPy's own files at git 417d595, kept byte for byte (head_oracle.py pins their blob ids): not ours to route.
VERBATIM = "tools/parity/tests/head_417d595"
STARTS = {"subprocess", "multiprocessing", "pty", "concurrent.futures.process"}
OS_STARTS = {"fork", "forkpty", "system", "popen", "posix_spawn", "posix_spawnp"}
OS_STARTS |= {f"{w}{s}" for w in ("spawn", "exec") for s in ("l", "le", "lp", "lpe", "v", "ve", "vp", "vpe")}
SCRATCH = {"mkdtemp", "mkstemp", "mktemp", "TemporaryDirectory", "NamedTemporaryFile", "gettempdir", "gettempdirb"}
# Each a file that keeps a rule its own way, and why. A stale entry fails too.
OWN_CHILDREN = {
    "tools/parity/child.py": "the rule itself",
    "tools/parity/unitypy_dump.py": (
        "the oracle's process per input: a death by any signal, SIGKILL included, is that input's crash record "
        "(CANONICAL_DUMP.md 4.2), never an agreement: compare.py reports it as a difference (G1), and g3b.py "
        "runs the input again through child.run_all"
    ),
    "crates/unityrs-bench-lzma/unitypy_lzma.py": (
        "the LZMA benchmark's pool of decoders, which is what it times; run by hand, never a test or a gate"
    ),
    "tools/perf/bench_unitypy.py": (
        "UnityPyBoost's side of the perf bench: its fork pool of readers is what it times, as the spike's was; "
        "perf.py runs the bench itself through child.run"
    ),
}
OWN_SCRATCH = {
    "tools/parity/scratch.py": "the rule itself",
    "tools/parity/fetch_unitypy_samples.py": (
        "a download's partial file beside its destination, renamed into place or removed on failure: the "
        "tool's output, not scratch"
    ),
}


def python_files(root: Path):
    """Every Python file under `root`: each `.py`, and each script with a python `#!` line."""
    for here, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP and not os.path.join(here, d).endswith(VERBATIM))
        for name in sorted(files):
            path = Path(here, name)
            if name.endswith(".py"):
                yield path
            elif "." not in name:
                with open(path, "rb") as fh:
                    first = fh.readline(200)
                if first.startswith(b"#!") and b"python" in first:
                    yield path


def found(root: Path, modules, os_names, attrs):
    """Each `path:line: what` under `root` that imports one of `modules` (or a name of `os_names` from os, of
    `attrs` from tempfile), names `os.<one of os_names>` or `tempfile.<one of attrs>`, or imports a module by
    a string. Read from the syntax tree, so comments and strings never count."""
    out = []
    for path in python_files(root):
        rel = path.relative_to(root).as_posix()
        tree = ast.parse(path.read_bytes(), filename=rel)
        for node in ast.walk(tree):
            what = None
            if isinstance(node, ast.Import):
                what = next((a.name for a in node.names if a.name.split(".")[0] in modules or a.name in modules), None)
            elif isinstance(node, ast.ImportFrom) and node.module:
                names = {a.name for a in node.names}
                if node.module.split(".")[0] in modules or node.module in modules:
                    what = f"from {node.module}"
                elif node.module == "os" and names & os_names or node.module == "tempfile" and names & attrs:
                    what = f"from {node.module} import {', '.join(sorted(names))}"
                elif node.module == "concurrent.futures" and "ProcessPoolExecutor" in names and modules:
                    what = "ProcessPoolExecutor"
            elif isinstance(node, ast.Attribute) and modules and node.attr == "ProcessPoolExecutor":
                what = "ProcessPoolExecutor"
            elif isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
                if (node.value.id, node.attr) in {("os", n) for n in os_names} | {("tempfile", n) for n in attrs}:
                    what = f"{node.value.id}.{node.attr}"
            elif isinstance(node, ast.Call) and isinstance(node.func, (ast.Name, ast.Attribute)):
                name = node.func.id if isinstance(node.func, ast.Name) else node.func.attr
                arg = node.args[0] if node.args else None
                if name in ("__import__", "import_module") and isinstance(arg, ast.Constant):
                    if str(arg.value).split(".")[0] in modules or arg.value in modules:
                        what = f"{name}({arg.value!r})"
            if what:
                out.append(f"{rel}:{node.lineno}: {what}")
    return sorted(set(out), key=lambda s: (s.split(":")[0], int(s.split(":")[1])))


def child_runs(root):
    return found(root, STARTS, OS_STARTS, set())


def scratch_made(root):
    return found(root, set(), set(), SCRATCH)


class ScanTests(unittest.TestCase):
    def check(self, hits, exempt, rule):
        files = {h.split(":")[0] for h in hits}
        self.assertEqual([h for h in hits if h.split(":")[0] not in exempt], [], rule)
        self.assertEqual(sorted(set(exempt) - files), [], "an exemption no file needs any more")

    def test_every_child_in_the_repository_runs_through_child_py(self):
        rule = "children run other than through tools/parity/child.py, which a SIGKILL from outside misleads"
        self.check(child_runs(ROOT), OWN_CHILDREN, rule)

    def test_every_scratch_folder_in_the_repository_is_made_by_scratch_py(self):
        rule = "scratch made other than through tools/parity/scratch.py, which a failure or a dying child leaves"
        self.check(scratch_made(ROOT), OWN_SCRATCH, rule)

    def test_the_scans_find_each_direct_form(self):
        root = scratch.for_case(self, "unityrs-scan-")
        files = {
            "clean.py": (
                "import child, scratch, tempfile\n# import subprocess\n'subprocess.run(x)'\n"
                "child.run(['sh'])\nscratch.make()\ntempfile.TemporaryFile()\n"
            ),
            "imports.py": "import subprocess\nfrom multiprocessing import Pool\nimport pty, os.path\n",
            "calls.py": "import os\nos.fork()\nos.system('x')\npid = os.posix_spawn\n__import__('subprocess')\n",
            "pool.py": "from concurrent.futures import ProcessPoolExecutor\n",
            "temps.py": "import tempfile\ntempfile.mkdtemp()\nfrom tempfile import TemporaryDirectory\n",
            "script": "#!/usr/bin/env python3\nimport subprocess\n",
            "notes": "#!/bin/sh\nimport subprocess\n",
        }
        for name, text in files.items():
            (root / name).write_text(text)
        self.assertEqual(
            child_runs(root),
            [
                "calls.py:2: os.fork",
                "calls.py:3: os.system",
                "calls.py:4: os.posix_spawn",
                "calls.py:5: __import__('subprocess')",
                "imports.py:1: subprocess",
                "imports.py:2: from multiprocessing",
                "imports.py:3: pty",
                "pool.py:1: ProcessPoolExecutor",
                "script:2: subprocess",
            ],
        )
        self.assertEqual(
            scratch_made(root),
            ["temps.py:2: tempfile.mkdtemp", "temps.py:3: from tempfile import TemporaryDirectory"],
        )


if __name__ == "__main__":
    unittest.main()
