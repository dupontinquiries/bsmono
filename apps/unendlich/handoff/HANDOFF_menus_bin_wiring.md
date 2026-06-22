# HANDOFF — r330-menus bin wiring (PROPOSE, do not apply in-sandbox)

**WID:** `r330-menus` · **Pure deliverables:** `sim::settings_model`, `sim::menu_screen_layout`
This file proposes the bin/render-side wiring. Per SHARED_RULES §2 the worker does **not**
touch `render/world_renderer.rs` (glfw/egui path can't link in-sandbox); the coordinator
applies + validates on-device.

---

## 1. New first-class settings fields (now live in the pure model)

`sim::settings_model::Settings` gained three validated fields:

| field | type | range / values | `apply_kind` |
|---|---|---|---|
| `render_scale` | `f32` | clamp `0.5 ..= 2.0`, NaN-safe | `NeedsPipelineRebuild` |
| `shadow_resolution` | `ShadowRes` enum | `R4096` / `R8192` / `R16384` (`.as_u32()`) | `NeedsPipelineRebuild` |
| `dynamic_lights` | `u32` | clamp `0 ..= 64` (count) | `NeedsPipelineRebuild` |

Adjust them through the existing NaN-safe `adjust(&mut Settings, SettingField, f32)` path —
`SettingField::{RenderScale, ShadowResolution, DynamicLights}`. `ShadowResolution` steps the
discrete ladder by the **sign** of the delta (saturating at the ends); the other two add the
delta and clamp.

`apply_kind(SettingField) -> ApplyKind` tells the bin whether a knob hot-applies (`Live`) or
needs a pipeline/swapchain rebuild (`NeedsPipelineRebuild`). render-distance stays `Live`;
vsync is `NeedsPipelineRebuild` (swapchain recreate). Use it to gate the rebuild path so a
FOV/render-distance nudge doesn't tear down the pipeline.

## 2. `render/world_renderer.rs` staged `ui_*` fields (propose)

The renderer already stages some of these — wire the rest to mirror the existing pattern.

- **Field decls (~L528 neighbourhood, next to `ui_render_distance` / `ui_render_scale`):**
  `ui_render_scale: f32` **already exists (r325)** — repoint it at
  `settings_model` clamping (`RS_MIN/RS_MAX = 0.5/2.0`) instead of the ad-hoc `0.5..1.0`
  comment, and treat its writeback as `NeedsPipelineRebuild`.
  Add, following the same `pub ui_*:` style:
  ```rust
  /// r330: shadow-map resolution ladder (4096/8192/16384). Rebuild on change.
  pub ui_shadow_res: u32,
  /// r330: dynamic-light budget (count, 0..=64). Rebuild on change.
  pub ui_dynamic_lights: u32,
  /// r330: FOV slider for the Camera/Gameplay page (50..=110).
  pub ui_fov_deg: f32,
  ```
- **Init (~L1465, beside `ui_render_distance: 6,`):**
  ```rust
  ui_shadow_res: 8192,
  ui_dynamic_lights: 16,
  ui_fov_deg: 90.0,
  ```
  (mirror `settings_model::defaults()` so the model and the staged UI agree on first paint.)
- **Write-back (~L4333, in the `// r169 write-backs.` block beside `self.ui_shadows_on = ...`):**
  ```rust
  // r330: clamp through the pure model, classify the apply path.
  // (pseudo — adapt to the local `let mut ui_*` staging vars in the egui block)
  if settings_dirty {
      let kind = settings_model::apply_kind(changed_field);
      self.ui_shadow_res     = ui_shadow_res;
      self.ui_dynamic_lights = ui_dynamic_lights;
      self.ui_fov_deg        = ui_fov_deg;
      if kind == settings_model::ApplyKind::NeedsPipelineRebuild {
          self.request_pipeline_rebuild = true; // existing swapchain-recreate path
      }
  }
  ```
  The staged `let mut ui_* = self.ui_*;` reads at ~L3036/3105 are where the egui sliders
  read/write each frame — add the three new locals there too.

**Recommended:** keep one `settings_model::Settings` as the source of truth and derive the
`ui_*` scalars from it on open / write them back on close, rather than tracking each scalar
independently. `SettingsEditor` already gives you draft/apply/revert for the Cancel button.

## 3. Camera / Gameplay page route (propose)

`sim::menu_screen_layout::camera_gameplay_layout()` builds the page (FOV → Render Scale →
Shadow Resolution → Dynamic Lights → Invert Y → Back) with stable ids
`ID_CG_FOV..=ID_CG_INVERT_Y` + `ID_BACK`. It is intentionally a **free builder, not a new
`Screen` variant** (SHARED_RULES §4: don't restructure the shared `title_flow::Screen` enum).

To make it reachable, the coordinator adds the route in shared files:
1. `sim/title_flow.rs`: add `Screen::CameraGameplay` (+ any transition from `Settings`).
2. `sim/menu_screen_layout.rs::layout_for_title`: add an arm
   `Screen::CameraGameplay => camera_gameplay_layout(),`.
3. Optionally add a "Camera / Gameplay" button to the `Screen::Settings` widget list that
   transitions to it.

Map focused widget id → `SettingField` in the bin:
`ID_CG_FOV→Fov`, `ID_CG_RENDER_SCALE→RenderScale`, `ID_CG_SHADOW_RES→ShadowResolution`,
`ID_CG_DYNAMIC_LIGHTS→DynamicLights`, `ID_CG_INVERT_Y→InvertY`.

## 4. Optional: `settings_schema.rs` parity (coordinator call)

`settings_schema::GraphicsSettings` is the **persistence** model (KV round-trip). If these
knobs should survive a restart, add `render_scale` / `shadow_size` (it already has
`shadow_size`!) / `dynamic_lights` there too and extend `to_kv`/`from_kv`/`clamped`/`validate`.
Left out of this worker's scope to avoid touching a second shared model; flagging it so the
two models don't silently drift.
