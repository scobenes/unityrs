"""The winners across inputs of the NGUI cutter's reference: docs/BATCH_SPEC.md 4.5's steps 1 to 3 (E31, E32, E61).

Written from 4.5, E31, E32 and E61, as `ngui_cutter.py` is from section 5 (docs/plans/extract.md section 3, item
3): "per-atlas collisions and the unit winner" are this module's. Its input is the cutter's `Result` of each input
that ran (a failed input commits no candidate, E60), the spec's `prefer` map and the inputs that failed; its output
is each key's `Decision`, with every candidate and the rule 7.3's `collisions.rule` names for it. Step 4 (4.6's
prefix conflicts) is not modelled.

Where 4.5 gives no answer the module takes the most literal reading:
- a unit E61 withholds takes part in step 2 as a winning unit (its keys "are reserved in step 2 as a winning
  unit's are");
- a key's flip is a rule-3 decision whose candidates, those of the unit that decided it for a unit's key, do not
  all have one MD5;
- a candidate from a unit that lost step 1 keeps that unit's rule and is no winner; a key only such candidates
  hold has no rule that chooses one (`status` `no_rule`), and one that plain candidates hold too is decided among
  the plain ones (step 3's "every other key").
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass


def bytewise(text: str) -> bytes:
    return text.encode("utf-8", "surrogateescape")


def md5(data: bytes) -> str:
    return hashlib.md5(data).hexdigest()


@dataclass(frozen=True)
class Candidate:
    input: str
    md5: str
    unit: str | None
    rule: str
    winner: bool


@dataclass(frozen=True)
class Decision:
    """A key's outcome: `publish`, `withheld` (`reason` `withheld`, `unit_conflict` or `publish_incomplete`) or
    `no_rule`; whether a flip decided it; and its candidates in bytewise order of input."""

    key: str
    status: str
    reason: str | None
    flip: bool
    candidates: tuple[Candidate, ...]

    @property
    def winner(self) -> str | None:
        return next((c.input for c in self.candidates if c.winner), None)


@dataclass(frozen=True)
class Choice:
    winner: str
    rule: str
    flip: bool
    withheld: bool


def choose(key: str, candidates: list[tuple[str, str]], prefer: dict, failed: set) -> Choice:
    """E31 among `candidates`, (input, MD5) in bytewise order of input, and E61: the key is withheld when its
    `prefer` entry names an input that failed; its rule-2 or rule-3 winner is still the candidate the rules chose
    (7.3's `outputs`)."""
    entry = prefer.get(key) or {}
    want_input, want_md5 = entry.get("input"), entry.get("md5")
    inputs = [i for i, _ in candidates]
    withheld = want_input is not None and want_input in failed
    if want_input in inputs:
        return Choice(want_input, "prefer_input", False, withheld)
    matching = [i for i, m in candidates if want_md5 is not None and m == want_md5]
    if matching:
        return Choice(matching[0], "prefer_md5", False, withheld)
    return Choice(inputs[0], "first_input", len({m for _, m in candidates}) > 1, withheld)


def winners(results, prefer: dict | None = None, failed=(), publish_incomplete: bool = True) -> dict[str, Decision]:
    """4.5's steps 1 to 3 over the cutter's `Result`s: {key: Decision}, in bytewise order of key. `prefer` maps a
    key to `{"input": id or None, "md5": hex or None}` (1.5's Prefer)."""
    prefer, failed = prefer or {}, set(failed)
    cands: dict[str, list[tuple[str, str, str | None]]] = {}
    members: dict[str, dict[str, set[str]]] = {}
    unit_md5: dict[tuple[str, str], str] = {}
    for r in sorted(results, key=lambda r: bytewise(r.input)):
        for o in r.outputs:
            digest = md5(o.data)
            cands.setdefault(o.key, []).append((r.input, digest, o.unit))
            if o.unit is not None:
                members.setdefault(o.unit, {}).setdefault(r.input, set()).add(o.key)
                if o.key == o.unit:
                    unit_md5[(o.unit, r.input)] = digest
    # Step 1: each unit key's winning unit, by E31 and E61 on the unit key's output.
    step1 = {}
    for u in sorted(members, key=bytewise):
        step1[u] = choose(u, [(i, unit_md5[(u, i)]) for i in sorted(members[u], key=bytewise)], prefer, failed)
    # Step 2: the winning units' keys are reserved (a withheld unit key's every unit's); of the winning units that
    # claim one key, the first by unit key keeps it and every other is withheld whole. It runs once.
    claims: dict[str, list[str]] = {}
    for u, c in step1.items():
        keys = set().union(*members[u].values()) if c.withheld else members[u][c.winner]
        for k in keys:
            claims.setdefault(k, []).append(u)
    keeper = {k: min(us, key=bytewise) for k, us in claims.items()}
    conflicted = {u for k, us in claims.items() for u in us if u != keeper[k]}

    def member_rule(u: str) -> str:
        return "withheld" if step1[u].withheld else "unit_conflict" if u in conflicted else step1[u].rule

    def status(withheld_by: str | None) -> tuple[str, str | None]:
        if withheld_by is not None:
            return "withheld", withheld_by
        if failed and not publish_incomplete:
            return "withheld", "publish_incomplete"
        return "publish", None

    decided = {}
    for k in sorted(cands, key=bytewise):
        cs = cands[k]
        if k in claims:  # a reserved key: its keeper's winner; a plain candidate loses (rule `unit`)
            ku = keeper[k]
            c = step1[ku]
            rows = tuple(
                Candidate(i, m, u, "unit" if u is None else member_rule(u), u == ku and i == c.winner)
                for i, m, u in cs
            )  # fmt: skip
            flip = c.rule == "first_input" and len({m for _, m, u in cs if u == ku}) > 1
            withheld_by = "withheld" if c.withheld else "unit_conflict" if ku in conflicted else None
            decided[k] = Decision(k, *status(withheld_by), flip, rows)
            continue
        plain = [(i, m) for i, m, u in cs if u is None]
        if not plain:  # only candidates of units that lost step 1: 4.5 names no rule that chooses one
            rows = tuple(Candidate(i, m, u, step1[u].rule, False) for i, m, u in cs)
            decided[k] = Decision(k, "no_rule", None, False, rows)
            continue
        c = choose(k, plain, prefer, failed)
        rows = tuple(
            Candidate(i, m, u, ("withheld" if c.withheld else c.rule) if u is None else step1[u].rule,
                      u is None and i == c.winner)
            for i, m, u in cs
        )  # fmt: skip
        decided[k] = Decision(k, *status("withheld" if c.withheld else None), c.flip, rows)
    return decided


def report(decisions: dict[str, Decision]) -> list[dict]:
    """The decisions as JSON rows, in bytewise order of key."""
    return [
        {"key": d.key, "status": d.status, "reason": d.reason, "flip": d.flip, "winner": d.winner,
         "candidates": [[c.input, c.unit, c.rule, c.winner] for c in d.candidates]}
        for d in decisions.values()
    ]  # fmt: skip
