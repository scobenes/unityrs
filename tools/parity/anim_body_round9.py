"""Checks, on the recorded round-9 training runs, that the hold-out adequacy prediction
(anim_body_holdout.py) flags round 9's failing hold-out sample from the spec alone, reading no
hold-out file.

The recordings are local Editor runs, not in the repository, so this is a manual check, not a
test: CI's parity suite must run every test it holds.

    python3 anim_body_round9.py --runs RUN_2022 RUN_6000 --unityrs PATH_TO_THE_CLI_V7_PINS
"""

import argparse
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import anim_body_freeze as freeze  # noqa: E402
import anim_body_holdout as holdout  # noqa: E402
import anim_body_seal as seal  # noqa: E402
from anim_fixture_body import HOLDOUT_RIGS  # noqa: E402

FAILING = 0.651947021484375


def check(folders, unityrs):
    """The problems found: none when the prediction flags the failing sample in every version."""
    artifact = json.loads((seal.HERE / "anim_body_freeze_v7.json").read_text(encoding="utf-8"))
    avatar_of, clip_of = freeze.readers(unityrs)
    runs, _, design = freeze.frozen_training(folders, artifact, avatar_of, clip_of)
    found = holdout.predicted(runs, design, artifact["coefficients"], avatar_of, rig=HOLDOUT_RIGS[0])
    problems = []
    if sorted(found) != artifact["scope"]["editor_versions"]:
        problems.append(f"versions {sorted(found)}, not {artifact['scope']['editor_versions']}")
    for version, entry in sorted(found.items()):
        samples = [row for row in entry["samples"] if row["local"] == FAILING]
        if len(samples) != 1:
            problems.append(f"{version}: {len(samples)} samples at {FAILING}")
            continue
        if not samples[0]["margin"] < 1:
            problems.append(f"{version}: margin {samples[0]['margin']} at {FAILING}, not under 1")
        if not any(f"@{FAILING}:" in line for line in entry["failures"]):
            problems.append(f"{version}: no failure names @{FAILING}")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", nargs="+", type=pathlib.Path, required=True)
    parser.add_argument("--unityrs", required=True)
    args = parser.parse_args()
    problems = check(args.runs, args.unityrs)
    for problem in problems:
        print(problem, file=sys.stderr)
    print("FAIL" if problems else "OK: the prediction flags round 9's failing sample in every version")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
