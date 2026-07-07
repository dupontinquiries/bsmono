# SHAREDLIB_REPORT — `r340-sl-math` (math / util cluster)

Cluster: `util::{mathx, vecmath, simvec, fastmath, batch_kernel, bbox, hash_util}` + the scalar
clamp/lerp/smoothstep/hash sites across the tree.

**Base caveat:** the r340 base (`r339`) was not in my sandbox; I worked off the available base, so
**line numbers are indicative** (the files/patterns are stable across revisions; exact lines drift).
Counts are from a full-tree grep (1097 `.rs`).

**Headline:** the canonical homes already exist (`hash_util::mix64`, `util::mathx`, `util::vecmath`,
`util::simvec`) — the r332/r336 work unified the *definitions*. What remains is **call-site
consolidation**: a large tail of files still carry private copies of helpers that already have a
home. Intra-`util` duplication is now minimal; the win is routing the tail in once.

---

## Candidate 1 — splitmix64 bit-mixer (HIGH VALUE, cross-cluster)
**Pattern:** private re-implementations of the splitmix64 mixer (constants
`0x9E3779B97F4A7C15`, `0xBF58476D1CE4E5B9`, `0x94D049BB133111EB`) and per-seed `->[0,1)` helpers,
instead of `hash_util::mix64` / `mix64_unit` / `mix64_2`.
**Scale:** **71 files** contain the splitmix constants outside `hash_util`. Two sub-shapes:
- *thin delegators* already collapsed (good, leave): e.g. `world/terrain_warp.rs:49`,
  `world/plant_exotics.rs:184`, `world/batch_ridge_noise.rs:70` — `fn mix(x)=hash_util::mix64(x)`.
- *full local copies* to fold: e.g. `world/ridge_noise.rs:57`, `world/giant_cave.rs:98`,
  `world/weather.rs:199` (`hash_u64`), `world/batch_terrain_warp.rs:57`, `world/scatter_cluster.rs:66`,
  `world/ore_distribution.rs:58,66` (`hash3`+`mix`), plus `sim::legendary_weapon::sm64` and the
  `cloud_noise::hash3` lattice hash.
- *`->[0,1)` reimplementations* of `mix64_unit`: `sim/popgen.rs:22`, `sim/plant_propagation.rs:50`,
  `sim/item_resolve.rs:101`, `sim/item_genetics.rs:41`, `sim/tree_pollination.rs:58` (`fn unit(...)`).
**Home:** `util::hash_util` (exists: `mix64`, `mix64_unit`, `mix64_2`).
**Status:** PROPOSE (coordinator sweep). **NOTE divergences, don't blind-swap:** some `fn unit` take a
`&mut state` and *step* a stream (RNG-stepper shape) rather than hashing a fixed seed — those map to a
small `hash_util` stream helper, not `mix64_unit` directly; flag per-site.
**Cross-overlap:** `sl-worldgen` (noise/placement hashing) and `sl-batch` (batch seed hashing) will
propose the same `hash_util` consolidation — **dedup there**; `hash_util` is the single home.

## Candidate 2 — scalar clamp/lerp/smoothstep (continues the r332 mathx migration)
**Pattern:** inline `[0,1]` clamps and private `fn clamp/lerp/smoothstep`.
**Scale:** inline saturate idioms **743** sites (`.clamp(0.0,1.0)` ×625, `.max(0.0).min(1.0)` ×118);
private scalar-helper defs outside the math modules **101**. Examples: `sim/driver.rs:237`,
`sim/shading_uniforms.rs:69,82`, `sim/ecology_population.rs:147` (`fn clamp`),
`sim/terrain_variation.rs:94`, `sim/fog_volume.rs:65`, `sim/dragon_growth_stage.rs:105`,
`sim/biome_climate.rs:95` (`fn smoothstep`).
**Home:** `util::mathx` (exists: `clamp`/`clampf` (NaN-pass), `saturate` (NaN→0), `lerp`,
`smoothstep`). Map per the r332 rule: `.max(0).min(1)`→`saturate`, `.clamp(0,1)`→`clampf(_,0,1)`,
clamped `smoothstep(t)`→`smoothstep(0,1,t)`.
**Status:** PROPOSE (coordinator sweep — same mechanical migration r332 began).
**NOTE divergences:** `sim/roar_sample.rs:26` defines `fn lerp(t, lo, hi)` — **argument order differs**
(`t` first); do NOT swap to `mathx::lerp(a,b,t)` without reordering. Unclamped `smoothstep` bases
(`cloud_noise`) and `plant_noise::smootherstep` (quintic) are **different functions** — keep, or add
`mathx::smoothstep_unit`/`smootherstep` rather than fold into `mathx::smoothstep`.
**Cross-overlap:** every other cluster clamps (`sl-fields` clamp-to-bounds, `sl-batch` `nonneg`,
`sl-mesh`, `sl-worldgen`) — those route to `mathx` too; **dedup**.

## Candidate 3 — vec3 length / normalize / dot (scalar + batched)
**Pattern:** hand-rolled `(x*x+y*y+z*z).sqrt()` and normalize-with-degenerate-guard.
**Scale:** **79** hand-rolled len/normalize sites; private `fn normalized`/`fn unit3` e.g.
`sim/sound_mix.rs:103`.
**Home:** `util::vecmath` (`len3`/`norm3`/`lerp3`, scalar) + `util::simvec` (`len_into`/
`normalize_into`/`dot_into`, batched). `fastmath::norm3_fast` stays the **opt-in** approximate path.
**Status:** PROPOSE. **RESPECT:** `norm3`/`simvec::normalize_one` share the **exact** degenerate
fallback `[1,0,0]` (NaN-free) — any folded site that previously returned `0` on zero is a *semantic*
change; flag, don't silently unify (this is a real divergence — see `sl-math` simvec MANIFEST).

## Candidate 4 — intra-`util` tidy (small, in-cluster, clean)
- `vecmath::lerp3` is three inline `a+(b-a)*t` — could delegate to `mathx::lerp` per component
  (bit-identical). Clean one-file dedup. **PROPOSE** (didn't rewrite to avoid touching `vecmath`
  ahead of the coordinator's overlap pass).
- `vecmath::scale_clamp01` uses `.clamp(0,1)` (NaN→NaN) — distinct from `mathx::saturate` (NaN→0);
  if unified, it must become `clampf`, not `saturate`. **NOTE.**
- No clamp/lerp duplication exists *inside* `fastmath` (only rsqrt/sin/cos/exp approximations) — the
  approximate path is already cleanly separated; **do not** let any field path silently call it.

---

## What I EXTRACTED (additive, shipped) — the perf bonus
No new module was needed (homes exist). The shipped code is the **PERF BONUS (a)**: `util::simvec`
gains pure per-item kernels (`normalize_one`, `len_one`) under a `// r340-sl-math` header, making the
batched ops provably a map of a pure kernel, so they route through `util::task_lanes::map_pure`
(Class-P) for free. See MANIFEST for the parity proof (serial == parallel, bit-for-bit, real rayon).

## RESPECT-the-fields note
`mathx`/`vecmath`/`simvec` are the FP32-exact primitives the continuous physics fields rely on. Every
consolidation above is **bit-identical** (clamp/lerp/normalize are the same IEEE ops; `mix64` is the
one true mixer). The only "faster" math in the cluster, `fastmath`, stays **opt-in** and must never be
silently swapped into a field path — flagged so the coordinator's sweep doesn't route a field clamp
through an approximation.
