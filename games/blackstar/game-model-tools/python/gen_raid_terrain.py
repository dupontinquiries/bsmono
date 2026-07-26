#!/usr/bin/env python3
"""gen_raid_terrain.py -- procedural battlefields for raid bosses.

raid_content.py gives every boss an epithet, a class and combat TAGS. This turns
those tags into an actual arena: a .hgt heightfield the engine already loads
(`data/<mapid>.hgt`, header "res half" then res+1 rows of res+1 heights).

Terrain is DETERMINISTICALLY SEEDED by the boss name, so a given boss always
fights on the same ground, and THEMED by the tags:
  burrow / armor_pierce -> pitted crater field (the thing digs)
  quake / aoe           -> fractured ridges + fault lines
  artillery / armor     -> cover ridges with firing lanes between
  colossus / siege      -> wide open arena, low rolling ground (room for a titan)
  swarm / web           -> broken mounds + hollows (breaks up sightlines)
  default               -> rolling dunes
Always carves a FLAT landing zone at the centre so the drop-in isn't on a cliff,
and reports playability metrics (slope histogram, traversable fraction) because
the terrain has to be checked by numbers, not by eye.

Usage:
  python3 tools/gen_raid_terrain.py --raid raid_content.json --out engine_src/data
  python3 tools/gen_raid_terrain.py --name Gigadrill --tags burrow,armor_pierce --out .
"""
import sys, os, json, argparse, hashlib
import numpy as np

def seed_of(name):
    return int(hashlib.sha256(name.encode()).hexdigest()[:8], 16)

def value_noise(rng, res, octaves=5, persistence=0.5):
    """Layered value noise on a (res+1)^2 grid -- smooth, tileable enough, no deps."""
    n = res + 1
    total = np.zeros((n, n), dtype=np.float64)
    amp, norm = 1.0, 0.0
    freq = 2
    for _ in range(octaves):
        g = rng.random((freq + 1, freq + 1))
        # bilinear upsample to full res
        ys = np.linspace(0, freq, n)
        xs = np.linspace(0, freq, n)
        y0 = np.clip(ys.astype(int), 0, freq - 1); y1 = y0 + 1
        x0 = np.clip(xs.astype(int), 0, freq - 1); x1 = x0 + 1
        fy = (ys - y0)[:, None]; fx = (xs - x0)[None, :]
        # smoothstep for continuity
        fy = fy * fy * (3 - 2 * fy); fx = fx * fx * (3 - 2 * fx)
        g00 = g[np.ix_(y0, x0)]; g10 = g[np.ix_(y1, x0)]
        g01 = g[np.ix_(y0, x1)]; g11 = g[np.ix_(y1, x1)]
        layer = (g00 * (1 - fy) * (1 - fx) + g10 * fy * (1 - fx)
                 + g01 * (1 - fy) * fx + g11 * fy * fx)
        total += layer * amp
        norm += amp
        amp *= persistence
        freq *= 2
    return total / max(norm, 1e-9)

def radial(res):
    n = res + 1
    a = np.linspace(-1, 1, n)
    X, Y = np.meshgrid(a, a)
    return np.sqrt(X * X + Y * Y), X, Y

def shape_for(tags, rng, res, base):
    """Turn the noise into a themed battlefield."""
    r, X, Y = radial(res)
    t = set(tags)
    h = base.copy()

    if t & {"burrow", "armor_pierce"}:
        # pitted crater field: punch a handful of bowls
        h *= 0.8
        for _ in range(rng.integers(6, 11)):
            cx, cy = rng.uniform(-0.75, 0.75, 2)
            rad = rng.uniform(0.12, 0.26)
            d = np.sqrt((X - cx) ** 2 + (Y - cy) ** 2)
            bowl = np.clip(1.0 - (d / rad) ** 2, 0, 1) ** 2
            h -= bowl * rng.uniform(0.5, 1.0)
        theme = "pitted crater field"
    elif t & {"quake", "aoe"}:
        # fractured: sharp ridges from folded noise + a fault line
        h = 1.0 - np.abs(h * 2 - 1)          # fold -> creases
        ang = rng.uniform(0, np.pi)
        fault = np.abs(X * np.cos(ang) + Y * np.sin(ang))
        h -= np.clip(1.0 - fault / 0.10, 0, 1) * 0.8
        theme = "fractured ridges + fault line"
    elif t & {"artillery", "armor"}:
        # cover ridges with lanes: banded ridges, lanes cut through
        h = 0.45 * h + 0.55 * (0.5 + 0.5 * np.sin(6.0 * (X * np.cos(0.6) + Y * np.sin(0.6))))
        lanes = np.abs(np.sin(3.0 * (X * -np.sin(0.6) + Y * np.cos(0.6))))
        h *= 0.35 + 0.65 * lanes
        theme = "cover ridges with firing lanes"
    elif t & {"colossus", "siege"}:
        # wide open low-relief arena -- room to fight something huge
        h = 0.35 * h
        h += 0.25 * np.clip(r - 0.55, 0, None)      # gentle rim, open middle
        theme = "wide open arena"
    elif t & {"swarm", "web"}:
        # broken mounds + hollows to cut sightlines
        h = 0.6 * h + 0.4 * (h ** 3) * 2.0
        for _ in range(rng.integers(8, 14)):
            cx, cy = rng.uniform(-0.85, 0.85, 2)
            rad = rng.uniform(0.06, 0.15)
            d = np.sqrt((X - cx) ** 2 + (Y - cy) ** 2)
            h += np.clip(1.0 - d / rad, 0, 1) ** 2 * rng.uniform(0.3, 0.7)
        theme = "broken mounds + hollows"
    else:
        theme = "rolling dunes"

    # soft bowl toward the rim so the arena reads as enclosed, never a wall
    h += np.clip(r - 0.8, 0, None) * 1.2
    # FLAT LANDING ZONE at the centre -- drop-in must not be on a cliff. A soft
    # mask let steep crater/fault features survive at its edge, so this is a true
    # plateau (hard flat core) with a wide smooth blend out of it.
    core = np.clip(1.0 - (r / 0.14) ** 2, 0, 1)          # solid inner disc
    blend = np.clip(1.0 - (r / 0.30) ** 2, 0, 1) ** 2    # wide feather
    lz = np.maximum(core, blend * 0.85)
    lz = np.clip(lz, 0, 1)
    plateau = float(np.average(h, weights=np.clip(core, 1e-6, None)))
    h = h * (1 - lz) + plateau * lz
    return h, theme

