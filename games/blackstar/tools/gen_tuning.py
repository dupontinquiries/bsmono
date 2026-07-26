#!/usr/bin/env python3
"""gen_tuning.py -- fill model_tuning.json for EVERY installed model, measurably.

The correction harness (game.c F4) and the auto_orient seeds covered the first 48
imported models; the library is now 1013. This closes the gap without hand-tuning:

  kind        name-keyword classification first (names in this library are strong:
              bot_/tank/blaster/boat/planet/... carry the category), bounding-box
              shape as the fallback (auto_orient.py's measured heuristic, ~70% --
              which is why names win when they speak).
  rx/ry/rz    the processed library is already normalized (max extent 2.0, Y-up for
              standing things; engine default ry=-90 handles facing). So rotation is
              DEVIANT DETECTION only: a robot/humanoid whose Y extent is its
              SMALLEST axis is lying down (rx=-90 stands it up); everything else
              keeps the default. No speculative rotations -- the F4 harness and the
              in-game log remain the escalation path for the ones a box can't judge.
  scale       per-kind multiplier over the measured library default 0.33, then
              normalized by the model's own vertical extent so a 0.7-tall weapon
              and a 2.0-tall titan land at their kind's intended in-game height.
  pair        vehicle <-> robot transform pairs by shared RARE name token (a token
              appearing in <= 6 model names is identifying: bumblebee, brawl,
              megatron, barricade...). Only cross-kind pairs; deterministic
              (alphabetical first-match); written as the OTHER model's name.

Writes/merges model_tuning.json (name-keyed, exactly the installer's format, plus
the new "kind"/"pair" keys the installer bakes into imported_models.h). Values a
human already tuned (non-default rx/ry/rz/scale) are NEVER overwritten -- same rule
as install_models.py's inherit logic.

Std-lib only on purpose. Rust twin queued in maintenance/RUST_PORT_PLAN.md Tier 1.
"""
import os, re, sys, json, struct

DEF_RX, DEF_RY, DEF_RZ, DEF_SCL = 0.0, -90.0, 0.0, 0.33

# kind ids match the header enum the installer writes:
KINDS = ["prop", "robot", "vehicle", "weapon", "env", "ship"]
KW = {
    "ship":    ["starship", "spaceship", "battle_ship", "star_destroyer", "cruiser",
                "portaviones", "lhd", "nemesis_ship", "ark"],
    "weapon":  ["sword", "blaster", "gun", "rifle", "cannon", "scythe", "axe", "blade",
                "benelli", "pistol", "launcher", "railgun", "missile", "katana", "knife",
                "shotgun", "smg", "sniper", "turret", "weapon"],
    "vehicle": ["truck", "jeep", "boat", "plane", "helicopter",
                "camaro", "peterbilt", "mustang", "corvette", "vehicle", "bike",
                "motorcycle", "buggy", "van_", "_van", "bus_", "_bus", "semi", "hauler",
                "aircraft", "bomber", "fighter_jet", "chevrolet", "lamborghini", "audi",
                "porsche", "nissan", "ambulance", "police_car", "firetruck"],
    "robot":   ["bot_", "_bot", "robot", "mech", "prime", "megatron", "soundwave",
                "walker", "humanoid", "figure", "titan", "bumblebee", "optimus",
                "grimlock", "starscream", "ironhide", "jazz", "ratchet", "shockwave",
                "sideswipe", "barricade", "brawl", "blackout", "devastator", "sentinel",
                "dinobot", "autobot", "decepticon", "droid", "android", "skeleton",
                "speakerman", "transformers"],
    "env":     ["building", "tower", "house", "mountain", "lake", "tree", "rock",
                "ruin", "station_", "bathymetry", "terrain", "landscape", "city",
                "rodeo_drive", "street", "bridge_", "sandstone", "glacial", "hills",
                "california", "scene", "environment", "arena_", "hangar", "warehouse"],
}
# generic tokens that never identify a franchise pair
GENERIC = set("""the of and a an to in for with v1 v2 v3 3d model animated animation
transformers transformer war fall cybertron rise dark moon age extinction last knight
game ready lowpoly low poly free rigged idle reference concept stylized part or full
lmb rib veh limb bot robot vehicle tank car truck imported scale large small
tread mech walker flyer futuristic advanced warfare call duty donors form alliance
transform hot halo aka wolf wing both meshes gravity cyber tron mode
human rigged textured version updated final new old classic modern military
""".split())


