#!/usr/bin/env python3
"""weapons.py -- catalogue the weapon models and place them in a latent space.

    ./tools/weapons.py              # catalogue + clusters
    ./tools/weapons.py --json out.json

THE SHAPE OF THE PROBLEM is the same one the bots had and the answer is the
same shape too: the names carry real information, the geometry carries more,
and neither should be trusted alone.

Where this differs from the bot roster: a weapon's STATS are not free
invention. Reach, damage and rate trade against each other, and the trade is
what makes a weapon feel like a choice rather than a number. So every entry
here is generated from measurable or nameable properties and then CHECKED —
no weapon may be best at everything, and a test asserts it.

Six axes, which is what the clustering runs on:

  reach      0 melee .. 1 sniper
  damage     per hit, relative
  rate       shots or swings per second, relative
  charge     0 instant .. 1 must be wound up
  spread     0 laser .. 1 shotgun
  exotic     0 kinetic .. 1 does something the others do not

The Destiny-shaped idea you raised lands on the last axis: an exotic is not
a bigger number, it is a weapon that breaks one of the trades. Those are
placed deliberately and marked, not sprinkled.
"""
import json
import math
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "..", "..", "blackstar", "engine_src", "data")

# damage types, in the game's own elemental vocabulary (content::ELEMENTS)
KINETIC, ENERGY, PLASMA, ION, THERMAL, CRYO, SONIC = (
    "kinetic", "energy", "plasma", "ion", "thermal", "cryo", "sonic")

# name fragment -> (family, reach, damage, rate, charge, spread, exotic, dtype, projectile)
RULES = [
    # ---- melee, and the energy blades you asked for ----
    (r"lsbr|lightsaber|energy.*sword|plasma.*blade",
     ("energy blade", 0.04, 0.80, 1.30, 0.0, 0.0, 0.35, PLASMA, "none")),
    (r"double_lightsaber|double.*blade|twin.*blade",
     ("twin blade", 0.06, 0.70, 1.80, 0.0, 0.0, 0.55, PLASMA, "none")),
    (r"sword|sabre|katana|blade|slasher",
     ("blade", 0.05, 0.90, 1.00, 0.0, 0.0, 0.10, KINETIC, "none")),
    (r"hammer|maul|mace",
     ("hammer", 0.05, 1.60, 0.45, 0.35, 0.20, 0.40, KINETIC, "shock")),
    (r"axe|cleaver",  ("axe", 0.05, 1.30, 0.60, 0.15, 0.0, 0.15, KINETIC, "none")),
    (r"spear|lance|pike|halberd",
     ("polearm", 0.12, 1.10, 0.70, 0.0, 0.0, 0.20, KINETIC, "none")),
    (r"claw|talon|ripper",
     ("claws", 0.03, 0.55, 2.20, 0.0, 0.0, 0.25, KINETIC, "none")),
    # ---- ranged ----
    (r"rail|gauss|slug",
     ("railgun", 0.95, 1.80, 0.30, 0.75, 0.0, 0.45, ION, "hitscan")),
    (r"sniper|marksman|long",
     ("marksman", 0.90, 1.50, 0.35, 0.30, 0.0, 0.10, KINETIC, "bolt")),
    (r"cannon",   ("cannon", 0.60, 1.55, 0.35, 0.20, 0.15, 0.25, THERMAL, "shell")),
    (r"launcher|missile|rocket",
     ("launcher", 0.70, 1.70, 0.25, 0.25, 0.10, 0.40, THERMAL, "missile")),
    (r"plasma",   ("plasma gun", 0.55, 1.20, 0.60, 0.15, 0.10, 0.55, PLASMA, "orb")),
    (r"blaster|pistol|dc_17",
     ("blaster", 0.45, 0.70, 1.40, 0.0, 0.12, 0.15, ENERGY, "bolt")),
    (r"rifle|carbine",
     ("rifle", 0.70, 0.85, 1.10, 0.0, 0.08, 0.10, KINETIC, "bolt")),
    (r"shotgun|scatter|flak",
     ("scattergun", 0.20, 1.40, 0.55, 0.0, 0.85, 0.20, KINETIC, "pellets")),
    (r"beam|laser",
     ("beam", 0.75, 0.60, 3.00, 0.10, 0.0, 0.50, ENERGY, "beam")),
    (r"acid|corros",
     ("acid projector", 0.35, 0.90, 0.80, 0.0, 0.45, 0.75, PLASMA, "spray")),
    (r"cryo|frost|ice",
     ("cryo", 0.40, 0.60, 0.90, 0.10, 0.35, 0.70, CRYO, "spray")),
    (r"sonic|resonan|shriek",
     ("sonic", 0.30, 0.50, 1.00, 0.20, 0.60, 0.80, SONIC, "wave")),
    # ---- not weapons in the damage sense ----
    (r"shield|barrier|bulwark",
     ("shield", 0.0, 0.0, 0.0, 0.30, 0.0, 0.60, ENERGY, "none")),
    (r"energy_ball|orb|crystal|aether",
     ("focus", 0.50, 1.00, 0.50, 0.60, 0.20, 0.90, ENERGY, "orb")),
]

