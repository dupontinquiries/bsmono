#!/usr/bin/env python3
"""Score a scene render against the rules in SCENE_RULES.md.

Usage: score_render.py <image.png> [--rules SCENE_RULES.md] [--json]
Prints a one-line summary, or full JSON with --json. Exit 0 always (scoring is advisory).
The committed scene code is the baseline; this only reads images. See SCENE_RULES.md.
"""
import sys, os, json, re
import numpy as np
from PIL import Image
try:
    import cv2
    HAVE_CV2 = True
except Exception:
    HAVE_CV2 = False

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_RULES = os.path.join(HERE, "..", "SCENE_RULES.md")


def load_cfg(path):
    txt = open(path).read()
    m = re.search(r"```json\s*(\{.*?\})\s*```", txt, re.S)
    return json.loads(m.group(1))


def _scene_crop_bgr(path):
    """Load an image, crop UI chrome, return an HxWx3 uint8 BGR array (cv2 order)."""
    img = Image.open(path).convert("RGB")
    a = np.asarray(img)
    H = a.shape[0]
    a = a[int(H * 0.03):int(H * 0.92)]            # drop toolbar + status bar
    return a[:, :, ::-1].copy()                    # RGB -> BGR


def ref_similarity(cand_path, ref_path):
    """Framing-tolerant similarity of a candidate render to a reference image, via three cues:
       color (HSV histogram), layout (low-res value correlation), depth (row-luminance/atmospheric profile).
       Returns (combined 0..1, {color, layout, depth})."""
    c = _scene_crop_bgr(cand_path)
    r = _scene_crop_bgr(ref_path)
    if HAVE_CV2:
        # ---- color: 2D Hue-Sat histogram correlation ----
        ch = cv2.cvtColor(c, cv2.COLOR_BGR2HSV); rh = cv2.cvtColor(r, cv2.COLOR_BGR2HSV)
        hc = cv2.calcHist([ch], [0, 1], None, [32, 32], [0, 180, 0, 256]); cv2.normalize(hc, hc, 0, 1, cv2.NORM_MINMAX)
        hr = cv2.calcHist([rh], [0, 1], None, [32, 32], [0, 180, 0, 256]); cv2.normalize(hr, hr, 0, 1, cv2.NORM_MINMAX)
        color = float(max(0.0, cv2.compareHist(hc, hr, cv2.HISTCMP_CORREL)))
        # ---- layout: 64x48 grayscale normalized cross-correlation ----
        gc = cv2.resize(cv2.cvtColor(c, cv2.COLOR_BGR2GRAY), (64, 48)).astype(np.float32)
        gr = cv2.resize(cv2.cvtColor(r, cv2.COLOR_BGR2GRAY), (64, 48)).astype(np.float32)
    else:
        ch = c.astype(np.float32); rh = r.astype(np.float32)
        color = 0.5  # neutral if cv2 missing
        gc = np.asarray(Image.fromarray(c[:, :, ::-1]).convert("L").resize((64, 48))).astype(np.float32)
        gr = np.asarray(Image.fromarray(r[:, :, ::-1]).convert("L").resize((64, 48))).astype(np.float32)
    gc -= gc.mean(); gr -= gr.mean()
    denom = (np.linalg.norm(gc) * np.linalg.norm(gr)) + 1e-6
    layout = float(max(0.0, (gc * gr).sum() / denom))
    # ---- depth/atmosphere: per-row mean-luminance profile correlation (vertical value layering) ----
    def rowprof(x):
        g = x.mean(2) if x.ndim == 3 else x
        g = np.asarray(Image.fromarray(g.astype(np.uint8)).resize((1, 64))).astype(np.float32).ravel()
        g -= g.mean(); n = np.linalg.norm(g)
        return g / n if n > 1e-6 else g
    pc = rowprof(c[:, :, ::-1]); pr = rowprof(r[:, :, ::-1])
    depth = float(max(0.0, (pc * pr).sum()))
    combined = 0.70 * color + 0.20 * layout + 0.10 * depth  # color is the cue that transfers across render styles
    return combined, {"color": color, "layout": layout, "depth": depth}


