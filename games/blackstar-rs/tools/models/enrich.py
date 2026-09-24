#!/usr/bin/env python3
"""enrich.py — add character metadata to a BSWEB parts web.

    enrich.py derive  web.bin.zst          computed fields, no human input
    enrich.py tag     web.bin.zst          one keystroke per part
    enrich.py report  web.bin.zst          coverage + what each field buys

The problem this addresses: the web has 1,790 parts and exactly ONE semantic
field — `name`, with 10 distinct values. Every wheel is "wheel". A bot
assembled from these is geometrically varied and mechanically identical,
because nothing downstream has anything to read.

Three tiers, cheapest first. Tier 0 needs no human at all; tier 1 is one
keypress; tier 2 is four small integers and is where the combinatorics live.

bsweb preserves unknown index fields through `#[serde(flatten)]`, so
everything written here survives `rm`, `gc`, `merge` and a re-save.
"""
import argparse, json, math, os, re, struct, sys
from collections import Counter, defaultdict

MAGIC = b"BSWEB\x01"


# ------------------------------------------------------------------ io

def read_web(path):
    raw = open(path, "rb").read()
    if raw[:2] == b"\x1f\x8b":
        import gzip; raw = gzip.decompress(raw); codec = "gz"
    elif raw[:4] == b"\x28\xb5\x2f\xfd":
        import zstandard; raw = zstandard.ZstdDecompressor().decompress(raw, max_output_size=1 << 30); codec = "zst"
    else:
        codec = "raw"
    if raw[:6] != MAGIC:
        sys.exit("not a BSWEB container")
    il, pl = struct.unpack("<II", raw[6:14])
    return json.loads(raw[14:14 + il]), raw[14 + il:14 + il + pl], codec


def write_web(path, index, payload, codec):
    idx = json.dumps(index, separators=(",", ":")).encode()
    raw = MAGIC + struct.pack("<II", len(idx), len(payload)) + idx + payload
    tmp = path + ".tmp"
    if codec == "zst":
        import zstandard
        open(tmp, "wb").write(zstandard.ZstdCompressor(level=19).compress(raw))
    elif codec == "gz":
        import gzip
        open(tmp, "wb").write(gzip.compress(raw, 9))
    else:
        open(tmp, "wb").write(raw)
    os.replace(tmp, path)
    return os.path.getsize(path)


def key(p):
    return f"{p.get('catalog')}/{p['id']}" if p.get("catalog") else p["id"]


# --------------------------------------------------- tier 0: derived

# `from` already holds the donor: car-1968_alfa_romeo…, tank--alpine doom.
# 286 distinct donors in 12 classes, and nothing reads it. A wheel off a 1968
# Alfa and a wheel off a siege tank are different objects; the web already
# knows which is which and throws the fact away.
DONOR = {
    "car": "civilian", "racer": "civilian", "truck": "utility",
    "tank": "military", "mech": "military", "heli": "military",
    "flyer": "aero", "spaceship": "aero", "hoverer": "aero", "boat": "marine",
}


def donor_class(p):
    src = (p.get("from") or "").lower()
    head = re.split(r"[-_\s]", src, 1)[0]
    return DONOR.get(head, "salvage" if head == "imported" else "other")


def shape_band(p):
    """Longest:shortest bbox ratio. `name` does not capture this at all —
    'wheel' spans aspect 2.1 to 31.6, i.e. discs and long cylinders share a
    label. This is the discriminator a slot picker actually wants."""
    s = sorted(x for x in (p.get("span") or [0, 0, 0]))
    if s[0] <= 1e-6:
        return "flat"                      # degenerate axis: a plate or a card
    r = s[2] / s[0]
    return "compact" if r < 3 else "bar" if r < 8 else "plate" if r < 20 else "needle"


def bulk_band(p):
    """Geometric size band, same log spacing web_slots.py already uses so the
    two agree rather than drifting."""
    s = max(0.02, min(2.0, p.get("size") or 0.02))
    t = (math.log(s) - math.log(0.02)) / (math.log(2.0) - math.log(0.02))
    return ["tiny", "small", "medium", "large"][max(0, min(3, int(t * 4)))]


def detail_band(p):
    """Triangles against bbox area — 'how fussy does this read'. Volume is
    unusable because a flat part has a near-zero axis and the ratio explodes
    (measured p90/p10 of 3.4 million on wheels). Area is stable."""
    sp = sorted(p.get("span") or [1, 1, 1])
    area = max(1e-6, sp[1] * sp[2])
    d = (p.get("nt") or 1) / area
    return "plain" if d < 2000 else "worked" if d < 20000 else "ornate"


def derive(parts):
    n = 0
    for p in parts:
        p["donor"] = donor_class(p)
        p["shape"] = shape_band(p)
        p["bulk"] = bulk_band(p)
        p["detail"] = detail_band(p)
        n += 1
    return n


# ------------------------------------------------ tier 1 + 2: human

# One key each. Not a form — a form is why metadata never gets added.
FEEL = {"1": "light", "2": "medium", "3": "heavy"}
FINISH = {"q": "clean", "w": "worn", "e": "industrial", "r": "ornate", "t": "alien"}

