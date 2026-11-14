"""Reproduce synthetic batch pins from independent, checked-in oracle facts."""

import argparse
import os

import edge_cases
import oracle_cases
import rule_cases
import texture_case
import unit_case
from model import CASES, REPO, encoded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="compare without writing")
    args = parser.parse_args()
    os.chdir(REPO)
    for generator in [oracle_cases, texture_case, rule_cases, unit_case, edge_cases]:
        generator.generate()
    root = REPO / "tests/specs/v1"
    different = []
    for name, (case, ensure_ascii) in sorted(CASES.items()):
        path = root / f"{name}.json"
        data = encoded(case, ensure_ascii)
        if args.check:
            if not path.exists() or path.read_bytes() != data:
                different.append(path.relative_to(REPO))
        else:
            path.write_bytes(data)
    extra = {p.stem for p in root.glob("*.json")} - CASES.keys()
    if extra:
        different.extend(
            root.relative_to(REPO) / f"{name}.json" for name in sorted(extra)
        )
    if different:
        for path in different:
            print(f"golden differs: {path}")
        return 1
    print(
        f"{'Checked' if args.check else 'Generated'} {len(CASES)} synthetic batch goldens"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