def pixel_match(cand_path, ref_path):
    """TRUE pixel-level match for an ALIGNED world reference (same view): global SSIM + (1-MSE) on a common-size
    grayscale. Only meaningful when candidate and reference are the same framing (e.g. a target world screenshot or
    a golden render for regression). For unaligned concept art use ref_similarity (color) instead."""
    c = _scene_crop_bgr(cand_path); r = _scene_crop_bgr(ref_path)
    if HAVE_CV2:
        gc = cv2.resize(cv2.cvtColor(c, cv2.COLOR_BGR2GRAY), (160, 120)).astype(np.float32) / 255.0
        gr = cv2.resize(cv2.cvtColor(r, cv2.COLOR_BGR2GRAY), (160, 120)).astype(np.float32) / 255.0
    else:
        gc = np.asarray(Image.fromarray(c[:, :, ::-1]).convert("L").resize((160, 120))).astype(np.float32) / 255.0
        gr = np.asarray(Image.fromarray(r[:, :, ::-1]).convert("L").resize((160, 120))).astype(np.float32) / 255.0
    mse = float(((gc - gr) ** 2).mean())
    mu_a, mu_b = gc.mean(), gr.mean(); va, vb = gc.var(), gr.var()
    cov = float(((gc - mu_a) * (gr - mu_b)).mean()); c1, c2 = 0.01 ** 2, 0.03 ** 2
    ssim = float(((2 * mu_a * mu_b + c1) * (2 * cov + c2)) / ((mu_a ** 2 + mu_b ** 2 + c1) * (va + vb + c2)))
    combined = max(0.0, 0.6 * ssim + 0.4 * (1.0 - min(1.0, mse * 6.0)))
    return combined, {"ssim": ssim, "mse": mse}


def tri_band(x, lo, hi):
    """1.0 inside [lo,hi], linear falloff to 0 over one band-width outside."""
    w = max(hi - lo, 1e-6)
    if x < lo:
        return max(0.0, 1.0 - (lo - x) / w)
    if x > hi:
        return max(0.0, 1.0 - (x - hi) / w)
    return 1.0


def metrics(path, cfg):
    img = Image.open(path).convert("RGB")
    a = np.asarray(img).astype(np.float32) / 255.0
    H, W, _ = a.shape
    top, bot = int(H * 0.03), int(H * 0.92)          # crop UI chrome
    crop = a[top:bot]
    ch, cw, _ = crop.shape
    L = 0.2126 * crop[..., 0] + 0.7152 * crop[..., 1] + 0.0722 * crop[..., 2]
    mean = float(L.mean()); std = float(L.std())
    blow = float((L > 0.92).mean()); dark = float((L < 0.04).mean())
    # background = median of the two TOP corners of the full frame (sky/void)
    cw6, ch6 = int(W * 0.06), int(H * 0.06)
    corners = np.concatenate([a[:ch6, :cw6].reshape(-1, 3), a[:ch6, -cw6:].reshape(-1, 3)])
    bg = np.median(corners, axis=0)
    dist = np.linalg.norm(crop - bg, axis=2)
    covmask = dist > 0.10
    coverage = float(covmask.mean())
    # hue variety: saturation-weighted hue histogram entropy over covered pixels
    mx = crop.max(2); mn = crop.min(2); df = mx - mn
    sat = np.where(mx > 1e-6, df / (mx + 1e-6), 0.0)
    r, g, b = crop[..., 0], crop[..., 1], crop[..., 2]
    hue = np.zeros_like(mx)
    nz = df > 1e-6
    # vectorized hue (degrees/60 sectors → [0,1))
    with np.errstate(invalid="ignore", divide="ignore"):
        rc = (mx - r) / np.where(df > 0, df, 1)
        gc = (mx - g) / np.where(df > 0, df, 1)
        bc = (mx - b) / np.where(df > 0, df, 1)
        h = np.where(mx == r, bc - gc, np.where(mx == g, 2.0 + rc - bc, 4.0 + gc - rc))
    hue = (h / 6.0) % 1.0
    sel = covmask & (sat > 0.12)
    if sel.sum() > 50:
        hist, _ = np.histogram(hue[sel], bins=12, range=(0, 1), weights=sat[sel])
        p = hist / (hist.sum() + 1e-9)
        p = p[p > 0]
        hue_entropy = float(-(p * np.log(p)).sum())
    else:
        hue_entropy = 0.0
    # structure: mean gradient magnitude of luminance
    gy = np.abs(np.diff(L, axis=0)); gx = np.abs(np.diff(L, axis=1))
    structure = float((gy.mean() + gx.mean()) / 2.0)
    # vertical spread of covered pixels (height usage)
    ys = np.where(covmask)[0]
    vspread = float(ys.std() / ch) if ys.size else 0.0
    return dict(mean=mean, std=std, blow=blow, dark=dark, coverage=coverage,
                hue_entropy=hue_entropy, structure=structure, vspread=vspread)


