# DISPATCH PACKET — r330 ADDENDUM (3 more threads)

Three additional threads beyond the six in `DISPATCH_PACKET_r330.md`. All read
`docs/dispatch_r330/SHARED_RULES.md` FIRST. Base = latest `unendlich-r330*-VERIFIED.zip`.

Two are **code-maintenance / de-duplication** (shared helper libs, factoring repeated
sub-functions) and one is **content wiring** (legendary swords, fully into the game).

**Hard non-collision rule (these three + the six already out):** each brief carries a
FILE ALLOW-LIST and a DO-NOT-TOUCH list. Stay inside it. New shared modules go in `util/`
or `sim/` as NEW files; touch `util/mod.rs` / `sim/mod.rs` with **ONE** `pub mod` line only —
the coordinator reconciles all the mod-line additions on merge.

**Merge order on return (addendum):** `refactor-math → refactor-batch → swords`
(the two helper libs land first as new files + isolated migrations; swords touches `item.rs`
exhaustive matches and is gated last). Each gated with `rustc --test` (standalone where the
module is self-contained) or `cargo test --features "render-core egui" --lib <module>`.

---

## T-refactor-math — shared EXACT scalar-math helper lib + migrate dup'd call sites
**WID:** `r330-refactor-math`
**Why:** `fn clamp` is locally re-defined in **~161** files, `fn lerp` in ~34, `fn smoothstep`
in ~20, plus scattered `nonneg`/`saturate`. `util::fastmath` only holds *approximate*
transcendentals (`rsqrt`, `sin_fast`, …) — there is **no** home for the exact small helpers.
**Build:** new **`util::mathx`** (`crates/unendlich-engine/src/util/mathx.rs`) — the canonical
**exact** scalar helpers, each `#[inline]`, pure, panic-free, NaN-sane:
`clamp(x,lo,hi)`, `clampf`, `lerp(a,b,t)`, `inv_lerp`, `remap(x,a,b,c,d)`, `smoothstep(e0,e1,x)`,
`saturate(x)` (= clamp 0..1), `nonneg(x)`. Match the most common existing semantics (e.g. the
`nonneg`/clamp-to-`[0,a]` shape in `sim/batch_aggro_decay.rs`) so migrations are bit-identical.
**Then migrate** a set of **≥10** modules that currently define a *local* copy of one of these:
grep `fn clamp\|fn lerp\|fn smoothstep\|fn nonneg\|fn saturate` and pick modules **inside your
allow-list**, replace the local def with a `use crate::util::mathx::…`, and **prove each
migration is behaviour-preserving** (a parity test: old-local-impl output == `mathx` output over
a sampled grid, for at least the trickiest few). Report the exact migrated list in your MANIFEST.
**ALLOW-LIST (migrate only within these):** the **visual/post-fx scalar** modules at crate root —
e.g. `fog_blend`, `depth_fade`, `decal_fade`, `vignette_grain`, `chromatic_aberration`,
`height_fog`, `edge_highlight`, `fresnel_schlick`, `tonemap_curve`, `exposure_adapt`,
`bloom_math`, `lens_dirt`, `rim_light`, `sss_approx` (and similar root-level visual `*.rs`).
**DO NOT TOUCH:** anything under `sim/` (refactor-batch owns the `batch_*` + `sim/` clusters),
`item.rs`/`loot.rs` (swords), and the six-thread files (`leaf_decay`, `world_catalog`,
`world_launch`, `settings_model`, `menu_screen_layout`, `surface_scan`, `roamer_cap`,
`item_tooltip`, the mesher/`chunk.vert`). Don't change `util::fastmath`.
**Verify:** `mathx` unit tests (clamp/lerp/smoothstep edge + NaN behaviour; remap inverts;
saturate bounds) **plus** the per-migration parity tests. Self-contained → `rustc --test`.
**Watch:** keep semantics EXACT — `mathx` is not `fastmath`; no approximations. If a local
`clamp` had subtly different NaN/order behaviour, NOTE it rather than silently changing it.

