# Upscaling: what it would buy, and where it would cost

Asked: port unendlich's neural upscaler to drive 500Hz at 1080p, or to push
1080p→4K.

Short answer: **the 4K case is sound, the 500Hz case is the wrong tool, and
there is nothing to port yet.**

---

## 0. There is no upscaler to port

`unendlich-engine`'s `settings_model` has `upscale_factor()` and
`upscale_mode()` — quarter-res → 4×, half-res → 2×, else 1× — with docstrings,
clamping and a UI slider labelled *"render scale (neural upscale; <100% renders
smaller)"*.

Both functions have **zero callers**. No shader implements a network, no weights
exist in the tree, and `render_scale` reaches the renderer only as
`ui_render_scale`, which scales the UI.

So the question is not "port it" but "build it", and that changes the estimate
completely. What follows is about whether it would be worth building.

---

## 1. 500Hz at 1080p: upscaling is the wrong lever

500Hz is a **2.0 ms** frame budget.

**The fill is not the problem at that rate.** 1080p × 500 = 1.04 Gpix/s. A
current desktop GPU does that without difficulty; it is roughly what it already
does driving 4K at 120Hz. What fails at 500Hz is the *per-frame* cost that does
not shrink with resolution:

- draw-call submission and command-buffer build
- simulation and animation ticks
- present, and the CPU↔GPU sync around it

An upscaler addresses **none** of those. It adds GPU work and a dependency
between passes, to save fill that was not the bottleneck.

**And its own cost does not fit.** A DLSS2/FSR2-class temporal upscaler runs
about 1–2 ms producing 4K on a desktop GPU. Producing 1080p it is cheaper —
call it 0.3–0.5 ms — but against a 2.0 ms budget that is **15–25% of the frame,
spent to avoid fill that was costing less than that**.

The honest lever for 500Hz is reducing per-frame CPU work: fewer draw calls,
instancing, and not re-walking the scene every frame. clawbound's
`quality::Quality` does not help here either — it cuts fill, which is the same
mistake in a different hat.

---

## 2. 1080p→4K: this one is worth building

4K is 8.3 Mpix against 1080p's 2.1 Mpix — **4× the fill**. At 60–120Hz the
budget is 8.3–16.6 ms, so a 1–2 ms upscale buying back most of a 4× fill
reduction is a clear trade.

It is also the case where clawbound's measurements already point:
`post.wgsl` costs 24 bloom taps + 22 god-ray taps **per output pixel**. At 4K
that is 46 samples × 8.3 M = 380 M samples a frame. Rendering at 1080p and
upscaling cuts that by 4× on its own, before any geometry saving.

**Order matters:** the post pass must run at *render* resolution, before the
upscale, or the saving is given straight back.

---

## 3. Latency: upscaling and frame generation are different things

Worth separating, because they are usually discussed together and only one of
them costs latency.

| | what it does | latency added |
|---|---|---|
| **Spatial upscale** (FSR1-class) | one pass over the current frame | its own GPU time only |
| **Temporal upscale** (DLSS2/FSR2) | current frame + motion vectors + history | its own GPU time only |
| **Frame generation** (DLSS3/FSR3) | interpolates between frame N and N+1 | **at least one full frame**, because N+1 must exist before the tween can be shown |

So upscaling does **not** inherently add latency — it adds *cost*, which
becomes latency only if it does not fit the budget. Frame generation always
adds a frame: 2 ms at 500Hz, 16.6 ms at 60Hz, and it is the 60Hz case that
feels bad.

For a game where a leap's landing has to read correctly, frame generation is
the one to refuse. Upscaling is not.

---

## 4. If it gets built

1. **Temporal, not spatial.** A spatial upscale of a 1080p frame to 4K is a
   blur with extra steps; the quality comes from history and motion vectors.
   That means the renderer must produce motion vectors, which it does not today
   — that is the real work, not the network.
2. **Post at render resolution**, as above.
3. **Measure before the network.** A plain bilinear 1080p→4K upscale with the
   post pass moved before it establishes the fill saving on its own. If that
   does not hit the frame target, a network will not either — and if it does,
   the network is only buying image quality, which is a much easier decision to
   reason about.
4. **The UI must not upscale.** Text and icons rendered at 1080p and magnified
   to 4K look worse than anything the saving buys. Draw the UI after, at native.

---

## 5. What not to do

Do not wire `upscale_mode()` to anything as-is. It returns a magnification
factor for a network that does not exist, so wiring it would produce a renderer
that quietly renders at quarter resolution and scales up with whatever the
sampler does — which is exactly the "it shipped and looked broken" shape, and
the slider already promises "neural" in its label.

Either build the thing, or leave the knob unwired and say so. It currently says
so nowhere, which is the one change worth making before any of the above.