# Four small ints, -2..+2, that SUM across an assembled bot. This is the
# combinatorial field: 1,790 parts each needing four numbers turns into a
# space of bots that are mechanically distinct, without authoring a single
# bot. The signature system in content/signatures.json already works in
# multipliers (dmg_mul, speed_mul, taken_mul), so these compose into it
# rather than replacing it.
BIAS_KEYS = ["armor", "speed", "heat", "power"]

# Sensible starting bias inferred from what we already know, so the human is
# CORRECTING rather than inventing. That is the difference between a pass
# that gets finished and one that does not.
def suggest_bias(p):
    b = dict.fromkeys(BIAS_KEYS, 0)
    if p.get("bulk") in ("large",):      b["armor"] += 1; b["speed"] -= 1
    if p.get("bulk") in ("tiny", "small"): b["speed"] += 1
    if p.get("donor") == "military":     b["armor"] += 1
    if p.get("donor") == "aero":         b["speed"] += 1; b["armor"] -= 1
    if p.get("donor") == "civilian":     b["heat"] -= 1
    if p.get("name") in ("barrel",):     b["power"] += 2; b["heat"] += 1
    if p.get("name") in ("wheel", "roadwheel", "tread_link"): b["speed"] += 1
    if p.get("detail") == "ornate":      b["heat"] += 1
    return {k: max(-2, min(2, v)) for k, v in b.items()}


def tag(parts, limit, only_name, only_untagged):
    sel = [p for p in parts
           if (not only_name or p.get("name") == only_name)
           and (not only_untagged or "feel" not in p)]
    sel.sort(key=lambda p: -(p.get("instances") or 0))   # most reused first
    sel = sel[:limit]
    if not sel:
        print("nothing to tag"); return 0
    print(f"{len(sel)} part(s), most-reused first. Each one is two keypresses.")
    print(f"  feel   {'  '.join(f'{k}={v}' for k, v in FEEL.items())}")
    print(f"  finish {'  '.join(f'{k}={v}' for k, v in FINISH.items())}")
    print("  s=skip  q=quit and save\n")
    done = 0
    for i, p in enumerate(sel, 1):
        print(f"[{i}/{len(sel)}] {key(p)}")
        print(f"      {p.get('name')} · {p.get('bulk')} · {p.get('shape')} · "
              f"{p.get('detail')} · {p.get('donor')} · {p.get('nt')} tris · "
              f"used {p.get('instances')}x")
        print(f"      from {p.get('from')}")
        a = input("      feel? ").strip().lower()
        if a == "q": break
        if a == "s" or a not in FEEL: print(); continue
        b = input("      finish? ").strip().lower()
        if b == "q": break
        if b not in FINISH: print(); continue
        p["feel"] = FEEL[a]
        p["finish"] = FINISH[b]
        p.setdefault("bias", suggest_bias(p))
        done += 1
        print()
    return done


# ---------------------------------------------------------- reporting

def report(parts):
    print(f"{len(parts)} parts\n")
    fields = ["name", "donor", "shape", "bulk", "detail", "feel", "finish", "bias"]
    print(f"{'field':<10}{'coverage':>10}{'distinct':>10}   values")
    for f in fields:
        have = [p[f] for p in parts if p.get(f) is not None]
        if f == "bias":
            print(f"{f:<10}{len(have)/len(parts)*100:>9.0f}%{'—':>10}   4 ints, summed per assembly")
            continue
        c = Counter(have)
        vals = ", ".join(f"{k}:{v}" for k, v in c.most_common(6))
        print(f"{f:<10}{len(have)/len(parts)*100:>9.0f}%{len(c):>10}   {vals[:64]}")

    # the number that matters: how many DISTINCT part characters exist
    combos = Counter(tuple(p.get(f) for f in ["name", "donor", "shape", "bulk", "detail"])
                     for p in parts)
    print(f"\ndistinct (name, donor, shape, bulk, detail) combinations: {len(combos)}")
    print(f"  vs {len(set(p.get('name') for p in parts))} distinct names before enrichment")
    singles = sum(1 for v in combos.values() if v == 1)
    print(f"  {singles} of those are unique to one part")


def main():
    ap = argparse.ArgumentParser(prog="enrich")
    s = ap.add_subparsers(dest="cmd", required=True)
    for c in ("derive", "tag", "report"):
        q = s.add_parser(c); q.add_argument("web")
        if c != "report":
            q.add_argument("-o", help="write here instead of in place")
    t = [a for a in s.choices["tag"]._actions]
    s.choices["tag"].add_argument("-n", type=int, default=40)
    s.choices["tag"].add_argument("--name", help="only this part kind")
    s.choices["tag"].add_argument("--untagged", action="store_true")
    a = ap.parse_args()

    index, payload, codec = read_web(a.web)
    parts = index["parts"]

    if a.cmd == "report":
        report(parts); return
    if a.cmd == "derive":
        n = derive(parts)
        print(f"derived 4 fields on {n} parts")
    else:
        if "donor" not in (parts[0] if parts else {}):
            print("running derive first — tag needs it for context\n")
            derive(parts)
        n = tag(parts, a.n, a.name, a.untagged)
        print(f"\ntagged {n} part(s)")
    dest = a.o or a.web
    sz = write_web(dest, index, payload, codec)
    print(f"wrote {dest} ({sz/1e6:.1f} MB)")


if __name__ == "__main__":
    main()
