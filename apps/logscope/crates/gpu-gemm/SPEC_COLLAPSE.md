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
collapse distinct modules. An assert guards the anchor line — but **it is a runtime assert inside
`gemm_pipeline`, not a build-time check**. A drifted anchor compiles fine and
dies on the first pipeline creation.

In a gate-first workflow that is nearly equivalent, since every gate run
constructs GEMM pipelines and drift cannot survive one kernel-validate pass. It
is *not* equivalent for anyone relying on `cargo build` alone, which is why the
distinction is stated rather than glossed: a green build does not mean the
anchor still matches.

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


## Ledger updates from the c689 report

Both of these post-date the patch text above.

### The two-sided gemmshape question has been run once, and resolved to the middle

On the **M2 Max, post-fix**: real spread appeared — **45% on q/o** — but only
**3–6% on the time-dominant shapes**, moving the floor **~2.5%**.

So "kernel work parked" is **restored on evidence for that box** rather than
merely reinstated. The Ultra rerun is still owed, and still needs the
store-namespace bump first, since the persisted winners under the collapse would
otherwise seed it.

That is the honest shape of the answer: not the clean two-sided outcome the
prediction imagined, but a mixed middle where the sweep matters for a couple of
shapes and does not move the total much.

### The `attention_mh_full_t` routing is an ACCURACY fix, not a speed one

It must **not** inherit the earlier −160 ms expectation. The c689 A/B tied
chunked against naive — **2698.9 vs 2630.7** — because EXT_PAR moves the same
K/V volume, it just moves it coalesced.

Stating this matters because "route the vision path to the tiled kernel" reads
like a performance task, and a reader who acts on it expecting −160 ms will
conclude the routing failed when it did exactly what it was for.

**The perf lever for the quadratic is a different kernel**: a query-block flash
variant that stages K/V tiles once per row-block. That now sits at the top of
the kernel queue.
