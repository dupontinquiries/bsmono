# Shader Pass Notes (r243)

> Part of the perf work — see `PERF_PROFILING.md`.

A shaders pass with the real toolchain available in-sandbox this round
(`glslangValidator` 15.1 + `spirv-val` from `glslang-tools`). Note the build
itself only recompiles GLSL when a compiler is present (build.rs hashes the
source into a `.spv.hash` sidecar and ships precompiled SPIR-V otherwise), so
GLSL edits are safe — a missing compiler can't break the build — but they only
take visual effect once recompiled on a machine with `glslc`/`glslangValidator`
(the operator's `hot-shaders` path does this).

## New: `tools/validate_shaders.sh`
Compiles every shader in `crates/unendlich-engine/src/shaders` and runs
`spirv-val` on the result. Pure verification — does NOT touch the committed
`assets/shaders/*.spv` (recompiling under a different glslang version yields a
byte-different-but-equivalent module; that is expected, not staleness). Run it
from the repo root. **Current state: 28/28 shaders compile + pass spirv-val** —
the shader set is clean; there were no broken shaders to fix.

## Done: Mie phase micro-opt (`sky.frag`)
`pow(x, 1.5)` → `x * sqrt(x)` in `mie_phase` — mathematically equal, cheaper
(one mul + one sqrt vs an exp/log pair), runs per fragment, visually identical.
Validated. The committed `assets/shaders/sky.frag.spv` was regenerated and its
stale `.spv.hash` removed so the operator's build recompiles it cleanly.

## Deferred (needs a CPU-side UBO change — flagged, not done)
**Per-fragment `inverse(view_proj)` in `sky.frag`.** The sky is a fullscreen
triangle, so this 4×4 inverse runs for (potentially) every framebuffer pixel,
re-deriving a value that's constant for the whole frame — millions of redundant
inverses at 4K. Fix: add `mat4 inv_view_proj;` to `CameraUbo` (and the matching
Rust struct), write `view_proj.inverse()` once per frame on the CPU, replace the
in-shader `inverse(ubo.view_proj)` with `ubo.inv_view_proj`. Pure speedup,
identical output. Held back because it's a descriptor/UBO-layout change that
must be made + runtime-verified together with the Rust side; not safe to do
blind. After the field is added, `tools/validate_shaders.sh` confirms the GLSL
still compiles.

## Other `pow()` candidates (lower priority, left as-is)
`chunk.frag` and `sky.frag` have several `pow(x, N)` with constant integer or
half-integer N (e.g. the sun/moon glow `pow(x, 8.0)` / `pow(x, 64.0)`, fresnel
`pow(x, 5.0)`). Modern shader compilers lower constant-exponent `pow` well, so
hand-expanding these trades readability for a likely-marginal gain — not worth
it without profiler evidence that the sky/water fragment stage is a bottleneck.
The `1.5` case was the clear win (half-integer → the textbook `x*sqrt(x)`).

## Why not more
The shader codebase is mature and already well-optimised (e.g. `bloom_blur.frag`
already does the linear-sampled 9→5 tap Gaussian reduction; `chunk.frag`'s fog
march is carefully tuned with a documented changelog). The remaining real wins
(precomputed inverse, any change to the fog march or water) all either need CPU
coordination or change visible output — both of which want a GPU to verify, so
they're documented for the operator's hands-on turn rather than changed blind.
