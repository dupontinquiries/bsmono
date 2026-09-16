#!/usr/bin/env python3
"""tuning.py — emit a model_tuning.json for the forge roster, and check it
with blackstar's own rules.

test_tuning.py enforces four things on that file:
  * every installed model has an entry with a kind in
    {prop, robot, vehicle, weapon, env, ship}
  * `pair` is SYMMETRIC: pairs[a] == b implies pairs[b] == a
  * a pair CROSSES kinds: {kind(a), kind(b)} == {vehicle, robot}
  * every pair target exists

Generating both sides from fold.PAIRS is what makes symmetry structural
rather than something to re-check — an authored pair list drifts the first
time a mesh is renamed, which is PIPELINE.md's recorded failure mode.
"""
import json, sys
from bots import CHASSIS
from fleet import ALL
from fold import PAIRS

KIND = {"car": "vehicle", "hoverer": "vehicle", "flyer": "vehicle",
        "spaceship": "ship"}


def table():
    t = {}
    for s in ALL:
        t[s.name] = {"kind": KIND[s.cls], "size": 1.0, "role": s.role}
    for c in CHASSIS:
        t[c.name] = {"kind": "robot", "size": 1.0, "role": c.role}
    # both directions from one source
    for chassis, (vehicle, why) in PAIRS.items():
        t[chassis]["pair"] = vehicle
        t[vehicle]["pair"] = chassis
        t[chassis]["pair_why"] = why
    return t


# ── blackstar's own checks, lifted from tools/test_tuning.py ──────────────
KINDS = {"prop", "robot", "vehicle", "weapon", "env", "ship"}


def check(t):
    bad = []
    for n, e in t.items():
        if e.get("kind") not in KINDS:
            bad.append(f"MISSING/BAD kind: {n}")
    pairs = {n: e["pair"] for n, e in t.items() if "pair" in e}
    for a, b in pairs.items():
        if b not in t:
            bad.append(f"PAIR TARGET MISSING: {a} -> {b}"); continue
        if pairs.get(b) != a:
            bad.append(f"PAIR ASYMMETRIC: {a} -> {b} -> {pairs.get(b)}")
        ka, kb = t[a].get("kind"), t[b].get("kind")
        if {ka, kb} != {"vehicle", "robot"}:
            bad.append(f"PAIR KINDS WRONG: {a}({ka}) <-> {b}({kb})")
    return bad, pairs


if __name__ == "__main__":
    t = table()
    bad, pairs = check(t)
    from collections import Counter
    print(f"{len(t)} entries, kinds {dict(Counter(e['kind'] for e in t.values()))}")
    print(f"{len(pairs)//2} pairs\n")
    for a, b in sorted(pairs.items()):
        if t[a]["kind"] == "robot":
            print(f"  {a:<11} <-> {b:<9}  {t[a].get('pair_why','')[:62]}")
    print()
    if bad:
        for e in bad: print("FAIL", e)
        sys.exit(1)
    print("OK: kinds valid, pairs symmetric, every pair crosses robot<->vehicle")
    json.dump(t, open("model_tuning_forge.json", "w"), indent=1)
