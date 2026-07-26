#!/usr/bin/env python3
"""model_metadata.py -- turn the model library's zip-name scheme into structured
per-model metadata the game can route (patrol region, raid placement, modes...).

The download library encodes classification in PATHS + ZIP NAMES:
  path:  bots/animals/spiders/... or raid-bosses/ or vehicles/cars|flyers|tanks...
         -> category + subcategory
  name prefixes:  both- (robot+vehicle modes), animated / animated idle,
                  detailed, animal-bot--, spiderbot--, hawk-flyer--,
                  dragonflyer--, raidboss-- / raid-boss--, flyer...
  '--<name>'  -> the character/model name
  '-aka ...' / 'aka ...'  -> author's note (quality hints, real-world basis)

Usage:
  python3 tools/model_metadata.py LIBRARY_ROOT --out metadata.json
  # or feed it a text tree (one path per line):
  python3 tools/model_metadata.py --tree tree.txt --out metadata.json
"""
import sys, os, re, json, argparse

CATEGORY_DIRS = {
    "raid-bosses":"raid_boss", "raid_bosses":"raid_boss",
    "animals":"animal", "spiders":"animal", "falcons":"animal", "dragons":"animal",
    "patrol-bots":"patrol_bot", "bots":"bot",
    "cars":"vehicle", "tanks":"vehicle", "flyers":"vehicle", "boats":"vehicle",
    "hoverers":"vehicle", "spaceships":"vehicle", "vehicles":"vehicle",
    "missiles":"weapon", "weapons":"weapon", "cities":"structure",
    "terrains":"terrain", "references":"reference", "raid-bosses":"raid_boss",
}
SUBTYPE_PREFIXES = [
    ("animal-bot", "beast"), ("spiderbot", "spider"), ("spider", "spider"),
    ("hawk-flyer", "avian"), ("dragonflyer", "dragon"), ("raidboss", "raid_boss"),
    ("raid-boss", "raid_boss"), ("flyer", "flyer"),
]

def parse_name(fn):
    """Return a metadata dict for one zip filename (no path)."""
    stem = re.sub(r"\.zip$", "", fn, flags=re.I)
    meta = {"raw": fn, "modes": ["robot"], "flags": [], "subtype": None,
            "name": None, "note": None}
    low = stem.lower()

    # flags / modes
    if low.startswith("both") or "--both" in low or low.startswith("both-"):
        meta["modes"] = ["robot", "alt"]; meta["flags"].append("both_modes")
    if "with-transform-animation" in low or "transform-animation" in low:
        meta["flags"].append("transform_anim")
    if "animated idle" in low: meta["flags"].append("idle_anim")
    elif "animated" in low:    meta["flags"].append("animated")
    if "detailed" in low:      meta["flags"].append("detailed")
    if "+weapon" in low or "weapon" in low: meta["flags"].append("has_weapon")
    if "low-poly" in low or "low poly" in low: meta["flags"].append("low_poly")
    if "pbr" in low:           meta["flags"].append("pbr")
    if re.search(r"set of \d", low): meta["flags"].append("set")

    # subtype from prefix
    for pref, st in SUBTYPE_PREFIXES:
        if low.startswith(pref):
            meta["subtype"] = st; break

    # note after 'aka'
    m = re.search(r"[-\s]aka[-\s]+(.*)$", stem, flags=re.I)
    if m:
        meta["note"] = m.group(1).strip()
        stem = stem[:m.start()]

    # name = strip known prefixes, take the '--<name>' or trailing token
    core = stem
    for pref in ["both--", "both-", "both+weapon-", "animated idle ", "animated - ",
                 "animated-", "detailed-", "animal-bot--", "spiderbot--", "spider-",
                 "hawk-flyer--", "dragonflyer--", "raidboss--", "raidboss-",
                 "raid-boss--", "raid-boss-", "flyer--", "flyer"]:
        if core.lower().startswith(pref):
            core = core[len(pref):]; break
    # a '--name' segment wins if present
    dd = re.search(r"--\s*([A-Za-z0-9_ ]+)", core)
    if dd:
        meta["name"] = dd.group(1).strip()
    else:
        # else clean the remaining core (drop franchise boilerplate)
        c = re.sub(r"(?i)transformers?_?|fall of cybertron|war (for|of) cybertron|"
                   r"decepticon|autobot|cybertron", "", core)
        c = re.sub(r"[_\-]+", " ", c).strip()
        meta["name"] = c or core.strip()
    return meta

def walk_paths(paths):
    out = []
    for p in paths:
        parts = [x for x in re.split(r"[\\/]", p) if x]
        if not parts: continue
        fn = parts[-1]
        if not fn.lower().endswith(".zip"): continue
        cat = None; sub = None
        for seg in parts[:-1]:
            key = seg.lower()
            if key in CATEGORY_DIRS:
                if cat is None: cat = CATEGORY_DIRS[key]
                sub = key
        meta = parse_name(fn)
        meta["category"] = cat or "unknown"
        meta["dir"] = sub
        meta["path"] = p
        out.append(meta)
    return out

def paths_from_tree(text):
    """Reconstruct full paths from an indented `tree`-style listing (│├└─ etc.),
    so parent dirs (= category) are recovered. Keeps only .zip leaves."""
    paths = []; stack = {}
    for line in text.splitlines():
        s = re.sub(r"[│├└─┬┤┌┐┘┴]", " ", line)
        name = s.strip()
        if not name or name.startswith("#") or name.startswith("`") or name.startswith("|"):
            continue
        depth = len(s) - len(s.lstrip())
        stack[depth] = name
        for k in [k for k in stack if k > depth]:
            del stack[k]
        if name.lower().endswith(".zip"):
            paths.append("/".join(stack[k] for k in sorted(stack)))
    return paths

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root", nargs="?", help="library root to walk")
    ap.add_argument("--tree", help="a text file of paths / tree output instead of walking")
    ap.add_argument("--out", default="metadata.json")
    a = ap.parse_args()
    if a.tree:
        paths = paths_from_tree(open(a.tree, errors="ignore").read())
    elif a.root:
        paths = []
        for dp,_d,fs in os.walk(a.root):
            for f in fs:
                if f.lower().endswith(".zip"): paths.append(os.path.join(dp,f))
    else:
        print("give a root dir or --tree file"); sys.exit(1)
    recs = walk_paths(paths)
    json.dump(recs, open(a.out,"w"), indent=1)
    # summary
    from collections import Counter
    cats = Counter(r["category"] for r in recs)
    both = sum(1 for r in recs if "both_modes" in r["flags"])
    print(f"{len(recs)} models -> {a.out}")
    print("by category:", dict(cats))
    print(f"both-modes (robot+alt): {both}")

if __name__ == "__main__":
    main()
