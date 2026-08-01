#!/usr/bin/env python3
"""analyze_models.py — run the body-plan analysis over the imported model table
and emit a C header the engine can read.

Without this, `ex_model_transforms()` in game.c has no data and falls back to a
proxy. This is what makes the generalizer a corpus-wide tool rather than a
per-model one: every imported model gets a verdict, computed once offline,
available in-game at zero cost.

Per model it records:

* **up axis** — measured, never assumed. Brawl's tank is longest in Y and his
  robot in Z; assuming a convention is what broke the earlier fold work.
* **mirror axis and paired fraction** — the body plan. 74–85% on Brawl, the t4b
  mech and scorponok; 3% on the hovertank.
* **monolithic** — the largest component's share. The hovertank is 65% one
  welded hull, which is a different problem from being asymmetric and needs a
  different fix.
* **unwelded** — every triangle carrying private vertices. This invalidates the
  component and openness figures, so it is recorded first and checked first.
* **open fraction** — hollow shells need the mirror flag on texture transfer or
  the texture reads backwards on one side.
* **has UVs** — 586 of the corpus do, 547 do not.

Regenerate:  python3 tools/analyze_models.py
"""
import os, sys, struct, array, math, re

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, 'game-model-tools', 'python'))
from transform_collage import load_iqm, weld, components, bbox

MIRROR_TOL = 0.04
MIN_PART = 40

def has_uv(path):
    b = open(path, 'rb').read(4096)
    if b[:15] != b'INTERQUAKEMODEL': return False
    g = lambda o: struct.unpack_from('<I', b, o)[0]
    nva, nv, ofs = g(44), g(48), g(52)
    full = open(path, 'rb').read()
    for i in range(nva):
        e = ofs + i * 20
        if struct.unpack_from('<I', full, e)[0] == 1:
            size, off = struct.unpack_from('<I', full, e+12)[0], struct.unpack_from('<I', full, e+16)[0]
            a = array.array('f'); a.frombytes(full[off:off + nv*size*4])
            us = a[0::size]
            return (max(us) - min(us)) > 0.01 if len(us) else False
    return False

def analyse(path):
    try:
        v, t = load_iqm(path)
    except Exception:
        return None
    if not v or len(v) < 12 or not t:
        return None
    lo, hi = bbox(v); span = [hi[i]-lo[i] for i in range(3)]
    if max(span) <= 0: return None

    edges = {}
    for i in range(0, len(t), 3):
        a, b, c = t[i], t[i+1], t[i+2]
        for u, w in ((a,b),(b,c),(c,a)):
            k = (min(u,w), max(u,w)); edges[k] = edges.get(k, 0) + 1
    openf = sum(1 for n in edges.values() if n == 1) / max(len(edges), 1)
    unwelded = openf > 0.95

    comps = [c for c in components(len(v), t, weld(v, max(span)*1e-5)) if len(c) >= 12]
    if not comps: return None
    big = max(len(c) for c in comps)

    parts = []
    for c in comps:
        if len(c) < MIN_PART: continue
        pts = [v[k] for k in c]; l, h = bbox(pts)
        parts.append((len(c), [(l[j]+h[j])/2 for j in range(3)], [h[j]-l[j] for j in range(3)]))

    pairs, pv = 0, 0
    lat = 0
    tv = sum(p[0] for p in parts) or 1
    if parts:
        scale = max(span)
        def count(ax):
            mid = sum(p[1][ax] for p in parts)/len(parts)
            used = [False]*len(parts); n = 0; s = 0
            order = sorted(range(len(parts)), key=lambda i: -parts[i][0])
            for i in order:
                if used[i]: continue
                va, ca, ea = parts[i]; off = ca[ax]-mid
                for j in order:
                    if used[j] or j == i or parts[j][0] != va: continue
                    vb, cb, eb = parts[j]; bo = cb[ax]-mid
                    if bo*off > 0: continue
                    d = (abs(bo+off) + sum(abs(ca[k]-cb[k]) for k in range(3) if k != ax)
                         + sum(abs(ea[k]-eb[k]) for k in range(3))) / scale
                    if d <= MIRROR_TOL:
                        used[i] = used[j] = True; n += 1; s += va*2; break
            return n, s
        lat = max(range(3), key=lambda a: count(a)[0])
        pairs, pv = count(lat)

    up = max(range(3), key=lambda i: span[i])
    return dict(up=up, lat=lat, pairs=pairs, paired=pv/tv, comps=len(comps),
                mono=big/len(v), openf=openf, unwelded=unwelded,
                uv=has_uv(path), verts=len(v))