def score(m, cfg):
    w = cfg["weights"]; b = cfg["bands"]
    s = {}
    s["exposure"] = tri_band(m["mean"], *b["exposure"])
    s["not_blown"] = 1.0 if m["blow"] <= b["blow_lo"] else max(0.0, 1.0 - (m["blow"] - b["blow_lo"]) / (b["blow_hi"] - b["blow_lo"]))
    s["contrast"] = min(1.0, m["std"] / b["contrast_cap"])
    s["coverage"] = tri_band(m["coverage"], *b["coverage"])
    s["hue"] = tri_band(m["hue_entropy"], *b["hue_entropy"])
    s["structure"] = min(1.0, m["structure"] / b["structure_cap"])
    s["vspread"] = min(1.0, m["vspread"] / b["vspread_cap"])
    total = sum(w[k] * s[k] for k in w)
    # hard fails
    h = cfg["hard"]; note = ""
    if m["mean"] < h["black_meanL"]:
        total = 0.0; note = "BLACK"
    elif m["coverage"] < h["void_cov"]:
        total *= 0.3; note = "VOID"
    elif m["blow"] > h["washout_blow"]:
        total *= 0.4; note = "WASHOUT"
    return total, s, note


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    as_json = "--json" in sys.argv
    rules = DEFAULT_RULES
    if "--rules" in sys.argv:
        rules = sys.argv[sys.argv.index("--rules") + 1]
    ref = sys.argv[sys.argv.index("--ref") + 1] if "--ref" in sys.argv else None
    ref_w = float(sys.argv[sys.argv.index("--ref-weight") + 1]) if "--ref-weight" in sys.argv else 0.3
    cfg = load_cfg(rules)
    path = args[0]
    m = metrics(path, cfg)
    rule_total, sub, note = score(m, cfg)
    total = rule_total
    refinfo = None
    if ref and os.path.exists(ref):
        if "--pixel" in sys.argv:
            ref_sim, refsub = pixel_match(path, ref)        # aligned world reference (SSIM/MSE)
        else:
            ref_sim, refsub = ref_similarity(path, ref)     # unaligned concept art (color/layout/depth)
        # a hard-failed render (black/void/washout) can't be rescued by a good match
        gate = 1.0 if note == "" else 0.4
        total = ((1.0 - ref_w) * rule_total + ref_w * ref_sim) * (1.0 if note == "" else gate)
        refinfo = {"ref_sim": round(ref_sim, 3), **{k: round(v, 3) for k, v in refsub.items()},
                   "ref": os.path.basename(ref)}
    if as_json:
        out = dict(score=round(total, 4), rule_score=round(rule_total, 4),
                   sub={k: round(v, 3) for k, v in sub.items()},
                   metrics={k: round(v, 4) for k, v in m.items()}, note=note)
        if refinfo:
            out["ref"] = refinfo
        print(json.dumps(out))
    else:
        parts = " ".join(f"{k}={sub[k]:.2f}" for k in sub)
        rtxt = ""
        if refinfo:
            extra = " ".join(f"{k}{refinfo[k]:.2f}" for k in refinfo if k not in ("ref_sim", "ref"))
            rtxt = f"  REF={refinfo['ref_sim']:.2f}({extra})"
        print(f"{total:.4f} [{note}] rule={rule_total:.2f}{rtxt}  {parts}  (cov={m['coverage']:.2f} L={m['mean']:.2f} blow={m['blow']:.2f})")


if __name__ == "__main__":
    main()
