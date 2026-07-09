# Paperbound — Tiny Diffusion (portable image generation)

## What this is (and isn't)

**Not** Stable Diffusion. Real SD is a multi-GB PyTorch model that needs a GPU
and downloads weights from HuggingFace — the opposite of portable on a
postmarketOS/Alpine phone. Instead this is the same *paradigm* (a denoising
diffusion model) shrunk to something that runs on a phone CPU:

- pure **numpy**, no torch, no GPU,
- generates **16×16 tiles** (upscaled as needed) — tiny on purpose,
- a full reverse-diffusion is **~8 ms** per tile,
- the trained model is a **~17 KB** `.npz` shipped with the game,
- always has a procedural fallback so it can never fail to produce something.

It's used for **abstract / textural VFX** (ember bursts, ink splashes, frost,
aura, smoke) and could be extended to map surface washes — the kind of content
where "evocative, not literal" is exactly right. It is deliberately **not** used
to draw creatures; the origami part engine already does that far better.

## How it works

`diffusion.py`:
- **DDPM math** — cosine noise schedule, epsilon-prediction, ancestral sampling
  (`TinyDiffusion.generate`).
- **Denoiser** — a per-pixel MLP over a 3×3 unfold (local context) + pixel
  coordinates + a sinusoidal timestep embedding + a one-hot concept code. Sharing
  the MLP across pixels makes it a tiny convolutional denoiser with simple
  backprop (~10k params).
- **Training** — Adam over procedurally-generated concept targets, ~25 s once;
  weights are baked to `diffusion_weights.npz` and shipped, so **runtime never
  trains** — it just loads and samples.
- **Concepts** — `paper, ink, ember, frost, aura, smoke`.

In `game.py`, `vfx_sprite(concept, size, seed)` returns a cached pygame surface
(alpha = luminance, so it reads as a glow for additive blending). Sprites are
generated **once and cached**; nothing runs on the per-frame hot path, and VFX
are cosmetic so **net determinism is unaffected**. The muzzle-flash effect uses a
generated ember tile when available and falls back to drawn circles otherwise.

## Regenerating / extending the model

```
python diffusion.py          # retrains (~25 s) and rewrites diffusion_weights.npz
```

Add a concept by extending `CONCEPTS` and `_concept_target()` (its procedural
example), then retrain. Bump network size (`_H1/_H2`) or `iters` for more detail
at the cost of a bigger weight file / longer train.

## Optional: bridge to real Stable Diffusion

If you *do* run a local SD server, `diffusion.SDBridge` can request tiny images
from it (AUTOMATIC1111 `/sdapi/v1/txt2img` shape), mirroring the LLM bridge:

```
export PAPERBOUND_SD_URL="http://localhost:7860"   # your SD server base
export PAPERBOUND_SD_STEPS=8
```

`SDBridge.txt2img(prompt, size)` returns a numpy RGB array or None (never
raises). The tiny numpy model remains the always-available fallback, so nothing
is required. This path can't be tested in the sandboxed build environment
(HuggingFace and arbitrary hosts are blocked), so verify it against your own SD
server.
```
