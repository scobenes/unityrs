"""Reproduce synthetic batch pins from independent, checked-in oracle facts."""

import argparse
import os

import archive_case
import edge_cases
import oracle_cases
import rule_cases
import texture_case
import unit_case
import v2_cases
from model import CASES, REPO, encoded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="compare without writing")
    args = parser.parse_args()
    os.chdir(REPO)
    for generator in [
        oracle_cases,
        texture_case,
        rule_cases,
        unit_case,
        edge_cases,
        archive_case,
        v2_cases,
    ]:
        generator.generate()
    specs = REPO / "tests/specs"
    different = []
    expected = set()
    for name, case in sorted(CASES.items()):
        # Each pin lives under its spec's version: tests/specs/v<N>/.
        path = specs / f"v{case['spec']['spec_version']}" / f"{name}.json"
        expected.add(path)
        data = encoded(case)
        if args.check:
            if not path.exists() or path.read_bytes() != data:
                different.append(path.relative_to(REPO))
        else:
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(data)
    extra = set(specs.glob("v*/*.json")) - expected
    different.extend(p.relative_to(REPO) for p in sorted(extra))
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