---

## T-refactor-batch — shared batch-map helper; collapse the `batch_*` boilerplate
**WID:** `r330-refactor-batch`
**Why:** **67** `batch_*` modules each mirror a scalar sibling with the *same shape*: an
`#[inline] kernel`, a scalar `*_one`, and a `*_batch` that hand-rolls the same
"map kernel over a `(…)` slice into a `Vec`" loop — and several re-define their own `nonneg`/
clamp (see `sim/batch_aggro_decay.rs`). The map loop and the helpers are the repetition to factor.
**Build:** new **`util::batch_kernel`** (`crates/unendlich-engine/src/util/batch_kernel.rs`):
- `map_into<T, U>(src: &[T], out: &mut Vec<U>, f: impl Fn(&T) -> U)` — clears+reserves `out`,
  one streaming pass; the canonical replacement for the per-module `_batch` loop. Also a
  `map_collect<T,U>(src, f) -> Vec<U>` convenience. Pure, alloc-bounded, panic-free.
- (Optional) a tiny `Batch1` trait `{ type In; type Out; fn one(&Self::In)->Self::Out; }` with a
  provided `batch(&[In], &mut Vec<Out>)` default routing through `map_into`, IF it reads cleanly;
  if it fights the existing free-fn style, skip it and just use `map_into`.
- Re-use `util::mathx::{nonneg, clamp}` (from refactor-math) for the helper dedup — **but do NOT
  edit refactor-math's files**; if `mathx` isn't merged yet, define `nonneg` locally in
  `batch_kernel` and leave a `// TODO fold into mathx` so the coordinator unifies them.
**Then migrate** **≥6** `batch_*` modules to route their `*_batch` through `map_into` (kernel and
the bit-exact scalar parity preserved — the modules' own "batch == scalar per-element" tests must
still pass). Pick from `sim/batch_*` within your allow-list; report the migrated list.
**ALLOW-LIST:** `sim/batch_*.rs` modules whose **scalar sibling is NOT touched by refactor-math**
(all `sim/batch_*` qualify — refactor-math is root-visual only), e.g. `batch_aggro_decay`,
`batch_scent_decay`, `batch_memory_decay`, `batch_crit_roll`, `batch_drop_chance`,
`batch_threat_assess`, `batch_path_cost`, `batch_status_stack`. Edit ONLY the `batch_*` file
(its `_batch` fn + local-helper removal) — **do NOT edit the scalar sibling** (`aggro_decay.rs`
etc.), so refactor-math and you never overlap.
**DO NOT TOUCH:** root-level visual `*.rs` (refactor-math), `item.rs`/`loot.rs`/`legendary_loot`
(swords), the six-thread files, `util::fastmath`.
**Verify:** `batch_kernel` unit tests (`map_into` clears `out`, reserves, preserves order, empty
slice → empty) + each migrated module's existing batch-vs-scalar parity test still green
(`cargo test --features "render-core egui" --lib <module>`; or flatten the module + `map_into`
into one `rustc --test` file).
**Watch:** the migrations must be **bit-for-bit identical** to the current output — this is a
mechanical de-dup, not a numerics change. Don't touch the off-limits shader files.

---

