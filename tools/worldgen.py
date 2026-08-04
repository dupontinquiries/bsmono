#!/usr/bin/env python3
"""worldgen.py -- deep-space patrol worlds, and the bots that belong on them.

    ./tools/worldgen.py 7            # one world from seed 7
    ./tools/worldgen.py 1 2 3 --json out.json

THE IDEA, which is the overlap you asked for: a world's PHYSICS constrains
which bots plausibly come from it, so a generated world is not decoration
around a random roster — it is a filter that the roster is drawn through.

  no atmosphere       a wing produces nothing. Thrust or rotor-in-a-can only.
  thin air            a rotor loses its bite; helicopters have a ceiling.
  high gravity        reach is expensive (inertia goes as r^2), so short
                      limbs and treads; tall spindly bodies do not survive.
  low gravity         the opposite — long reach is cheap, and traction is
                      not, so wheels lose to thrust and legs.
  no ground           nothing with wheels at all.

"Outlandish but believable" is then a precise thing rather than a mood: keep
every hard constraint, and sample the SOFT preferences further from their
centre. A hexapod on a 2.4 g world is strange and survivable; a wing-flyer on
an airless one is neither, and no amount of flavour makes it read.

Nothing here invents geometry. It selects and places EXISTING chassis, which
is what stops the result being mesh soup — every bot on a generated world is
a model somebody made, standing somewhere its physics allows.
"""
import json
import math
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

# --- the physical envelope, shared with src/terrain.rs and src/lift.rs -----
STALL_MS = 26.0          # a wing gives nothing below this
ROTOR_G = 1.55           # rotor lift at full throttle in one atmosphere
THRUST_G = 1.40          # reaction thrust, air or no air


def rng(seed):
    # WARM IT UP. A raw xorshift seeded with a small integer correlates badly
    # for the first several draws — seeds 3, 11 and 42 all produced heavy
    # worlds, which looked like a biased distribution and was actually a cold
    # generator. Mixing the seed and discarding the first outputs fixes it.
    s = (seed * 2654435761 + 0x9E3779B9) & 0xFFFFFFFF
    s ^= (s >> 16)
    s = (s * 0x45D9F3B) & 0xFFFFFFFF
    s ^= (s >> 16)
    if s == 0:
        s = 0x1234567

    def nxt():
        nonlocal s
        s ^= (s << 13) & 0xFFFFFFFF
        s ^= s >> 17
        s ^= (s << 5) & 0xFFFFFFFF
        return s / 0xFFFFFFFF
    for _ in range(8):
        nxt()
    return nxt


def make_world(seed):
    """A destination, generated inside the ranges the solar system already
    spans. Not uniform: the interesting worlds are the extremes, so gravity
    is drawn log-uniform rather than flat, which is also how real bodies are
    distributed."""
    r = rng(seed * 2654435761 + 1)
    g = 10 ** (r() * 1.9 - 1.1)            # 0.08 .. 6.3 g
    airless = r() < 0.45
    air = 0.0 if airless else 10 ** (r() * 2.4 - 2.4)   # 0.004 .. 1.0 atm
    terrain = "deep_space" if r() < 0.08 else (
        "asteroid" if r() < 0.18 else ("procedural" if r() < 0.55 else "surface"))
    if terrain in ("deep_space",):
        g, air = 0.0, 0.0
    wind = 0.0 if air <= 0.005 else r() * 4.0
    return {
        "seed": seed,
        "gravity_g": round(g, 3),
        "air_atm": round(air, 4),
        "terrain": terrain,
        "wind_ms": round(wind, 2),
        "has_ground": terrain not in ("deep_space",),
    }


def lift_ok(kind, w):
    """HARD constraint: can this lift type hold its own weight here?"""
    g = w["gravity_g"] * 9.81
    if g <= 0.01:
        return True                       # nothing to hold up
    if kind == "wing":
        # lift needs airspeed AND air; in vacuum a wing is decoration
        return w["air_atm"] > 0.02 and 0.016 * 60 * 60 * w["air_atm"] >= g
    if kind == "rotor":
        return ROTOR_G * g * min(w["air_atm"], 1.0) >= g
    return THRUST_G * g >= g              # thrust: always, it carries reaction mass


