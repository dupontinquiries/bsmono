# DISPATCH PACKET — r330

Coordinator: building **multiplayer** in-house this round (`sim::net_channels` landed +
VERIFIED, see COORDINATOR_NOTES_r330.md). Six sections dispatched below — one worker each.
All read `docs/dispatch_r330/SHARED_RULES.md` FIRST. Base = `unendlich-r330-*-CHECKED.zip`.

Each brief is **one scoped, doc-grounded, in-sandbox-validatable pure task**. Anchors are
real (file:line verified against this tree). Workers return changed `.rs` (+ optional patch)
per SHARED_RULES §6.

**Merge order on return** (new files / isolated cores first, shared-file touches last):
`T-items → T-bughunt → T-mobs → T-menus → T-world → T-gpu`
(items/bughunt/mobs/menus are new-file or self-contained cores; world touches the
`surface_y_at` neighbourhood; gpu touches the mesher + a shader, gated last). Each:
`git apply -p1` → standalone/`--features "render-core egui"` test → roll next base.

---

## T-items — item/armor lore tooltip assembly (pure core)
**WID:** `r330-items`
**Goal:** A pure tooltip-composition core that turns an item (name + affixes + rarity +
lore) into ordered, styled tooltip lines — the data the menu renderer draws. Grounds the
design in `docs/design/item_armor_lore_tooltips.extracted.md`.
**Build:** new `sim::item_tooltip` (`crates/unendlich-engine/src/sim/item_tooltip.rs`).
- Input: an item view assembled from existing cores — `sim::affix_count`,
  `sim::affix_magnitude`, `sim::item` (rarity/material), and a lore string slice.
- Output: `Vec<TooltipLine { text, role: Title|Affix|Lore|Flavor, emphasis }>` with a
  deterministic order (title → rarity → affixes sorted by magnitude desc → separator →
  lore). No allocation surprises; no `String` formatting that panics.
- Provide `compose(item: &ItemTooltipView) -> Vec<TooltipLine>` + a width-aware
  `wrap_lore(text, max_cols) -> Vec<String>` (pure, byte/char-safe, no `.unwrap`).
**Anchors:** `sim/affix_count.rs`, `sim/affix_magnitude.rs`, `item.rs`,
`docs/design/item_armor_lore_tooltips.extracted.md`.
**Verify:** ≥6 tests (ordering stable, affixes sorted by magnitude, lore wraps without
splitting mid-grapheme on ASCII, empty-affix item still yields a title line, deterministic).
Standalone `rustc --test` if you keep the view struct self-contained; else
`--features "render-core egui"`.
**Bin handoff (md):** note where `render/world_renderer.rs` would call `compose()` to draw
the hover tooltip — propose, don't wire.

---

## T-mobs — roamer spawn-admission cap (pure core)
**WID:** `r330-mobs`
**Goal:** Fix **roamer overpopulation at the spawn gate** (`NEXT_SESSION_GUIDE.md §4d`:
observed 73 roamers vs cap 8). Build the pure admission core the spawn director consults so
spawning **never pushes population past the configured cap**.
**⚠ HISTORY — read before scoping (`WIRING_STATUS.md r300`):** the operator **declined
forced population despawn/eviction** of existing creatures (they want the herd intact and
chose cheaper per-creature work instead). So this task is **spawn-side admission ONLY** — do
**not** despawn or evict live creatures. Over-cap is corrected by *not spawning more*, letting
the population settle naturally.
**Build:** new `sim::roamer_cap` (`crates/unendlich-engine/src/sim/roamer_cap.rs`), consumed
by `sim::mob_spawn_director` / `sim::batch_spawn_budget` (propose wiring in md; you may add a
single pure-fn call-site at the spawn gate).
- `admit(current: u32, cap: u32, requested: u32) -> u32` → how many of `requested` may spawn
  (saturating; 0 when `current >= cap`; clamps so `current + admitted <= cap`). Alloc-free.
