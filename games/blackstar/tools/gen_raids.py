#!/usr/bin/env python3
"""gen_raids.py -- build raid content from the boss lore, and the header the
engine reads.

Two gaps this closes:
  * raid_plan.h referenced maps "kuiper" and "unicron" that do not exist in
    maps.json, so 2 of 3 phases pointed at nothing.
  * raids-and-relics.md has 19 bosses with rich fiction and none of it reached
    the game -- the raid was three generic phases with no boss at all.

The interesting part is turning fiction into MECHANICS rather than flavour text.
Each boss's lore already implies how its fight should work:

    Cherno Alpha  "if it ever stops walking, the reactor reaches criticality"
                  -> the boss must keep moving; stopping it is the fail state,
                     so the arena punishes players who body-block it
    Gigadrill     "it drilled through moons"
                  -> it leaves the arena floor. Phases alternate surface/undercut
    Speakerman    "could fracture planets by matching tectonic resonance"
                  -> periodic resonance pulses that damage anyone standing on
                     the same structure type
    Trenchmaster  "the ocean king"
                  -> rising flood line; high ground shrinks over the fight

Mechanic ids match the engine's raid_enc_t.mech field:
    0 PURGE   1 GROUND (anti-air, flyers forced down)
    2 AIR     (elevated, fly/grapple)     3 SYNC (min_units on plates)

Usage:
  python3 tools/gen_raids.py --out content/raids.json --header engine_src/exengine/raid_plan.h
"""
import argparse, json, os, random, re, sys

# name -> (title, mech, arena tag, the lore line the mechanic comes from)
BOSSES = [
 ("Astro Armor Titan Speakerman","The Last Resonance",3,"resonance",
  "sings across half a galaxy; fractures planets by matching tectonic resonance"),
 ("Devastator Jhiaxus","Prototype Omega",0,"rebuild",
  "destroying one only caused it to rebuild itself"),
 ("Plasma's Mechagodzilla","The Planet Burner",1,"burn",
  "its spinal plasma spine could ignite gas giants"),
 ("Cherno Alpha","The Walking Reactor",1,"march",
  "if it ever stops walking, the reactor reaches criticality"),
 ("Gigadrill","The Core Piercer",2,"undercut",
  "it drilled through moons, worlds, dwarf planets"),
 ("Hardboil","The Pressure King",0,"pressure",
  "built for depths that crush ordinary alloys"),
 ("Hussar Tank","The Charge",1,"charge",
  "it does not manoeuvre; it commits"),
 ("Judgementor","Executor of Empires",3,"sentence",
  "it did not fight wars, it ended them"),
 ("Primitive Mantis Mech","The First Hunter",0,"ambush",
  "the oldest predator pattern Cybertron ever built"),
 ("Pyrachnid","Forge Widow",3,"web",
  "it spins its own foundry around its prey"),
 ("Scorpion Commander","The Frontier Marshal",1,"frontier",
  "held a border that no longer exists"),
 ("Starlord","Twin Hammers",2,"sentry",
  "two hammers that are sentry guns"),
 ("Strider Tank","The Horizon Walker",2,"horizon",
  "it walks toward the horizon and the horizon moves"),
 ("Tremor","Planetbreaker",1,"quake",
  "the ground is its weapon"),
 ("Trenchmaster","The Ocean King",3,"flood",
  "the ocean king; the water rises to meet it"),
 ("Wuthering Waves Sigillum","The Seal",0,"seal",
  "a seal is only a door that has not been opened"),
 ("Titan Cameraman 3.0 Armor","Archivist",2,"archive",
  "it records everything; it has never stopped"),
 ("Sphere Robot","The Observer",0,"observe",
  "it watches, and being watched changes what you do"),
 ("Titan Cameraman 3.0 (What If...)","The Divergence",3,"diverge",
  "the recording and the event no longer agree"),
]

# maps the raid can draw on, plus the two the old header referenced but that were
# never defined. Adding them here is what stops phases pointing at nothing.
NEW_MAPS = {
  "kuiper": {"atmo": "void_edge", "energon": False,
             "structures": ["ring", "spire"],
             "landmark": "shepherd moons + a frozen debris braid"},
  "unicron": {"atmo": "devourer_red", "energon": True,
              "structures": ["arch", "spire", "tower"],
              "landmark": "the maw; horizon curves the wrong way"},
}

def slug(s):
    return re.sub(r"[^a-z0-9]+", "_", s.lower()).strip("_")

