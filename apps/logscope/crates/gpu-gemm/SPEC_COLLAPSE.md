# Two standing findings from the 2026-08-09 gate runs

Landed with the spec-collapse patch. Both outlive the patch itself, so they are
recorded here rather than only in a commit message.

## 1. Every tile-sweep number before this commit measured one pipeline

`gemm_pipeline` built one shader module and specialised it per tile with
`override TILE`. On Honeykrisp (M2 Ultra), pipelines built that way came back
**bit-exact across tiles** — the spec constant is collapsed somewhere below
wgpu, so every "tile" ran the same kernel.

The consequences are larger than the bug:

* **The "tile space exhausted, ~3% spread, kernel work parked" verdict is
  void.** It was three measurements of one pipeline. Re-run `gemmshape-bench`
  after this patch; the prediction is genuinely two-sided now — either a real
  spread appears and the 227 ms GEMM floor moves, or true ties return and the
  old conclusion is restored **on honest evidence for the first time**.
* **Persisted per-shape tile winners are noise.** They were probed under the
  collapse. Bump the gemm-tile store namespace or clear the cache before
  trusting any of them.

The fix is source templating: `override TILE: u32 = 16u;` is replaced with a
`const` literal per tile, so the module *bytes* differ. No driver cache can
collapse distinct modules. An assert guards the anchor line — if
`LINEAR_Q8_0_TILED`'s override line ever drifts, the build fails loudly rather
than silently reverting to one kernel.

**The cache was innocent.** It keys `(ptr, 0x4000|tile)` correctly; the
collapse was below wgpu. Worth stating because the obvious suspect was the
memoisation, and fixing that would have changed nothing.

## 2. `attention_mh_full_t` is known-inaccurate above ~64 rows on Honeykrisp

Divergence onset moved from t≈2100 on lavapipe to **t≈64 on the hardware** —
the direction a driver with measured fast-math departures should push an
unrescaled f32 sum.

**The ViT/DiT vision path runs on it today.** The fix exists: route those call
sites to the tiled kernel, which is certified on both drivers at 5.07e-8
absolute against exact arithmetic at t=2100. The `hd>192` fallback inherits the
same taint; no current target hits it.

## The oracle lesson, applied symmetrically

The three-way verdict this patch encodes: twins agree → pass; twins diverge but
tiled matches f64 → informational, naming the naive twin; both wrong → kernel
guilty.

That structure exists because **disagreement between twins does not say which
side moved** — the f64 column does. Before opening a kernel on any failing
gate, check that gate's own oracle class and denominators first: we have now
watched a gate convict its own oracle once this week.
