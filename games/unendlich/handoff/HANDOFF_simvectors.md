# HANDOFF — `r336-simvectors` (adoption notes; no bin wiring)

`util::simvec` is a pure library — nothing to wire in the bin. These are adoption notes for the
hot loops that should switch from per-vector `vecmath` calls to the batched ops.

## Drop-in adoption (results are identical to the current `vecmath` calls)
Any loop shaped like `for v in xs { out.push(vecmath::norm3(v)) }` becomes a single
`simvec::normalize_into(xs, &mut out)` (pre-size `out` to `xs.len()`). Same for `len3` → `len_into`,
`lerp3` → `lerp_into`, and the elementwise add/scale/dot. Because the arithmetic is identical
element-for-element (proven in-module against a verbatim `vecmath` oracle), this is a mechanical
swap — no numerical review needed.

Candidate sites (read-only here; coordinator/owners adopt):
- **mesher** (`render/mesher.rs`) — per-vertex normal normalize / position lerps over arrays.
- **particles** — bulk position/velocity `add`+`scale` integration steps.
- **flow fields** (water/wind) — bulk `dot`/`len`/`normalize` over a grid of vectors.

## When to choose SoA
For the heaviest loops, store as `Vec3Soa` (`from_aos`/`to_aos`) and call the `*_soa` ops — the
parallel `x`/`y`/`z` lanes are what LLVM vectorizes most readily. Convert once at the boundary; keep
the inner loop SoA. AoS ops are provided for everything too, for sites not worth converting.

## Notes / one caveat to reconcile
- One `pub mod simvec;` line in `util/mod.rs` (suggested after `pub mod vecmath;`). No other edits.
- **Degenerate normalize:** `normalize_into` reproduces `vecmath::norm3` exactly, whose near-zero
  fallback is **`[1, 0, 0]`** (panic-free, NaN-free) — *not* `[0,0,0]`. The r336 brief's verify note
  said "normalize of zero → zero", which contradicts the live `vecmath` semantics; parity with
  `vecmath` is the stated primary contract, so the batched mirror matches `norm3`. If the operator
  actually wants `0 → 0`, that is a `vecmath::norm3` policy change (one site) and should be made
  there so the scalar and batched paths stay in lockstep — out of scope for this thread.
