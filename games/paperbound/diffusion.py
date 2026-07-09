"""
PAPERBOUND — tiny diffusion.

A genuinely small **denoising diffusion probabilistic model** (DDPM) implemented
from scratch in pure numpy. It is *not* Stable Diffusion — real SD is a multi-GB
PyTorch model needing a GPU and HuggingFace weights, which is the opposite of
portable on a postmarketOS phone. This is the same generative *paradigm* (forward
noising + learned reverse denoising) shrunk to something that runs on a phone
CPU in a fraction of a second and ships as a few-KB weight file.

What it's good for: small, *textural / abstract* assets where "evocative, not
literal" is exactly right — VFX sprites (ember bursts, ink splashes, frost, aura,
smoke) and map surface washes. It is deliberately NOT used to draw creatures —
the origami part engine already does that far better. Right tool per job.

Design:
  • 16×16 RGB tiles (tiny → fast; a full reverse diffusion is ~a few ms).
  • epsilon-prediction DDPM with a cosine noise schedule.
  • denoiser = a per-pixel MLP over a 3×3 unfold (local context) + coordinates +
    a sinusoidal timestep embedding + a one-hot CONCEPT code. Sharing the MLP
    across pixels makes it a (tiny) convolutional denoiser with easy backprop.
  • trains in seconds on procedurally-generated concept targets, then the ~10k
    parameters are baked to diffusion_weights.npz and shipped.

Optional: SDBridge talks to a local Stable Diffusion HTTP server (AUTOMATIC1111
/ ComfyUI / sd.cpp) if the user has one — mirroring the LLM bridge. The numpy
model is always the fallback so the feature works with zero setup.

Public surface:
    m = TinyDiffusion.load_or_train()
    tile = m.generate("ember", size=16)         # -> (H,W,3) uint8
    TinyDiffusion... (train()/save()/load())
    SDBridge(cfg).txt2img(prompt, size)          # optional external SD
"""
import os, json, math
import numpy as np

CONCEPTS = ["paper", "ink", "ember", "frost", "aura", "smoke"]
IMG = 16                      # tile size (px); tiny on purpose
T = 24                        # diffusion steps
_WEIGHTS = os.path.join(os.path.dirname(__file__), "diffusion_weights.npz")


# ─────────────────────────── noise schedule (cosine) ──────────────────────
def _cosine_alpha_bar(T, s=0.008):
    steps = np.arange(T + 1)
    f = np.cos(((steps / T) + s) / (1 + s) * math.pi / 2) ** 2
    ab = f / f[0]
    return np.clip(ab, 1e-4, 0.9999)         # alpha_bar[t]


_ALPHA_BAR = _cosine_alpha_bar(T)
_BETAS = np.clip(1 - _ALPHA_BAR[1:] / _ALPHA_BAR[:-1], 1e-4, 0.999)
_ALPHAS = 1.0 - _BETAS
_ALPHA_BAR_T = np.cumprod(_ALPHAS)           # per-step cumulative


