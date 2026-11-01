"""Child processes: the rule every Python test and tool in this repository keeps when it runs one.

A child killed by SIGKILL did not end itself. Nothing a program does raises SIGKILL but a `kill` of it, and a
caller that runs a child to its end sends none, so the OOM killer (or a person) ended it from outside, whatever
the child would have come to. That run says nothing about the code under test, but the child is the same on the
next run, so `run`, `forked` and `run_all` run it again, up to `KILLED_RUNS` runs in all, and a child killed on
every one of them fails, saying the system killed it (`SystemKilled`). Every other ending is the child's own and
comes back at once, after one run: an exit status, an abort (SIGABRT), a segfault, any other signal. A child the
caller kills itself (a `timeout`, which `subprocess.run` ends with SIGKILL and reports as `TimeoutExpired`;
`run_all`'s `stop`) is the caller's doing, never run again.

Each run of a child also gets a scratch folder its parent owns and removes when the run ends, however it ends,
named in its `UNITYRS_TEST_SCRATCH` (`scratch.py`).

`unityrs_testkit::child` keeps the same rule for the Rust tests. Every Python file in the repository runs its
children through here: `tests/test_child.py` scans for `subprocess`, `os.fork` and the other ways to start a
process, and lists the two that do not, each with its reason. Standard library only.
"""

from __future__ import annotations

import os
import signal
import subprocess
import time
from dataclasses import dataclass

import scratch

# What callers pass and catch, so that no file but this one imports subprocess.
from subprocess import DEVNULL, PIPE, STDOUT, CalledProcessError, CompletedProcess, TimeoutExpired  # noqa: E402, F401

KILLED_RUNS = 4


class SystemKilled(AssertionError):
    """A child SIGKILL ended on each of its `KILLED_RUNS` runs: the system's doing, not the code under test's."""


def killed(returncode: int | None) -> bool:
    """Whether a child that ended with `returncode` (as `subprocess` gives it) was killed by SIGKILL."""
    return returncode == -signal.SIGKILL


def _system_killed(what, tail: str = "") -> SystemKilled:
    return SystemKilled(
        f"{what!r}: killed by SIGKILL on each of its {KILLED_RUNS} runs: the system ended it from outside "
        f"(the OOM killer, or a person), not the code under test, so it came to no result{tail}"
    )


def _tail(data) -> str:
    if not data:
        return ""
    text = data if isinstance(data, str) else data.decode("utf-8", "replace")
    return "\n".join(text.splitlines()[-20:])


def _env(kwargs: dict, folder) -> dict:
    """The child's environment: the caller's, with the run's own scratch folder, unless the caller named one
    (`scratch.VAR`), or none (an empty one, which the child does not get)."""
    given = kwargs.get("env")
    env = dict(os.environ if given is None else given)
    if given is None or scratch.VAR not in given:
        env[scratch.VAR] = str(folder)
    elif not given[scratch.VAR]:
        del env[scratch.VAR]
    return env


def run(args, *, check: bool = False, **kwargs) -> subprocess.CompletedProcess:
    """`subprocess.run(args, **kwargs)`, run again while SIGKILL from outside ends it, up to `KILLED_RUNS` runs in
    all (module docs). Every other outcome is the first run's, returned as `subprocess.run` returns it, and with
    `check` a non-zero status raises `CalledProcessError` as there.

    The streams must be ones a second run can use again: `input=`, `capture_output`, PIPE, DEVNULL, STDOUT or
    the caller's own; an open file would hand the second run what the first left.

    Raises `SystemKilled` when SIGKILL ends the child on every run, with the last run's last lines of output.
    """
    for name in ("stdin", "stdout", "stderr"):
        stream = kwargs.get(name)
        if stream is not None and not (isinstance(stream, int) and stream < 0):
            raise TypeError(f"child.run: {name}={stream!r}: a run again would reuse what the killed run left in it")
    for _ in range(KILLED_RUNS):
        with scratch.folder("unityrs-child-") as own:
            proc = subprocess.run(args, check=False, **{**kwargs, "env": _env(kwargs, own)})
        if not killed(proc.returncode):
            if check:
                proc.check_returncode()
            return proc
    raise _system_killed(args, f"\n--- stdout\n{_tail(proc.stdout)}\n--- stderr\n{_tail(proc.stderr)}")


def forked(fn) -> int:
    """`fn()` in a forked child, which then leaves with `os._exit(0)` (running none of this process's
    cleanups), run again while SIGKILL from outside ends it (module docs): the child's wait status, as
    `os.waitpid` gives it."""
    for _ in range(KILLED_RUNS):
        pid = os.fork()
        if pid == 0:  # the child
            try:
                fn()
            finally:
                os._exit(0)
        status = os.waitpid(pid, 0)[1]
        if not (os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL):
            return status
    raise _system_killed(fn)


@dataclass
class Ending:
    """How a child of `run_all` ended: its return code (negative for a signal), and whether the caller stopped
    it (`stop`)."""

    returncode: int
    stopped: bool = False


def run_all(jobs, workers: int, *, stop=None, preexec_fn=None) -> dict:
    """Runs each `(key, argv, out)` with stdout to the file `out` and stderr to `out` + ".err", at most `workers`
    at once, each run again while SIGKILL from outside ends it (module docs): `{key: Ending}`. `stop(proc)`,
    asked while a child runs, says when the caller kills it: that kill is the caller's, and its run the result.

    Raises `SystemKilled` when a child is killed from outside on every run.
    """
    todo, running, results = [(key, argv, out, 1) for key, argv, out in reversed(jobs)], {}, {}
    try:
        while todo or running:
            while todo and len(running) < workers:
                key, argv, out, runs = todo.pop()
                own = scratch.make("unityrs-child-")
                running[key] = [None, False, argv, out, runs, own]
                env = {**os.environ, scratch.VAR: str(own)}
                with open(out, "wb") as fh, open(f"{out}.err", "wb") as err:
                    running[key][0] = subprocess.Popen(
                        argv, stdin=subprocess.DEVNULL, stdout=fh, stderr=err, env=env, preexec_fn=preexec_fn
                    )
            for key, entry in list(running.items()):
                proc, stopped, argv, out, runs, own = entry
                code = proc.poll()
                if code is None:
                    if not stopped and stop is not None and stop(proc):
                        proc.kill()
                        entry[1] = True
                    continue
                del running[key]
                scratch.remove(own)
                if killed(code) and not stopped:
                    if runs == KILLED_RUNS:
                        raise _system_killed(argv)
                    todo.append((key, argv, out, runs + 1))
                    continue
                results[key] = Ending(code, stopped)
            if running:
                time.sleep(0.005)
    finally:  # a raise (here, or the caller's `stop`) leaves no child running and no folder behind
        for proc, _, _, _, _, own in running.values():
            if proc is not None:
                proc.kill()
                proc.wait()
            scratch.remove(own)
    return results