- `overflow_count(current: u32, cap: u32) -> u32` → a **diagnostic only** (for logging the
  stutter cause), NOT a despawn list.
**Anchors:** `NEXT_SESSION_GUIDE.md §4d`, `WIRING_STATUS.md r300` (declined-despawn history),
`sim/mob_spawn_director.rs`, `sim/batch_spawn_budget.rs`, `sim/popdyn.rs`, `sim/ai_budget.rs`.
**Verify:** ≥6 tests (`admit` never lets `current+admitted` exceed cap across a fuzz of
current/requested; cap 0 admits nothing; `current >= cap` admits 0; `overflow_count` is pure;
`admit` is alloc-free / deterministic).
**Watch:** keep `admit` allocation-free (it's on the spawn path); do NOT add a despawn path.

---

## T-world — surface read from generated voxels (FP32/FP64 parity, pure core)
**WID:** `r330-world`
**Goal:** Attack the **highest-value bug** (`§4a`): flora/mobs float because spawn/placement
reads the **CPU FP64 prediction** (`world/chunk_manager.rs::surface_y_at`, line **1830**)
while terrain is generated in **FP32**. Build the pure "read the actual top solid block of a
generated chunk" core so placement can switch off the prediction.
**Build:** new `sim::surface_scan` (`crates/unendlich-engine/src/sim/surface_scan.rs`).
- `top_solid_y(column: &[Voxel], y_min: i32) -> Option<i32>` — scan a generated voxel column
  top-down, return the first solid Y (None if all air). Pure, no generator calls.
- A numeric **divergence harness** in tests: feed a synthetic FP32-generated column whose
  top differs from an FP64 prediction value and assert `top_solid_y` follows the *voxels*,
  not the prediction (this is the bug, reproduced as a unit test).
**Anchors:** `§4a`, `world/chunk_manager.rs:1830` (`surface_y_at`),
`world/constants.rs` (`SEA_LEVEL = 64`), `world/{terrain,cpu,gpu}_generator.rs`.
**Verify:** ≥6 tests. **Do NOT** edit `chunk_manager.rs` placement call-sites yourself —
write a `HANDOFF_world_surface.md` proposing exactly which call-sites (flora placement,
mob ground, spawn) switch from `surface_y_at` to a `top_solid_y` lookup, and the perf note
(cache the scan per chunk). Coordinator/operator wire + validate on the FP32 GPU path.

---

## T-gpu — foliage sway-tag in the mesher (pure tag logic + shader gate)
**WID:** `r330-gpu`
**Goal:** Fix **grass/foliage sway catching solid block-tops** (`§4b`): the wind shader sways
by green vertex color, so it also sways solid grass-block tops and can't anchor plant bases.
Proper fix = the **mesher tags sway-eligible foliage vertices** with an explicit bit instead
of relying on color.
**Build:**
- Pure tagging logic: a `fn sway_flag(block, face, is_plant) -> u8` (or extend the vertex
  attribute) in the CPU mesher core (`render/mesher.rs`) **and** the GPU mesher
  (`world/gpu_mesher_vk` / its `.comp`) so both paths agree — keep it a pure function with
  unit tests; both meshers call it.
- Shader side: thread the flag through `shaders/chunk.vert` (sway only when flag set, anchor
  the base vertex). **`chunk.frag` is OFF-LIMITS** — do the gating in `.vert` only.
**Anchors:** `§4b`, `shaders/chunk.vert` (~88, wind sway), `render/mesher.rs`,
`world/gpu_mesher_vk*`, operator runs `GPU_MESHER=2`.
**Verify:** unit tests for `sway_flag` (block-top of a solid grass block → NOT sway-eligible;
a plant/foliage vertex → eligible; deterministic). `chunk.vert` MUST pass
`glslangValidator --target-env vulkan1.2 -V` + `spirv-val`; regenerate the committed
`assets/shaders/chunk.vert.spv`+`.hash`. If feasible, emit a small headless PNG (lavapipe)
showing tagged vs untagged. Ship the validation output.

---

## T-menus — graphics-settings model + Camera/Gameplay page (pure data)
**WID:** `r330-menus`
**Goal:** Extend the **pure settings model** so the new graphics knobs are first-class,
validated fields and a Camera/Gameplay page exists in the layout — without touching the bin.
Grounds `§4e` (settings wiring) on the pure side; live render-pipeline application stays a
coordinator/bin job.
**Build:** in `sim::settings_model` (add fields/fns under a `// r330-menus` header) +
`sim::menu_screen_layout`:
- New validated `SettingField`s: `RenderScale` (clamp e.g. 0.5–2.0), `ShadowResolution`
  (enum 4096/8192/16384), `DynamicLights` (count, clamp). Reuse the existing clamp/NaN-safe
  `adjust` path (`settings_model.rs:82` switch, NaN test at line 229 — keep that guarantee).
- An **apply-classification**: `fn apply_kind(field) -> ApplyKind { Live | NeedsPipelineRebuild }`
  so the bin knows which knobs hot-apply vs need a rebuild (render_scale/shadow/dynamic-lights
  = rebuild; render-distance = live). Pure data only.
- Add the **Camera/Gameplay** page to `menu_screen_layout` carrying the existing `Fov` field
  (`settings_model.rs:28`) + the new gameplay toggles' slots.
**Anchors:** `§4e`, `sim/settings_model.rs` (`SettingField`, `Fov`@28, `adjust`@82, NaN@229),
`sim/menu_screen_layout.rs`, `settings_schema.rs`, `menu_layout.rs`.
**Verify:** ≥6 tests (each new field clamps to range; NaN delta is ignored; `apply_kind`
correct per field; the Camera page lists Fov + new fields in order). `--features "render-core egui"`.
**Bin handoff (md):** propose the `render/world_renderer.rs` settings-tab additions
(staged `ui_*` field pattern @~450/528, init @~1463, writeback @~4333) — propose, don't wire.

---

## T-bughunt — floating-trunk decay + panic audit (pure cores)
**WID:** `r330-bughunt`
**Goal:** Two independent, well-scoped fixes:
1. **Floating-trunk decay** (`§4e`): logs left hanging when their support is gone aren't
   decaying correctly. Fix the leaf/log-adjacency support check in `world/leaf_decay.rs`
   (and `plant_lifecycle` crown logic only if the decay rule lives there) so an unsupported
   trunk segment is marked for decay. Keep it a pure predicate with tests.
2. **Panic audit**: replace `unwrap()`/`expect()` in **non-test** code in
   `sim/world_catalog.rs` and `sim/world_launch.rs` (both contain them — confirmed) with
   `Option`/`Result`/saturating handling per SHARED_RULES §1, adding a regression test for
   each path you harden. Don't change public signatures unless necessary; if you must,
   note it in the MANIFEST.
**Anchors:** `§4e`, `world/leaf_decay.rs`, `world/plant_lifecycle.rs`,
`sim/world_catalog.rs`, `sim/world_launch.rs`.
**Verify:** ≥6 tests total across the two items (an unsupported trunk decays; a supported
one does not; each hardened call no longer panics on the bad input that used to). Run the
relevant module tests; standalone where self-contained.

---

### Coordinator dispatch checklist
- [ ] Cut `worker-<WID>-r330.zip` per brief = base + `WORKER_ID` + `WORKER_BRIEF.md`
      (this brief) + `SHARED_RULES.md`, `target/` excluded, root = `unendlich/`
      (use `tools/thread_tool.rs extract`).
- [ ] On return: merge in the order above, gate each (`rustc --test` or
      `cargo test --features "render-core egui" --lib <module>`), roll `unendlich-r331-merged.zip`.
- [ ] Add an `## r331` entry to `docs/WIRING_STATUS.md` per merged item.