AXES = ["reach", "damage", "rate", "charge", "spread", "exotic"]


def classify(name):
    n = name.lower()
    for pat, spec in RULES:
        if re.search(pat, n):
            fam, *vals, dtype, proj = spec
            return fam, dict(zip(AXES, vals)), dtype, proj
    return None


def catalogue():
    out = []
    for f in sorted(os.listdir(DATA)):
        if not f.endswith(".iqm"):
            continue
        stem = f.replace("imported_", "").replace(".iqm", "")
        hit = classify(stem)
        if not hit:
            continue
        fam, axes, dtype, proj = hit
        # a weapon carried BY a bot is not a pickup; the filename says which
        carried = bool(re.match(r"(flyer|tank|hoverer|car|racer|moto|heli|raidboss|spaceship|grimorph|dragonflyer)_", stem))
        out.append({
            "id": stem, "family": fam, "damage_type": dtype, "projectile": proj,
            "carried_by_chassis": carried, **axes,
        })
    return out


def check_no_weapon_is_best_at_everything(ws):
    """The trade IS the design. A weapon that leads on reach, damage and rate
    at once is not exotic, it is unbalanced, and it would quietly become the
    only one anybody uses."""
    bad = []
    for w in ws:
        lead = sum(1 for a in ("reach", "damage", "rate")
                   if w[a] >= max(x[a] for x in ws) - 1e-9)
        if lead >= 3:
            bad.append(w["id"])
    return bad


def kmeans(X, k, iters=30, seed=1):
    import random
    rnd = random.Random(seed)
    C = [X[rnd.randrange(len(X))][:] for _ in range(k)]
    lab = [0] * len(X)
    for _ in range(iters):
        for i, p in enumerate(X):
            lab[i] = min(range(k), key=lambda j: sum((a - b) ** 2 for a, b in zip(p, C[j])))
        for j in range(k):
            mem = [X[i] for i in range(len(X)) if lab[i] == j]
            if mem:
                C[j] = [sum(v[d] for v in mem) / len(mem) for d in range(len(X[0]))]
    return lab, C


def main():
    ws = catalogue()
    print(f"{len(ws)} weapon models catalogued\n")
    import collections
    print("by family:", dict(collections.Counter(w["family"] for w in ws).most_common()))
    print("by damage type:", dict(collections.Counter(w["damage_type"] for w in ws).most_common()))
    print("by projectile:", dict(collections.Counter(w["projectile"] for w in ws).most_common()))
    bad = check_no_weapon_is_best_at_everything(ws)
    print(f"\nweapons leading on reach AND damage AND rate: {bad or 'none'}")

    X = [[w[a] for a in AXES] for w in ws]
    if len(X) >= 6:
        lab, C = kmeans(X, min(6, len(X)))
        print("\nclusters in the six-axis space:")
        for j in range(len(C)):
            mem = [ws[i]["family"] for i in range(len(ws)) if lab[i] == j]
            if not mem:
                continue
            c = C[j]
            print(f"  reach{c[0]:.2f} dmg{c[1]:.2f} rate{c[2]:.2f} chg{c[3]:.2f} "
                  f"sprd{c[4]:.2f} exo{c[5]:.2f}  n={len(mem)}  "
                  f"{dict(collections.Counter(mem).most_common(3))}")
        for i, w in enumerate(ws):
            w["cluster"] = lab[i]
    if "--json" in sys.argv:
        p = sys.argv[sys.argv.index("--json") + 1]
        json.dump({"_doc": [
            "Weapon catalogue and its six-axis latent space.",
            "Axes: reach, damage, rate, charge, spread, exotic.",
            "Stats are generated from name rules and CHECKED — no weapon may",
            "lead on reach, damage and rate at once, because the trade is the",
            "design. An exotic breaks ONE trade deliberately, not all three.",
        ], "weapons": ws}, open(p, "w"), indent=1)
        print(f"\n-> {p}")


if __name__ == "__main__":
    main()
