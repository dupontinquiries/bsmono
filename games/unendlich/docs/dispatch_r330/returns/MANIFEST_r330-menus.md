# MANIFEST — r330-menus

**WID:** r330-menus · **Section:** menus · **Grounded in:** §4e (settings wiring)
**Base:** `unendlich-r330b-mp-net_channels-and-dispatch-VERIFIED` tree.

## Files touched (2, both in-scope `sim::`)
- `crates/unendlich-engine/src/sim/settings_model.rs` — extended (additions under
  `// r330-menus` markers).
- `crates/unendlich-engine/src/sim/menu_screen_layout.rs` — extended (new builder +
  ids under a `r330-menus` header; `layout_for_title` and `Screen` left untouched).

`sim/mod.rs` **not touched** — both modules already declared (`pub mod settings_model;`,
`pub mod menu_screen_layout;`). No new dependencies. No bin/shader/IO edits.

## API added
`settings_model`:
- `enum ShadowRes { R4096, R8192, R16384 }` + `ShadowRes::as_u32() -> u32`.
- `enum ApplyKind { Live, NeedsPipelineRebuild }`.
- `fn apply_kind(SettingField) -> ApplyKind`.
- `Settings` fields: `render_scale: f32`, `shadow_resolution: ShadowRes`, `dynamic_lights: u32`.
- `SettingField` variants: `RenderScale`, `ShadowResolution`, `DynamicLights`.
- `defaults()` / `clamp()` / `adjust()` extended for the new fields (NaN-safe; saturating;
  the line-229 NaN guarantee preserved — verified by a new test).

`menu_screen_layout`:
- `fn camera_gameplay_layout() -> ScreenLayout` (FOV → Render Scale → Shadow Resolution →
  Dynamic Lights → Invert Y → Back).
- ids `ID_CG_FOV=0, ID_CG_RENDER_SCALE=1, ID_CG_SHADOW_RES=2, ID_CG_DYNAMIC_LIGHTS=3,
  ID_CG_INVERT_Y=4` (+ existing `ID_BACK=100`).

No public signatures changed; all additions are additive.

## Verify commands + results
Standalone gate (SHARED_RULES §3 documented floor for dependency-free `sim::` modules):

1. `rustc --edition 2021 --test settings_model.rs` (self-contained) →
   **14 passed / 0 failed** (8 pre-existing + 6 new r330 tests). Lib build warning-clean.
   New tests: `r330_new_fields_stay_in_range_across_fuzz`,
   `r330_render_scale_clamps_to_bounds`, `r330_dynamic_lights_steps_and_saturates`,
   `r330_shadow_resolution_ladder_steps_and_saturates`,
   `r330_nan_delta_keeps_new_fields_finite_and_in_range`,
   `r330_apply_kind_classification`.

2. `menu_screen_layout.rs` references `crate::sim::{menu_nav::NavDir, title_flow::Screen}`,
   so it was gated standalone via a shim that stubs **only those two unchanged upstream
   enums** (exact mirrors) → `rustc --edition 2021 --test` → **11 passed / 0 failed**
   (8 pre-existing + 3 new). Warning-clean. New tests:
   `r330_camera_page_lists_fov_then_new_fields_in_order`,
   `r330_camera_page_focus_starts_on_fov_and_cycles`,
   `r330_camera_page_has_back_and_is_deterministic`.

3. `CHANGES-r330-menus.patch` (`git diff`) — `git apply -p1 --check` **OK** against the base
   tree; applying reproduces the shipped `.rs` files byte-for-byte; `settings_model` re-gated
   from the patched tree → **14 passed / 0 failed**.

**Total task tests: 9 new (≥6 bar met), 25 passing across both modules.**

## Honesty / caveats
- Toolchain in-sandbox was **rustc 1.75** (apt), not 1.96 — no offline 1.96 bundle was
  present and `static.rust-lang.org`/`sh.rustup.rs` aren't allow-listed. The added code is
  plain std (no edition-2024/1.96-only features), edition 2021; the standalone `--test` gate
  is version-agnostic here. Coordinator's on-device cargo run is the real gate.
- No `cargo build` / `cargo test --features "render-core egui"` run (cargo absent; full-tree
  link overruns a turn per COORDINATOR_NOTES). `menu_screen_layout` was proven via the
  enum-stub shim; the stubs are byte-mirrors of the current upstream enums and are **not**
  part of the deliverable.
- **Pure data only.** No bin wiring, no shader, no visual claim ⇒ no PNG (none warranted).
  Live render-pipeline application is proposed in `HANDOFF_menus_bin_wiring.md`, not wired.
- `Screen` enum deliberately **not** extended (shared scaffolding); the Camera/Gameplay page
  ships as a free builder + a proposed route in the handoff.
- Pre-existing failures listed in SHARED_RULES untouched. `settings_schema.rs` persistence
  parity flagged in the handoff §4, intentionally out of scope (second shared model).

## Merge note
Merge slot per dispatch order: `… → T-menus → …` (self-contained core; only new-file/own-fn
additions to two `sim::` modules). Add a `## r331` ledger entry to `docs/WIRING_STATUS.md`
on merge.
