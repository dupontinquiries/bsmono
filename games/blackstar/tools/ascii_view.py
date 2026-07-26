#!/usr/bin/env python3
"""ascii_view.py -- look at a model without a display.

Every visual fault in this project so far -- models lying on their backs, limbs
detached, greebles reading as scattered debris, ribs sealed inside an opaque chest
-- was invisible to the numeric checks and only surfaced when a human looked. This
closes part of that loop: render the silhouette as depth-shaded text, which is
crude but enough to answer "is it upright", "are the limbs attached", "does it read
as a humanoid".

Applies the same orientation (rx/ry/rz) and scale the engine will use, taken from
model_tuning.json when available, so what it prints is what the game shows rather
than the raw asset.

Usage:
  python3 tools/ascii_view.py --target MODEL.gltf
  python3 tools/ascii_view.py --parts kit/imported_x_parts.txt --tuning model_tuning.json
  python3 tools/ascii_view.py --target M.gltf --views front,side,top --width 60
"""
import sys, os, json, math, argparse

RAMP = " .:-=+*#%@"          # near -> far becomes dark -> light

def _helpers():
    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path: sys.path.insert(0, here)
    import importlib.util, glob, re
    def imp(stem):
        try: return __import__(stem)
        except Exception: pass
        best, bv = None, -1
        for c in glob.glob(os.path.join(here, stem + "*.py")):
            b = os.path.basename(c)[:-3]
            m = re.search(r"_v(\d+)$", b); v = int(m.group(1)) if m else 0
            if v > bv: bv, best = v, b
        if not best: return None
        sp = importlib.util.spec_from_file_location(best, os.path.join(here, best + ".py"))
        mod = importlib.util.module_from_spec(sp); sys.modules[best] = mod
        sp.loader.exec_module(mod); return mod
    return imp("process_models"), imp("prop_view")

def rotate(V, rx, ry, rz):
    import numpy as np
    P = V.copy()
    for ax, deg in ((1, ry), (0, rx), (2, rz)):      # engine order: Y, X, Z
        if abs(deg) < 1e-6: continue
        a = math.radians(deg); c, s = math.cos(a), math.sin(a)
        i, j = [(1, 2), (2, 0), (0, 1)][ax]
        x, y = P[:, i].copy(), P[:, j].copy()
        P[:, i] = x * c - y * s
        P[:, j] = x * s + y * c
    return P

def render(V, F, width=54, view="front", aspect=0.5):
    """Depth-shaded silhouette. Rasterises triangle centroids -- enough to judge
    form, far cheaper than scan conversion."""
    import numpy as np
    if len(F) == 0: return ["(no geometry)"]
    axes = {"front": (0, 1, 2), "side": (2, 1, 0), "top": (0, 2, 1)}[view]
    u, v, w = axes
    tri = V[F]; c = tri.mean(1)
    r = np.max(np.abs(tri - c[:, None, :]), axis=(1, 2))
    lo, hi = V.min(0), V.max(0)
    span = np.maximum(hi - lo, 1e-6)
    H = max(6, int(width * (span[v] / span[u]) * aspect))
    img = np.full((H, width), -1e9, "f4")
    pu = (c[:, u] - lo[u]) / span[u]
    pv = 1.0 - (c[:, v] - lo[v]) / span[v]
    pw = (c[:, w] - lo[w]) / span[w]
    if view == "top": pw = 1.0 - pw
    rad = np.maximum(0, (r / span[u] * width * 0.5).astype(int))
    for i in np.argsort(pw):
        x = int(pu[i] * (width - 1)); y = int(pv[i] * (H - 1)); k = int(rad[i])
        x0, x1 = max(0, x - k), min(width, x + k + 1)
        y0, y1 = max(0, y - k), min(H, y + k + 1)
        np.maximum(img[y0:y1, x0:x1], pw[i], out=img[y0:y1, x0:x1])
    out = []
    for row in img:
        line = "".join(" " if q < -1e8 else RAMP[min(len(RAMP) - 1,
                       int((1.0 - q) * (len(RAMP) - 1)))] for q in row)
        out.append(line.rstrip())
    return [l for l in out] or ["(empty)"]