# ───────────────────────── procedural concept targets ─────────────────────
def _concept_target(concept, rng):
    """A small procedurally-drawn 16×16 example of a concept — the training data.
    Abstract/textural on purpose (that's what a tiny diffusion can learn well)."""
    im = np.zeros((IMG, IMG, 3), np.float32)
    yy, xx = np.mgrid[0:IMG, 0:IMG].astype(np.float32)
    cx, cy = rng.uniform(5, 11), rng.uniform(5, 11)
    d = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
    if concept == "paper":
        base = np.array([0.78, 0.72, 0.58]) + rng.uniform(-0.05, 0.05, 3)
        im[:] = base
        im += rng.normal(0, 0.05, (IMG, IMG, 1))              # grain
        for _ in range(rng.randint(2, 5)):                    # fibre streaks
            ry = rng.randint(0, IMG); im[ry:ry+1] *= rng.uniform(0.9, 1.05)
    elif concept == "ink":
        im[:] = np.array([0.82, 0.78, 0.66])
        blot = np.exp(-(d / rng.uniform(3, 5)) ** 2)
        im *= (1 - blot[..., None] * rng.uniform(0.7, 0.95))
        for _ in range(rng.randint(1, 4)):                    # spatter
            sx, sy = rng.randint(0, IMG), rng.randint(0, IMG)
            im[sy, sx] *= 0.2
    elif concept == "ember":
        glow = np.exp(-(d / rng.uniform(2.5, 4.5)) ** 2)
        col = np.array([1.0, 0.55, 0.18])
        im[:] = (col[None, None] * glow[..., None]) * rng.uniform(0.8, 1.1)
        for _ in range(rng.randint(3, 8)):                    # sparks
            sx, sy = rng.randint(0, IMG), rng.randint(0, IMG)
            im[sy, sx] = [1.0, rng.uniform(0.7, 0.95), 0.4]
    elif concept == "frost":
        glow = np.exp(-(d / rng.uniform(3, 5)) ** 2)
        col = np.array([0.72, 0.86, 1.0])
        im[:] = col[None, None] * (0.4 + 0.6 * glow[..., None])
        for _ in range(rng.randint(2, 5)):                    # crystal spokes
            ang = rng.uniform(0, math.pi); 
            for r in range(0, 7):
                px, py = int(cx + math.cos(ang) * r), int(cy + math.sin(ang) * r)
                if 0 <= px < IMG and 0 <= py < IMG: im[py, px] = [0.95, 0.98, 1.0]
    elif concept == "aura":
        hue = rng.uniform(0, 1)
        col = 0.5 + 0.5 * np.array([math.sin(hue*6.28), math.sin(hue*6.28+2), math.sin(hue*6.28+4)])
        glow = np.exp(-(d / rng.uniform(3.5, 6)) ** 2)
        im[:] = col[None, None] * glow[..., None] * rng.uniform(0.8, 1.1)
    else:  # smoke
        base = rng.uniform(0.35, 0.6)
        im[:] = base
        for _ in range(rng.randint(3, 6)):
            bx, by, br = rng.uniform(2, 14), rng.uniform(2, 14), rng.uniform(2, 5)
            b = np.exp(-(((xx - bx) ** 2 + (yy - by) ** 2) / (2 * br ** 2)))
            im += b[..., None] * rng.uniform(-0.2, 0.2)
    return np.clip(im, 0, 1)


# ───────────────────────────── the tiny model ─────────────────────────────
def _time_embed(t, dim=4):
    """sinusoidal timestep embedding, dim features."""
    t = float(t) / T
    return np.array([math.sin(t * math.pi), math.cos(t * math.pi),
                     math.sin(t * 2 * math.pi), math.cos(t * 2 * math.pi)][:dim], np.float32)


def _unfold(x):
    """3×3 neighbourhood unfold of an (H,W,3) image → (H*W, 27) with edge pad."""
    p = np.pad(x, ((1, 1), (1, 1), (0, 0)), mode="edge")
    patches = np.empty((IMG, IMG, 27), np.float32)
    k = 0
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            patches[..., k*3:k*3+3] = p[1+dy:1+dy+IMG, 1+dx:1+dx+IMG, :]
            k += 1
    return patches.reshape(IMG * IMG, 27)


# per-pixel coordinate features (fixed)
_YY, _XX = np.mgrid[0:IMG, 0:IMG].astype(np.float32)
_COORD = np.stack([(_XX / (IMG - 1) - 0.5), (_YY / (IMG - 1) - 0.5)], -1).reshape(IMG*IMG, 2)

_IN = 27 + 2 + 4 + len(CONCEPTS)     # unfold + coord + time + concept one-hot
_H1, _H2 = 48, 48