def score(bot, w):
    """(hard_ok, soft_score). Soft is 0..1 plausibility, higher is more
    ordinary — so 'outlandish' means picking a LOW soft score that still
    passes hard."""
    cls = bot["class"]
    g = w["gravity_g"]
    wheels = cls in ("car", "racer", "truck", "moto", "tank")
    flies = cls in ("flyer", "heli", "spaceship", "hawk", "dragonflyer")
    hovers = cls in ("hoverer", "hovertank")

    # HARD: things that simply cannot be
    if wheels and not w["has_ground"]:
        return False, 0.0
    if hovers and not w["has_ground"]:
        return False, 0.0
    if cls == "flyer" and not lift_ok("wing", w):
        return False, 0.0
    if cls == "heli" and not lift_ok("rotor", w):
        return False, 0.0
    if flies and cls in ("spaceship",) and not lift_ok("thrust", w):
        return False, 0.0

    s = 0.5
    # traction wants weight; a wheeled bot on a tenth of a g is a novelty
    if wheels:
        s += 0.35 * min(g / 1.0, 1.0) - 0.25 * max(0.0, 1.0 - g)
    # hovering wants something to push against
    if hovers:
        s += 0.3 * min(w["air_atm"] * 4, 1.0)
        if w["air_atm"] < 0.01:
            s -= 0.3
    # reach is expensive under weight: long-limbed plans suffer, treads do not
    if bot.get("subtype") in ("spider", "beast", "avian", "dragon"):
        s += 0.25 - 0.30 * min(g / 2.0, 1.0)
    if cls in ("tank", "hovertank"):
        s += 0.20 * min(g / 2.0, 1.0)
    # a spaceship is at home where there is nothing to land on
    if cls == "spaceship":
        s += 0.3 if not w["has_ground"] else -0.1
    # dust and wind reward sealed, low bodies
    if w["wind_ms"] > 2.5 and cls in ("moto",):
        s -= 0.2
    return True, max(0.0, min(1.0, s))


def populate(world, bots, n=9, outlandish=0.25, seed=0):
    """Draw a roster. Most are ordinary for this world; a quarter are drawn
    from the LOW end of the soft score, which is where the strange but
    survivable ones live."""
    r = rng(seed * 40503 + world["seed"] * 7 + 11)
    ok = [(b, s) for b in bots for hard, s in [score(b, world)] if hard]
    if not ok:
        return []
    ok.sort(key=lambda x: -x[1])
    common = ok[: max(1, len(ok) // 3)]
    odd = ok[-max(1, len(ok) // 3):]
    out, seen = [], set()
    while len(out) < n and (common or odd):
        pool = odd if r() < outlandish and odd else common
        b, s = pool[int(r() * len(pool)) % len(pool)]
        if b["id"] in seen:
            pool.remove((b, s))
            continue
        seen.add(b["id"])
        out.append({"id": b["id"], "name": b["name"], "class": b["class"],
                    "plausibility": round(s, 2),
                    "note": "native" if s >= 0.5 else "an odd thing to find here"})
    return out


def describe(w):
    g = w["gravity_g"]
    bits = []
    bits.append("no ground at all" if not w["has_ground"] else
                ("a tumbling rock field" if w["terrain"] == "asteroid" else
                 f"{'heavy' if g > 1.5 else 'light' if g < 0.4 else 'ordinary'} at {g:.2f} g"))
    bits.append("airless" if w["air_atm"] <= 0.005 else
                f"{w['air_atm']:.2f} atm" + (", and it moves" if w["wind_ms"] > 1.5 else ", still"))
    return "; ".join(bits)


def main():
    seeds = [int(a) for a in sys.argv[1:] if a.isdigit()] or [1]
    bots = json.load(open(os.path.join(HERE, "meta", "bots.json")))["bots"]
    out = []
    for sd in seeds:
        w = make_world(sd)
        roster = populate(w, bots, seed=sd)
        out.append({"world": w, "roster": roster})
        print(f"\nWORLD {sd}  [{w['terrain']}]  {describe(w)}")
        if not roster:
            print("   nothing in the roster can survive here")
            continue
        for b in roster:
            print(f"   {b['plausibility']:.2f}  {b['class']:10s} {b['name'][:30]:30s} {b['note']}")
    if "--json" in sys.argv:
        p = sys.argv[sys.argv.index("--json") + 1]
        json.dump(out, open(p, "w"), indent=1)
        print(f"\n-> {p}")


if __name__ == "__main__":
    main()