def load_parts(manifest, pm, pv):
    """Every part of a multi-part model, re-applying each part's pivot offset."""
    import numpy as np
    d = os.path.dirname(manifest)
    Vs, Fs, off = [], [], 0
    for line in open(manifest, errors="ignore"):
        p = line.split()
        if len(p) < 2: continue
        f = os.path.join(d, p[0])
        if not os.path.exists(f): continue
        try:
            v, n, t = pv.load_iqm(f)
        except Exception:
            continue
        V = np.array(v, "f4"); F = np.array(t, "u4")
        if len(p) > 7:
            try: V = V + np.array([float(p[5]), float(p[6]), float(p[7])], "f4")
            except ValueError: pass
        Vs.append(V); Fs.append(F + off); off += len(V)
    if not Vs: return None, None
    return np.vstack(Vs).astype("f4"), np.vstack(Fs).astype("u4")

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", default=None, help="a .gltf/.glb/.obj model")
    ap.add_argument("--parts", default=None, help="a *_parts.txt manifest")
    ap.add_argument("--tuning", default=None, help="model_tuning.json, to apply the engine's orientation")
    ap.add_argument("--name", default=None, help="tuning key (defaults to the folder/file name)")
    ap.add_argument("--views", default="front,side")
    ap.add_argument("--width", type=int, default=54)
    a = ap.parse_args()

    pm, pv = _helpers()
    if not pm: raise SystemExit("need process_models.py beside this tool")

    if a.parts:
        if not pv: raise SystemExit("need prop_view.py to read part IQMs")
        V, F = load_parts(a.parts, pm, pv)
        name = a.name or os.path.basename(a.parts).replace("imported_", "").replace("_parts.txt", "")
        if V is None: raise SystemExit("no readable parts in " + a.parts)
    elif a.target:
        if not os.path.isfile(a.target):
            print("ERROR: not found: %r" % a.target); sys.exit(2)
        geo = pm.load_geometry(a.target)
        if not geo: raise SystemExit("could not read " + a.target)
        V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
        name = a.name or os.path.basename(os.path.dirname(os.path.abspath(a.target)))
    else:
        print("give --target MODEL or --parts MANIFEST"); sys.exit(1)

    lo, hi = V.min(0), V.max(0)
    V = ((V - (lo + hi) * 0.5) / max(1e-6, float((hi - lo).max())) * 2.0).astype("f4")

    rx = ry = rz = 0.0
    if a.tuning and os.path.isfile(a.tuning):
        t = json.load(open(a.tuning))
        key = None
        nk = "".join(ch for ch in name.lower() if ch.isalnum())
        best = 0
        for k in t:
            kk = "".join(ch for ch in k.lower() if ch.isalnum())
            if (kk in nk or nk in kk) and len(kk) > best:
                key, best = k, len(kk)
        if key:
            e = t[key]; rx, ry, rz = e.get("rx", 0), e.get("ry", 0), e.get("rz", 0)
            print("applying tuning from %r: rx=%.0f ry=%.0f rz=%.0f" % (key, rx, ry, rz))
    Vr = rotate(V, rx, ry, rz)

    ext = Vr.max(0) - Vr.min(0)
    tall = int(np.argmax(ext))
    # A T-posed humanoid is legitimately widest in X (arm span), so X being
    # tallest is not evidence of anything. Z being tallest while Y is short means
    # the model is lying down -- that is the "flipped on their backs" fault, and
    # it is checkable rather than something only a human notices.
    # A lying-down model has height as its SMALLEST dimension by a clear margin.
    # Simply "not tallest in Y" flags every T-posed bot, whose arm span legitimately
    # exceeds its height -- and maximising height then stands them on an arm.
    flat = (ext[1] == ext.min()) and (ext[1] < 0.75 * max(ext[0], ext[2]))
    if flat:
        verdict = "!! LYING DOWN -- height %.2f is the smallest dimension" % ext[1]
    elif tall == 1:
        verdict = "UPRIGHT (tallest axis is Y)"
    else:
        verdict = "upright (widest in %s = arm span or bulk)" % "XYZ"[tall]
    print("\n%s   %d tris   extent x%.2f y%.2f z%.2f   %s"
          % (name, len(F), ext[0], ext[1], ext[2], verdict))
    if flat:
        # only suggest a turn that makes height no longer the smallest dimension
        best, best_h = None, ext[1]
        for cand in (90, -90, 180):
            e2 = rotate(V, rx + cand, ry, rz)
            g = e2.max(0) - e2.min(0)
            if float(g[1]) > best_h and float(g[1]) > g.min() * 1.15:
                best, best_h = cand, float(g[1])
        if best is not None:
            print("   suggested fix: rx = %+d  (height %.2f -> %.2f)"
                  % (rx + best, ext[1], best_h))
            print("   set it in model_tuning.json under a key matching %r" % name)
    for view in a.views.split(","):
        view = view.strip()
        if view not in ("front", "side", "top"): continue
        print("\n  [%s]" % view)
        for line in render(Vr, F, a.width, view):
            print("    " + line)

if __name__ == "__main__":
    main()
