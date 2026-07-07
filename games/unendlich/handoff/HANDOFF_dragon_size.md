# HANDOFF — `sim::dragon_archetype::size_scale` → forge integration (WID r336-dragon-size)

**Status:** the pure ramp (`size_scale`) is DONE + verified (27/27 incl. the
module's 20 existing tests). The **`dragon_forge` rest-dimension multiply is NOT
applied** — it's blocked on the r332 base (see below) and PROPOSED here for the
coordinator to wire on the r336 tree, where the r332 `clamp_extent` lives.

## Blocker (why the forge edit isn't in this return)
This worker's base on hand is `unendlich-r330b-...-VERIFIED`, which **predates
r332**. In it, `dragon_forge` has **no `clamp_extent` / rest·1.15 anim-balloon
clamp** (the line-985 cap is an eye-specific `eye_y_scale`, not the r332 general
clamp), and **no `size_class → body-scale` path** — size currently rides entirely
on `Proportions::bulk = map(g.bulk, 0.6, 1.98)`. The brief's core safety
requirement ("must not regress the r332 clamp; verify the clamp scales with rest
size") and the rest-dimension build path are defined by r332 code not present
here. Editing that shared file against the stale shape would risk conflicting
with the very clamp the coordinator note says must not regress (SHARED_RULES §1:
"don't half-wire … write down what's blocking"). So: ramp delivered + tested;
forge multiply proposed; clamp-scaling proven as a composition property (below),
to be re-run against the real `clamp_extent` on the r336 base.

## The ramp (delivered, in `sim::dragon_archetype`)
`pub fn size_scale(size_class: u8) -> f32` — geometric ramp
`SIZE_SCALE_MIN (0.45) .. SIZE_SCALE_MAX (3.0)` across `0..=SIZE_CLASS_MAX`, ≈6.67×
smallest→largest, monotone, finite, domain-clamped. Values: class 0→0.45,
1→0.72, 2→1.16, 3→1.87, 4→3.00.

## Proposed forge wiring (one multiply, BEFORE the r332 clamp)
In `sim::dragon_forge`, where the rest dimensions are built from the genome
(`Proportions::from_genome` → the `b`/`b2` body-scale used in the `half: [...]`
extents), fold the size-class scale into the **rest** dimensions, then let the
existing r332 `clamp_extent` (rest·1.15) bound *animation* growth as today:

```rust
// after classifying the genome:
let arch = crate::sim::dragon_archetype::classify(g);
let size = crate::sim::dragon_archetype::size_scale(arch.size_class); // 0.45..3.0
// apply to the REST body scale (the multiplier feeding `half` extents),
// e.g. fold into Proportions::bulk or the `b`/`b2` locals:
//   let b  = p.bulk * size;
//   let b2 = p.bulk * size;
// (g.bulk still adds within-class variation; size_scale sets the class.)
```

Because the r332 clamp is **relative** (`clamp_extent(rest, animated, 0.15)` ⇒
bound = rest·1.15), scaling `rest` by `size` scales the bound by `size` too — a
huge dragon's animated extents stay within rest·1.15 of *its* (larger) rest, so
it still animates correctly. The clamp caps *animation* growth, not *base* size.
**Do not** apply `size_scale` after the clamp, and **do not** touch the
arc-follow or the clamp logic — only the rest-dimension multiply.

## Composition proof (shipped, in lieu of the absent clamp)
`size_scale_proof.rs` models the r332 clamp contract
(`clamp_extent(rest, animated, max_grow) = min(animated, rest·(1+max_grow))`) and
proves, for every size_class and a swept animation factor, that the clamped
animated extent stays within `rest_scaled·1.15` and that this bound scales with
`size_scale` (the "clamp is relative, so it scales with size" property). Re-run
the equivalent against the **real** `clamp_extent` on the r336 base as the final
on-device check; add it as a `dragon_forge` test there.

## PNG proof (shipped)
`dragon_size_row.png` — CPU-raster silhouette row, size_class 0→4 left→right on a
shared baseline, each silhouette scaled by `size_scale(c)` (swatch widths encode
the factor). Demonstrates the restored small→huge range (≈6.7×). **This renders
proportional silhouette primitives scaled by the ramp — NOT the real skinned
`dragon_forge::skin` mesh** (that collage is the coordinator's on-device step once
the forge multiply lands on the r332 base).

## Verify after wiring (on device)
- `cargo test --features "render-core egui" --lib dragon_archetype` → the 27
  tests (incl. the 7 size_scale tests) pass.
- After the forge multiply: a smallest→largest `skin()` collage shows the range;
  a huge dragon's per-axis animated extents stay ≤ rest·1.15 of its rest (the
  r332 clamp test, now parameterized by size_class).