def encounters(rng, mech, n=4):
    """Scatter encounters, with the boss's own mechanic on at least one of them.
    A phase where every encounter is mech 0 plays identically to no mechanic."""
    out = []
    for i in range(n):
        x = round(rng.uniform(-95, 95), 1)
        z = round(rng.uniform(-95, 95), 1)
        m = mech if i == n - 1 else rng.choice([0, 0, mech])
        out.append({"x": x, "z": z, "kind": 1 if i == 0 else 0,
                    "mech": m, "min_units": 3 if m == 3 else 1})
    return out

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="content/raids.json")
    ap.add_argument("--header", default="engine_src/exengine/raid_plan.h")
    ap.add_argument("--maps", default="content/maps.json")
    ap.add_argument("--phases", type=int, default=3, help="phases per raid")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--raid", default=None, help="which raid to bake into the header")
    a = ap.parse_args()
    rng = random.Random(a.seed)

    maps = json.load(open(a.maps))
    added = [k for k in NEW_MAPS if k not in maps["maps"]]
    for k, v in NEW_MAPS.items():
        maps["maps"].setdefault(k, v)
    if added:
        json.dump(maps, open(a.maps, "w"), indent=1)
        print("added missing maps: %s" % ", ".join(added))

    pool = [m for m in maps["maps"] if m not in ("beach",)]
    raids = {}
    for name, title, mech, arena, line in BOSSES:
        rid = slug(name)
        # the last phase always lands on the boss's own arena if one exists,
        # otherwise on the most thematically extreme map available
        finale = "unicron" if mech == 3 else rng.choice(pool)
        chosen = rng.sample([m for m in pool if m != finale], a.phases - 1) + [finale]
        raids[rid] = {
            "name": name,
            "title": title,
            "arena": arena,
            "premise": line,
            "mech": mech,
            "phases": [
                {"name": pn, "map": mp, "encounters": encounters(rng, mech)}
                for pn, mp in zip(["The Approach", "The Gauntlet", "The %s" % title.split()[-1]],
                                  chosen)
            ],
        }

    doc = ("Raid content generated from raids-and-relics.md by tools/gen_raids.py. "
           "'mech' is the engine's raid_enc_t.mech (0 purge, 1 ground, 2 air, 3 sync) "
           "chosen from each boss's fiction, not assigned at random -- see 'premise'.")
    os.makedirs(os.path.dirname(a.out) or ".", exist_ok=True)
    json.dump({"_doc": doc, "raids": raids}, open(a.out, "w"), indent=1)
    print("wrote %s (%d raids, %d phases each)" % (a.out, len(raids), a.phases))

    # bake one raid into the header the engine already reads
    pick = a.raid or "cherno_alpha"
    if pick not in raids:
        pick = sorted(raids)[0]
    r = raids[pick]
    L = ["// AUTO-GENERATED by tools/gen_raids.py from raids-and-relics.md. Do not edit.",
         "#ifndef RAID_PLAN_H", "#define RAID_PLAN_H", "",
         "// mech: 0 PURGE  1 GROUND(anti-air, flyers forced down)  2 AIR(elevated, fly/grapple)  3 SYNC(min_units on plates)",
         "typedef struct { float x, z; int kind; int mech; int min_units; } raid_enc_t;",
         "typedef struct { const char *name; const char *map; int n_enc; const raid_enc_t *enc; } raid_phase_t;",
         "",
         "// %s -- \"%s\"" % (r["name"], r["title"]),
         "// %s" % r["premise"], ""]
    for i, ph in enumerate(r["phases"]):
        L.append("static const raid_enc_t g_raid_enc_%d[] = {" % i)
        for e in ph["encounters"]:
            L.append("  { %.1ff, %.1ff, %d, %d, %d }," %
                     (e["x"], e["z"], e["kind"], e["mech"], e["min_units"]))
        L.append("};")
    L += ["", "#define NUM_RAID_PHASES %d" % len(r["phases"]),
          "static const raid_phase_t g_raid_phases[NUM_RAID_PHASES] = {"]
    for i, ph in enumerate(r["phases"]):
        L.append('  { "%s", "%s", %d, g_raid_enc_%d },' %
                 (ph["name"], ph["map"], len(ph["encounters"]), i))
    L += ["};", "", "#endif", ""]
    open(a.header, "w").write("\n".join(L))
    print("baked '%s' into %s" % (pick, a.header))

    # every map a raid references must exist, or a phase loads nothing
    missing = sorted({p["map"] for rr in raids.values() for p in rr["phases"]}
                     - set(maps["maps"]))
    if missing:
        print("!! phases reference undefined maps: %s" % ", ".join(missing))
        sys.exit(1)
    print("all referenced maps exist")

if __name__ == "__main__":
    main()