## T-swords — get the legendary swords fully into the game (pure item/loot path)
**WID:** `r330-swords`
**Why / the gap:** `sim::legendary_weapon` already *forges* full `Weapon` stat-blocks
(`forge(seed, class, element_idx, tier)`, `WeaponClass`, `ProcKind`, element procs,
`chain_targets`/`jet_hits`), and `sim::rarity_tier` / `sim::item_modifier` / `sim::drop_chance`
exist — **but `item.rs::ItemKind` has only `Block/SpawnEgg/Ingot/Tool`**, so a forged legendary
**cannot become an inventory item, drop, or be held**. That's why they aren't "in the game."
Close the **pure** half of that gap; propose the render/equip half for the coordinator.
**Build:**
1. **`item.rs` — new `ItemKind` variant** `Legendary(LegendarySpec)` where
   `LegendarySpec { seed: u64, class: WeaponClass, element_idx: u8, tier: u8 }` — the **compact,
   deterministic forge tuple** (NOT the whole `Weapon`; reconstruct on demand via
   `legendary_weapon::forge(spec)` so items stay small and the stat-block stays single-source).
   Update **every exhaustive match**: `display_color` (tint by element/tier),
   `max_stack` → 1, `display_name` → e.g. `"<Element> <Class>"` from the spec, `is_spawn_egg`
   stays false. Add a `LegendarySpec::weapon(&self) -> Weapon` convenience (calls `forge`).
2. **New `sim::legendary_loot`** (`crates/unendlich-engine/src/sim/legendary_loot.rs`) — the pure
   drop-selection core: given `(roll_seed, drop_context)` and a rarity/drop-chance input from
   `rarity_tier`/`drop_chance`, decide IF a legendary drops and produce the `LegendarySpec`
   (deterministic from the seed: pick class/element/tier via splitmix the way `legendary_weapon`
   does). Returns `Option<LegendarySpec>`. No RNG/clock — seed passed in.
3. **Loot hook (pure):** propose in a `HANDOFF_swords.md` exactly where `loot.rs::drops(...)`
   (returns `Vec<ItemDrop>`) calls `legendary_loot` to append a legendary `ItemDrop` on a rare
   roll; you MAY add that one pure call-site if it stays panic-free and seed-driven.
**Anchors:** `sim/legendary_weapon.rs` (`forge`, `WeaponClass`, `ProcKind`, `Weapon`),
`sim/rarity_tier.rs`, `sim/item_modifier.rs`, `sim/drop_chance.rs`, `item.rs` (`ItemKind`@169,
`display_color`/`max_stack`/`display_name`), `loot.rs` (`ItemDrop`, `drops`, `base_drops`).
**ALLOW-LIST:** `item.rs`, new `sim/legendary_loot.rs`, `loot.rs` (the one drop hook only),
`HANDOFF_swords.md`. **DO NOT TOUCH:** `sim/item_tooltip.rs` (T-items owns it — it will *read*
your new variant; just make `display_name`/`display_color` correct), held/render/hotbar/bin.
**Verify:** ≥6 tests — `ItemKind::Legendary` round-trips its spec and `spec.weapon()` reproduces
the same `Weapon` as `forge` (determinism); `max_stack==1`; `display_name` non-empty per element;
`legendary_loot` is deterministic and respects drop chance (0% never, 100% always) + rarity gate;
two different seeds give different specs. `cargo test --features "render-core egui" --lib` (touches
`item.rs` → needs the lib), or flatten `legendary_loot` + a `forge` stub for a fast `rustc --test`.
**MERGE NOTE for coordinator:** `item.rs` gains a variant → T-items' `item_tooltip` match arms and
any other exhaustive `ItemKind` match must be updated on merge (land swords' `item.rs` first, then
rebase tooltip). The render/equip/hotbar/held-item wiring is **proposed in `HANDOFF_swords.md`**,
done on-device — workers can't compile the bin.

---

### Coordinator checklist (addendum)
- [ ] Cut `worker-<WID>-r330.zip` = base + `WORKER_ID` + this brief + `SHARED_RULES.md`
      (`tools/thread_tool.rs extract`), `target/` excluded.
- [ ] On return merge `refactor-math → refactor-batch → swords`; fold the two `nonneg`/clamp
      copies into one `util::mathx` if refactor-batch left the TODO; update all `ItemKind`
      exhaustive matches once swords lands; gate each; roll `unendlich-r331-merged.zip`;
      add `## r331` ledger entries.
