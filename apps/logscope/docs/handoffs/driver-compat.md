# Driver compatibility & the Asahi magenta tint

## ROOT CAUSE FOUND: leaked GL_SCISSOR_TEST
On the M2 Max (Asahi/Mesa, GL 4.6 Core, GLSL 330) the magenta covered MOST of the frame except a horizontal
band of correct colour, and LOGSCOPE_NO_POST / LOGSCOPE_NACRE / LOGSCOPE_GL_ES made NO difference — proving
the cast is upstream of post, nacre, and the GLSL profile.

egui invokes our paint callback with GL_SCISSOR_TEST still ENABLED, its scissor box clipped to the scene
widget rect (window coords). We bound our own offscreen FBOs and glClear-ed them WITHOUT disabling that
scissor, so the clear was clipped to that box: most of each offscreen target was never cleared and kept
UNDEFINED tile memory, which a tiler GPU (Apple/Asahi) surfaces as MAGENTA. The 'horizontal bar of accurate
colour' is exactly the scissored region; llvmpipe hid it because its undefined memory reads near-black. None
of the toggles mattered because they all run downstream of that half-cleared scene texture.

FIX: glow_scene::paint disables GL_SCISSOR_TEST for the offscreen passes (scene+bloom+god-ray) so each target
clears in full, then restores egui's scissor before the final composite (so it still honours the widget clip).
Kept as defence-in-depth: zero-initialised FBO textures + composite NaN-guard/clamp.

---

The scene renderer (`glow_scene.rs` + `gpuscene.rs`) is shared by every bin (`scene-view`, the GUI, the
pearl harness), so fixes here apply everywhere. Symptom under investigation: a **magenta tint** in
`scene-view` on the **Asahi** (Apple-silicon Mesa) driver, while llvmpipe / desktop GL are fine.

## Why a tint shows up on one driver and not another
A *tint* (not a crash, not total garbage) on a context whose shaders **compiled and linked** means it's a
**runtime** difference, not a missing feature. The usual suspects, roughly in order:

1. **Uninitialised render-target memory.** Tiler GPUs (Asahi is one) don't always clear FBO storage; sampling
   an unwritten texel can surface as garbage — magenta is a common flavour. *(Hardened: FBO colour textures are
   now zero-initialised, and a skipped pass at low LOD can no longer leave a target undefined.)*
2. **Out-of-range / NaN channels.** A `0/0`, `pow(neg, x)`, or `cos()` of a huge argument can yield NaN/Inf on
   one driver and a clamped value on another; unclamped it smears as a colour cast. *(Hardened: the composite
   NaN-guards and clamps its output to `[0,1]`.)*
3. **Thin-film / nacre over-application.** The water nacre hue depends on `dot(N, view)` and unbounded
   `v_world.y`/`u_time` inside `cos()`. A sign/precision difference can bias the whole (mostly-water) scene
   toward magenta. *(Now scalable via `u_nacre`; set `LOGSCOPE_NACRE=0` to test.)*
4. **Channel swizzle / format default.** *(Hardened: textures pin an identity R,G,B,A swizzle.)*
5. **GLSL profile quirks.** Native defaults to desktop `#version 330`; Asahi's stack is GLES-first.
   `LOGSCOPE_GL_ES=1` switches to the `#version 300 es` path (same shaders).

## Bisection procedure (run on the Asahi machine)
First just try the current build — the hardening in (1),(2),(4) may already fix it. If not, read the new
provenance line printed once at startup:

    [logscope] GL_VENDOR=… GL_RENDERER=… GL_VERSION=… | GLSL #version … | GL_SHADING_LANGUAGE_VERSION=…

then bisect with env toggles (each is independent):

| Command | If the tint DISAPPEARS it's… |
|---|---|
| `LOGSCOPE_NO_POST=1 scene-view` | …in the **post chain** (bloom / god-ray / pearl) |
| `LOGSCOPE_NACRE=0 scene-view`   | …the **thin-film nacre** in the scene pass |
| `LOGSCOPE_GL_ES=1 scene-view`   | …a **desktop-GL 330** path issue (use the GLES path) |
| `LOGSCOPE_RENDER_SCALE=0.6 …`   | (perf, not tint) renders the scene at 0.6× and upscales |

If it survives `LOGSCOPE_NO_POST=1` **and** `LOGSCOPE_NACRE=0`, the cast is in the base lighting / material
texture-array sampling (`mat`/`nrm` `sampler2DArray`) — next test would be a flat-material pass.

## Notes
- All toggles read at the renderer/uniform layer, so they work identically in the GUI bin.
- For the GUI these env reads should later be replaced by Settings values (one helper each) — see TODO.