class TinyDiffusion:
    def __init__(self, seed=0):
        r = np.random.RandomState(seed)
        def he(a, b): return (r.randn(a, b) * math.sqrt(2.0 / a)).astype(np.float32)
        self.W1 = he(_IN, _H1); self.b1 = np.zeros(_H1, np.float32)
        self.W2 = he(_H1, _H2); self.b2 = np.zeros(_H2, np.float32)
        self.W3 = (he(_H2, 3) * 0.1); self.b3 = np.zeros(3, np.float32)

    # -- forward: predict epsilon for a noised image at step t, given concept --
    def _features(self, x, t, concept_idx):
        feat = np.concatenate([
            _unfold(x), _COORD,
            np.tile(_time_embed(t), (IMG*IMG, 1)),
            np.tile(np.eye(len(CONCEPTS), dtype=np.float32)[concept_idx], (IMG*IMG, 1)),
        ], axis=1)
        return feat

    def _forward(self, feat):
        z1 = feat @ self.W1 + self.b1; a1 = np.maximum(z1, 0)
        z2 = a1 @ self.W2 + self.b2; a2 = np.maximum(z2, 0)
        out = a2 @ self.W3 + self.b3
        return out, (feat, z1, a1, z2, a2)

    def eps(self, x, t, concept_idx):
        out, _ = self._forward(self._features(x, t, concept_idx))
        return out.reshape(IMG, IMG, 3)

    # -- training (Adam) --------------------------------------------------
    def train(self, iters=1400, batch=24, lr=2e-3, seed=1, log=False):
        rng = np.random.RandomState(seed)
        params = [self.W1, self.b1, self.W2, self.b2, self.W3, self.b3]
        m = [np.zeros_like(p) for p in params]; v = [np.zeros_like(p) for p in params]
        b1, b2, eps = 0.9, 0.999, 1e-8
        for it in range(1, iters + 1):
            gW1 = gW2 = gW3 = None
            gb1 = gb2 = gb3 = None
            loss = 0.0
            # accumulate grads over a small batch of (concept, t) samples
            accum = [np.zeros_like(p) for p in params]
            for _ in range(batch):
                ci = rng.randint(len(CONCEPTS))
                x0 = _concept_target(CONCEPTS[ci], rng) * 2 - 1     # to [-1,1]
                t = rng.randint(T)
                ab = _ALPHA_BAR_T[t]
                noise = rng.randn(IMG, IMG, 3).astype(np.float32)
                xt = math.sqrt(ab) * x0 + math.sqrt(1 - ab) * noise
                feat = self._features(xt.astype(np.float32), t, ci)
                out, cache = self._forward(feat)
                target = noise.reshape(IMG*IMG, 3)
                diff = (out - target)
                loss += float(np.mean(diff ** 2))
                # backprop (MSE) through the 3-layer MLP
                (fe, z1, a1, z2, a2) = cache
                dout = (2.0 / (IMG*IMG)) * diff
                dW3 = a2.T @ dout; db3 = dout.sum(0)
                da2 = dout @ self.W3.T; dz2 = da2 * (z2 > 0)
                dW2 = a1.T @ dz2; db2 = dz2.sum(0)
                da1 = dz2 @ self.W2.T; dz1 = da1 * (z1 > 0)
                dW1 = fe.T @ dz1; db1 = dz1.sum(0)
                for i, g in enumerate([dW1, db1, dW2, db2, dW3, db3]):
                    accum[i] += g
            # Adam step on averaged grads
            for i, p in enumerate(params):
                g = accum[i] / batch
                m[i] = b1 * m[i] + (1 - b1) * g
                v[i] = b2 * v[i] + (1 - b2) * (g * g)
                mh = m[i] / (1 - b1 ** it); vh = v[i] / (1 - b2 ** it)
                p -= lr * mh / (np.sqrt(vh) + eps)
            if log and it % 200 == 0:
                print(f"  iter {it}/{iters} loss {loss/batch:.4f}")
        return self

    # -- sampling (ancestral DDPM) ---------------------------------------
    def generate(self, concept="ember", size=None, seed=None, steps=None):
        """Reverse-diffuse from noise to a tiny tile. Returns (H,W,3) uint8."""
        ci = CONCEPTS.index(concept) if concept in CONCEPTS else 0
        rng = np.random.RandomState(seed)
        x = rng.randn(IMG, IMG, 3).astype(np.float32)
        step_list = list(range(T - 1, -1, -1))
        if steps:                            # fewer steps = faster, coarser
            step_list = step_list[::max(1, T // steps)]
        for t in step_list:
            ab = _ALPHA_BAR_T[t]
            a = _ALPHAS[t]
            eps = self.eps(x, t, ci)
            # x0 estimate then posterior mean
            mean = (x - (1 - a) / math.sqrt(1 - ab) * eps) / math.sqrt(a)
            if t > 0:
                x = mean + math.sqrt(_BETAS[t]) * rng.randn(IMG, IMG, 3).astype(np.float32)
            else:
                x = mean
        img = np.clip((x + 1) / 2, 0, 1)
        if size and size != IMG:
            img = _resize(img, size)
        return (img * 255).astype(np.uint8)

    # -- persistence ------------------------------------------------------
    def save(self, path=_WEIGHTS):
        np.savez_compressed(path, W1=self.W1, b1=self.b1, W2=self.W2, b2=self.b2,
                            W3=self.W3, b3=self.b3)

    def load(self, path=_WEIGHTS):
        d = np.load(path)
        self.W1, self.b1 = d["W1"], d["b1"]; self.W2, self.b2 = d["W2"], d["b2"]
        self.W3, self.b3 = d["W3"], d["b3"]
        return self

    @classmethod
    def load_or_train(cls, path=_WEIGHTS):
        m = cls()
        if os.path.exists(path):
            try:
                return m.load(path)
            except Exception:
                pass
        return m.train().save() or m


def _resize(img, size):
    """nearest-ish bilinear upscale of a tiny (H,W,3) float image to size×size."""
    h, w = img.shape[:2]
    ys = (np.linspace(0, h - 1, size)).astype(np.float32)
    xs = (np.linspace(0, w - 1, size)).astype(np.float32)
    y0 = np.floor(ys).astype(int); x0 = np.floor(xs).astype(int)
    y1 = np.minimum(y0 + 1, h - 1); x1 = np.minimum(x0 + 1, w - 1)
    wy = (ys - y0)[:, None, None]; wx = (xs - x0)[None, :, None]
    a = img[y0][:, x0]; b = img[y0][:, x1]; c = img[y1][:, x0]; d = img[y1][:, x1]
    top = a * (1 - wx) + b * wx; bot = c * (1 - wx) + d * wx
    return top * (1 - wy) + bot * wy


# ─────────────────────── procedural fallback (always good) ─────────────────
def procedural_tile(concept="ember", size=16, seed=None):
    """A guaranteed-nice tile without the learned model — same generator the
    diffusion trained on, upscaled. Used when weights are missing or as a
    deterministic option."""
    import random as _r
    rng = np.random.RandomState(seed if seed is not None else _r.randint(0, 1 << 30))
    img = _concept_target(concept if concept in CONCEPTS else "ember", rng)
    if size != IMG:
        img = _resize(img, size)
    return (np.clip(img, 0, 1) * 255).astype(np.uint8)


# ───────────────────────── optional external SD bridge ────────────────────
class SDBridge:
    """Talk to a LOCAL Stable Diffusion HTTP server if the user runs one
    (AUTOMATIC1111 `/sdapi/v1/txt2img`, or a generic base). Requests tiny images
    to stay fast. Never required; the tiny numpy model is the fallback. Config via
    PAPERBOUND_SD_URL (base), PAPERBOUND_SD_STEPS."""
    def __init__(self, url=None, steps=None):
        self.url = url or os.environ.get("PAPERBOUND_SD_URL")
        self.steps = int(steps or os.environ.get("PAPERBOUND_SD_STEPS", 8))

    def available(self):
        return bool(self.url)

    def txt2img(self, prompt, size=64, timeout=30):
        """Returns (H,W,3) uint8 or None. Never raises."""
        if not self.available():
            return None
        try:
            import urllib.request, base64, io
            payload = {"prompt": prompt, "steps": self.steps,
                       "width": size, "height": size, "cfg_scale": 5,
                       "sampler_name": "Euler a"}
            req = urllib.request.Request(
                self.url.rstrip("/") + "/sdapi/v1/txt2img",
                data=json.dumps(payload).encode(),
                headers={"Content-Type": "application/json"}, method="POST")
            with urllib.request.urlopen(req, timeout=timeout) as r:
                d = json.loads(r.read().decode())
            b64 = d["images"][0]
            raw = base64.b64decode(b64.split(",", 1)[-1])
            try:
                from PIL import Image
                im = Image.open(io.BytesIO(raw)).convert("RGB")
                return np.array(im, np.uint8)
            except Exception:
                return None
        except Exception:
            return None


if __name__ == "__main__":
    import time
    t0 = time.time()
    m = TinyDiffusion().train(iters=1400, log=True)
    m.save()
    dt = time.time() - t0
    # generate a tile per concept, check it's not pure noise / not blank
    stats = {}
    gt0 = time.time()
    for c in CONCEPTS:
        tile = m.generate(c, size=16, seed=0)
        stats[c] = (round(float(tile.mean()), 1), round(float(tile.std()), 1))
    gdt = (time.time() - gt0) / len(CONCEPTS)
    # sanity: each concept differs from another (not mode-collapsed)
    a = m.generate("ember", seed=1).astype(float)
    b = m.generate("frost", seed=1).astype(float)
    diff = float(np.abs(a - b).mean())
    ok = all(4 < s[1] < 120 for s in stats.values()) and diff > 8
    print(f"train {dt:.1f}s | gen {gdt*1000:.1f}ms/tile | concept stats {stats}")
    print(f"ember-vs-frost mean abs diff {diff:.1f} (distinct: {diff>8})")
    print(f"{'✅' if ok else '❌'} tiny_diffusion self-test: trained + samples look "
          f"textural & concept-distinct, weights saved ({os.path.getsize(_WEIGHTS)} bytes)")