def verdict_code(r):
    """0 unusable, 1 weld first, 2 monolithic, 3 no plan, 4 usable.

    Ordered by which finding invalidates the others: an unwelded mesh makes the
    component and openness figures meaningless, so it outranks everything; a
    monolithic model has no parts to pair, so it outranks the symmetry verdict.
    """
    if r is None: return 0
    if r['unwelded']: return 1
    if r['mono'] > 0.5: return 2
    if r['paired'] < 0.25: return 3
    return 4

def main():
    hdr = os.path.join(ROOT, 'engine_src', 'imported_models.h')
    src = open(hdr).read()
    files = re.findall(r'"(data/[^"]+\.iqm)"', src)
    if not files:
        print("no model list found in imported_models.h"); return 1
    limit = int(sys.argv[1]) if len(sys.argv) > 1 else len(files)
    files = files[:limit]
    print(f"analysing {len(files)} models…", file=sys.stderr)

    rows = []
    for i, f in enumerate(files):
        p = os.path.join(ROOT, 'engine_src', f)
        r = analyse(p) if os.path.exists(p) else None
        rows.append(r)
        if (i+1) % 50 == 0: print(f"  {i+1}/{len(files)}", file=sys.stderr)

    out = os.path.join(ROOT, 'engine_src', 'model_analysis.h')
    with open(out, 'w') as fh:
        fh.write("// AUTO-GENERATED by tools/analyze_models.py -- DO NOT EDIT BY HAND.\n")
        fh.write("// Regenerate:  python3 tools/analyze_models.py\n")
        fh.write("//\n// Per-model body-plan analysis. verdict: 0 unreadable, 1 weld first,\n")
        fh.write("// 2 monolithic (one welded hull), 3 no usable symmetry, 4 usable.\n")
        fh.write("// up/lat are axis indices 0=X 1=Y 2=Z, MEASURED per model -- Brawl's tank\n")
        fh.write("// is longest in Y and his robot in Z, so a convention cannot be assumed.\n\n")
        fh.write("typedef struct { unsigned char verdict, up, lat, flags; "
                 "unsigned short pairs; float paired; } ex_model_plan_t;\n")
        fh.write("#define EX_PLAN_UNWELDED 1u\n#define EX_PLAN_OPEN     2u\n#define EX_PLAN_HASUV    4u\n\n")
        fh.write(f"static const ex_model_plan_t g_model_plan[{len(rows)}] = {{\n")
        for r in rows:
            if r is None:
                fh.write("  {0,1,0,0,0,0.0f},\n"); continue
            flags = (1 if r['unwelded'] else 0) | (2 if r['openf'] > 0.05 else 0) | (4 if r['uv'] else 0)
            fh.write(f"  {{{verdict_code(r)},{r['up']},{r['lat']},{flags},"
                     f"{min(r['pairs'],65535)},{r['paired']:.3f}f}},\n")
        fh.write("};\n")

    counts = {}
    for r in rows:
        counts[verdict_code(r)] = counts.get(verdict_code(r), 0) + 1
    names = {0:'unreadable',1:'weld first',2:'monolithic',3:'no plan',4:'USABLE'}
    print(f"\n-> engine_src/model_analysis.h")
    for k in sorted(counts): print(f"  {names[k]:12} {counts[k]:5}")
    return 0

if __name__ == '__main__':
    sys.exit(main())