def metrics(h, half, res):
    """Playability by numbers: slope distribution + traversable fraction."""
    cell = (2.0 * half) / res
    gy, gx = np.gradient(h, cell)
    slope = np.degrees(np.arctan(np.sqrt(gx * gx + gy * gy)))
    walk = float((slope <= 35.0).mean())        # a bot can climb ~35 deg
    steep = float((slope > 55.0).mean())
    r, _, _ = radial(res)
    lz = slope[r <= 0.16]
    return {
        "slope_mean": round(float(slope.mean()), 2),
        "slope_p95": round(float(np.percentile(slope, 95)), 2),
        "traversable_frac": round(walk, 3),
        "impassable_frac": round(steep, 3),
        "landing_zone_max_slope": round(float(lz.max()) if lz.size else 0.0, 2),
        "relief": round(float(h.max() - h.min()), 2),
    }

def write_hgt(path, h, half, res):
    with open(path, "w") as f:
        f.write("%d %.5f\n" % (res, half))
        for j in range(res + 1):
            f.write(" ".join("%.4f" % v for v in h[j]) + "\n")

def generate(name, tags, out_dir, res=88, half=85.0, relief=4.5):
    rng = np.random.default_rng(seed_of(name))
    base = value_noise(rng, res)
    h, theme = shape_for(tags, rng, res, base)
    # normalize to a sane relief band, floor at 0 (engine clamps below 0 anyway)
    h -= h.min()
    if h.max() > 1e-9:
        h *= (relief / h.max())
    mapid = "raid_" + "".join(c.lower() if c.isalnum() else "_" for c in name).strip("_")[:28]
    path = os.path.join(out_dir, mapid + ".hgt")
    write_hgt(path, h, half, res)
    return mapid, theme, metrics(h, half, res)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--raid", help="raid_content.json from raid_content.py")
    ap.add_argument("--name", help="single boss name")
    ap.add_argument("--tags", default="", help="comma tags for --name")
    ap.add_argument("--out", default=".")
    ap.add_argument("--res", type=int, default=88)
    ap.add_argument("--half", type=float, default=85.0)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)

    jobs = []
    if a.raid:
        data = json.load(open(a.raid))
        for b in data.get("raid_bosses", []):
            jobs.append((b["name"], b.get("tags", [])))
    elif a.name:
        jobs.append((a.name, [t for t in a.tags.split(",") if t]))
    else:
        print("give --raid raid_content.json or --name NAME --tags a,b")
        sys.exit(1)

    print("%d raid arena%s\n" % (len(jobs), "" if len(jobs) == 1 else "s"))
    bad = 0
    for name, tags in jobs:
        mapid, theme, m = generate(name, tags, a.out, a.res, a.half)
        flag = ""
        if m["traversable_frac"] < 0.75: flag += " [!] hard to cross"
        if m["landing_zone_max_slope"] > 12.0: flag += " [!] steep landing zone"
        if flag: bad += 1
        print("  %-26.26s %-28.28s walk %.0f%%  relief %.1f  lz-slope %.1f%s"
              % (name, theme, m["traversable_frac"] * 100, m["relief"],
                 m["landing_zone_max_slope"], flag))
    print("\n%d arenas written to %s%s" % (len(jobs), a.out,
          ("  (%d flagged)" % bad) if bad else "  (all playable)"))

if __name__ == "__main__":
    main()