def iqm_bounds(path):
    with open(path, "rb") as f:
        b = f.read()
    if b[:16] != b"INTERQUAKEMODEL\0":
        return None
    h = struct.unpack_from("<27I", b, 16)
    num_va, num_v, ofs_va = h[7], h[8], h[9]
    for i in range(num_va):
        t, _fl, fmt, size, off = struct.unpack_from("<5I", b, ofs_va + i * 20)
        if t == 0 and fmt == 7 and size == 3:
            lo = [1e30] * 3
            hi = [-1e30] * 3
            step = max(1, num_v // 4000)  # bounds from a stride sample: plenty for a box
            for v in range(0, num_v, step):
                x, y, z = struct.unpack_from("<3f", b, off + v * 12)
                for k, val in enumerate((x, y, z)):
                    if val < lo[k]:
                        lo[k] = val
                    if val > hi[k]:
                        hi[k] = val
            return [hi[k] - lo[k] for k in range(3)]
    return None


VEH_TOKENS = {"car", "tank", "jet", "bus", "van", "suv", "atv"}
ROBOT_TOKENS = {"robot", "bot", "mech", "walker", "droid", "android", "mecha"}

def classify(name, ext):
    toks = set(t for t in name.split("_") if t)
    # explicit robot tokens beat vehicle words living inside compounds (antitank...),
    # EXCEPT an explicit vehicle-form alt-mode marker, which names the vehicle side
    if "vehicle" in name and "form" in toks:
        return "vehicle"
    if toks & ROBOT_TOKENS:
        return "robot"
    if toks & VEH_TOKENS:
        return "vehicle"
    for kind in ("ship", "weapon", "vehicle", "env", "robot"):  # specificity order
        for kw in KW[kind]:
            if kw in name:
                return kind
    if ext:
        s = sorted(ext)
        flat, balance = s[0] / max(s[2], 1e-6), s[1] / max(s[2], 1e-6)
        if (0.62 - balance) + (0.32 - flat) > 0.04:  # auto_orient's measured line
            return "vehicle"
    return "robot" if ext and ext[1] >= max(ext[0], ext[2]) * 0.8 else "prop"


# in-game target heights (multiples of the library default character height):
TARGET_H = {"robot": 2.0, "vehicle": 1.1, "weapon": 0.9, "prop": 1.2, "env": 6.0, "ship": 5.0}


def excluded(name):
    return name.startswith("part_") or "_part" in name or name.endswith("_meshes")


def tokens(name):
    out = []
    for t in name.split("_"):
        letters = sum(ch.isalpha() for ch in t)
        if len(t) >= 3 and letters >= 2 and t not in GENERIC:
            out.append(t)
    return out


def main():
    root = sys.argv[sys.argv.index("--root") + 1] if "--root" in sys.argv else "."
    data = os.path.join(root, "engine_src", "data")
    tpath = os.path.join(root, "model_tuning.json")
    tuning = {}
    if os.path.exists(tpath):
        tuning = json.load(open(tpath))

    names = sorted(
        f[len("imported_"):-4]
        for f in os.listdir(data)
        if f.startswith("imported_") and f.endswith(".iqm")
    )
    exts, kinds = {}, {}
    lying = 0
    for n in names:
        ext = iqm_bounds(os.path.join(data, f"imported_{n}.iqm"))
        exts[n] = ext
        kinds[n] = classify(n, ext)

    # token index for pair detection
    tok_index = {}
    for n in names:
        for t in set(tokens(n)):
            tok_index.setdefault(t, []).append(n)

    pairs = {}
    for n in names:
        if kinds[n] != "vehicle" or n in pairs or excluded(n):
            continue
        best = None
        for t in tokens(n):
            group = tok_index.get(t, [])
            if len(group) > 6:
                continue  # common token, not identifying
            for m in group:
                if m != n and kinds[m] == "robot" and m not in pairs and not excluded(m):
                    if best is None or m < best:
                        best = m
        if best:
            pairs[n] = best
            pairs[best] = n

    changed = 0
    for n in names:
        ent = tuning.get(n, {})
        hand_tuned = ent and not (
            abs(ent.get("rx", DEF_RX) - DEF_RX) < 1e-6
            and abs(ent.get("ry", DEF_RY) - DEF_RY) < 1e-6
            and abs(ent.get("rz", DEF_RZ) - DEF_RZ) < 1e-6
            and abs(ent.get("scale", DEF_SCL) - DEF_SCL) < 1e-6
        )
        ext = exts[n]
        kind = kinds[n]
        if not hand_tuned:
            rx = DEF_RX
            if ext and kind in ("robot",) and ext[1] < min(ext[0], ext[2]) * 0.75:
                rx = -90.0  # lying down: stand it up
                lying += 1
            # scale: kind target over the model's own vertical extent (post-rx: if we
            # stood it up, the vertical is the larger horizontal axis)
            v_ext = ext[1] if ext else 2.0
            if rx != 0.0 and ext:
                v_ext = max(ext[0], ext[2])
            v_ext = max(v_ext, 0.05)
            scale = DEF_SCL * TARGET_H[kind] / 2.0 * (2.0 / v_ext)
            ent.update({"rx": rx, "ry": DEF_RY, "rz": DEF_RZ, "scale": round(scale, 4)})
            changed += 1
        ent["kind"] = kind
        if n in pairs:
            ent["pair"] = pairs[n]
        tuning[n] = ent

    json.dump(tuning, open(tpath, "w"), indent=1, sort_keys=True)
    from collections import Counter
    c = Counter(kinds.values())
    print(f"{len(names)} models: " + ", ".join(f"{k}={c[k]}" for k in KINDS if c[k]))
    print(f"pairs: {len(pairs)//2} vehicle<->robot; lying-fixed: {lying}; "
          f"tuned entries written: {changed} (hand-tuned preserved: {len(names)-changed})")


if __name__ == "__main__":
    main()
