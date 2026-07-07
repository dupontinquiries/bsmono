//! `sim::settings_model` — settings/options screen model (WID r63-04).
//!
//! The model behind the Settings screen reachable from the main menu and the
//! in-game pause overlay (replaces the lost `options_model` round). Holds the
//! tunable values, clamps them to valid ranges, nudges a field by a delta, and
//! provides a draft/apply/revert editor so the screen's Cancel button works.
//!
//! Pure: logic in, data out — no `crate::render`, no GPU, no I/O, no clock, no
//! RNG, no `.unwrap()`/`.expect()` in non-test code. Release-safe (no
//! `#[cfg]`-gated helpers); compiles clean under `cargo check --release`.

/// User-tunable settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub render_distance: u8,
    pub vsync: bool,
    pub fov_deg: f32,
    pub master_volume01: f32,
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
    // r330-menus: graphics knobs promoted to first-class, validated fields.
    pub render_scale: f32,
    pub shadow_resolution: ShadowRes,
    pub dynamic_lights: u32,
}

/// Which field a UI control adjusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingField {
    RenderDistance,
    Vsync,
    Fov,
    Volume,
    Sensitivity,
    InvertY,
    // r330-menus: new graphics knobs adjustable from the settings screen.
    RenderScale,
    ShadowResolution,
    DynamicLights,
}

// ---- r330-menus -------------------------------------------------------------

/// Discrete shadow-map resolution ladder (px edge). Always valid by
/// construction, so it needs no clamping — only laddered stepping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowRes {
    R4096,
    R8192,
    R16384,
}

impl ShadowRes {
    /// The shadow-map edge length in pixels.
    pub fn as_u32(self) -> u32 {
        match self {
            ShadowRes::R4096 => 4096,
            ShadowRes::R8192 => 8192,
            ShadowRes::R16384 => 16384,
        }
    }

    /// Step one rung up (`delta > 0`) or down (`delta < 0`) the ladder,
    /// saturating at the ends. A `0.0` or `NaN` delta leaves it unchanged
    /// (every comparison with NaN is false, so neither branch is taken).
    fn stepped(self, delta: f32) -> ShadowRes {
        if delta > 0.0 {
            match self {
                ShadowRes::R4096 => ShadowRes::R8192,
                ShadowRes::R8192 => ShadowRes::R16384,
                ShadowRes::R16384 => ShadowRes::R16384,
            }
        } else if delta < 0.0 {
            match self {
                ShadowRes::R16384 => ShadowRes::R8192,
                ShadowRes::R8192 => ShadowRes::R4096,
                ShadowRes::R4096 => ShadowRes::R4096,
            }
        } else {
            self
        }
    }
}

/// How a changed setting reaches the live renderer — the bin reads this to know
/// whether a knob hot-applies or forces a pipeline/swapchain rebuild (§4e).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyKind {
    /// Hot-applies on the next frame without rebuilding the pipeline.
    Live,
    /// Requires recreating the render pipeline / swapchain to take effect.
    NeedsPipelineRebuild,
}

/// Classify how the bin must apply a changed field. `render_scale`, shadow
/// resolution and the dynamic-light budget all change pipeline/attachment state
/// ⇒ rebuild; `vsync` recreates the swapchain ⇒ rebuild; render distance and the
/// remaining knobs hot-apply live. Pure total function (exhaustive match).
pub fn apply_kind(f: SettingField) -> ApplyKind {
    match f {
        SettingField::RenderScale
        | SettingField::ShadowResolution
        | SettingField::DynamicLights
        | SettingField::Vsync => ApplyKind::NeedsPipelineRebuild,
        SettingField::RenderDistance
        | SettingField::Fov
        | SettingField::Volume
        | SettingField::Sensitivity
        | SettingField::InvertY => ApplyKind::Live,
    }
}

// Valid ranges (documented).
const RD_MIN: u8 = 2;
const RD_MAX: u8 = 24;
const FOV_MIN: f32 = 50.0;
const FOV_MAX: f32 = 110.0;
// r330-menus ranges.
const RS_MIN: f32 = 0.25; // render scale — r336-neural-bonus: 25% floor (4x neural upscale)
const RS_MAX: f32 = 2.0;
const DL_MIN: u32 = 0; // dynamic light budget (count)
const DL_MAX: u32 = 64;

/// r336-neural-bonus: how much the neural upscaler must magnify the render target,
/// = 1/render_scale (0.25→4x, 0.5→2x, 1.0→1x). Clamped to the supported floor so a
/// stray scale can't request an unbounded factor.
pub fn upscale_factor(render_scale: f32) -> f32 {
    1.0 / clamp_f(render_scale, RS_MIN, RS_MAX)
}

/// The discrete neural-upscale mode the pipeline selects for a render scale:
/// `4` for ≤0.25 (quarter-res), `2` for ≤0.5 (half-res), else `1` (native).
pub fn upscale_mode(render_scale: f32) -> u8 {
    let rs = clamp_f(render_scale, RS_MIN, RS_MAX);
    if rs <= 0.25 + 1e-4 {
        4
    } else if rs <= 0.5 + 1e-4 {
        2
    } else {
        1
    }
}

/// The default settings (all in range).
pub fn defaults() -> Settings {
    Settings {
        render_distance: 8,
        vsync: true,
        fov_deg: 90.0,
        master_volume01: 0.8,
        mouse_sensitivity: 0.5,
        invert_y: false,
        // r330-menus
        render_scale: 1.0,
        shadow_resolution: ShadowRes::R8192,
        dynamic_lights: 16,
    }
}

/// NaN-safe clamp into `[lo, hi]` (NaN ⇒ the band midpoint, so the value is
/// always finite and in range).
fn clamp_f(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_nan() {
        (lo + hi) * 0.5
    } else {
        v.max(lo).min(hi)
    }
}

/// Clamp every field into its valid range. Idempotent.
pub fn clamp(s: &mut Settings) {
    s.render_distance = s.render_distance.clamp(RD_MIN, RD_MAX);
    s.fov_deg = clamp_f(s.fov_deg, FOV_MIN, FOV_MAX);
    s.master_volume01 = clamp_f(s.master_volume01, 0.0, 1.0);
    s.mouse_sensitivity = clamp_f(s.mouse_sensitivity, 0.0, 1.0);
    // r330-menus: render_scale is NaN-safe clamped; dynamic_lights saturates.
    // shadow_resolution is an enum ⇒ valid by construction, nothing to clamp.
    s.render_scale = clamp_f(s.render_scale, RS_MIN, RS_MAX);
    s.dynamic_lights = s.dynamic_lights.clamp(DL_MIN, DL_MAX);
    // vsync / invert_y are bool — nothing to clamp.
}

/// Nudge a field by `delta`, then clamp. Numeric fields add `delta` (render
/// distance rounds to the nearest step); bool fields toggle on a nonzero delta.
pub fn adjust(s: &mut Settings, f: SettingField, delta: f32) {
    match f {
        SettingField::RenderDistance => {
            // `NaN as i32` is 0 in Rust, so a NaN delta is a safe no-op.
            let step = delta.round() as i32;
            let v = s.render_distance as i32 + step;
            let v = v.clamp(RD_MIN as i32, RD_MAX as i32);
            s.render_distance = v as u8;
        }
        SettingField::Fov => s.fov_deg += delta,
        SettingField::Volume => s.master_volume01 += delta,
        SettingField::Sensitivity => s.mouse_sensitivity += delta,
        // `NaN.abs() > 0.0` is false ⇒ a NaN delta does not toggle.
        SettingField::Vsync => {
            if delta.abs() > 0.0 {
                s.vsync = !s.vsync;
            }
        }
        SettingField::InvertY => {
            if delta.abs() > 0.0 {
                s.invert_y = !s.invert_y;
            }
        }
        // r330-menus knobs.
        SettingField::RenderScale => s.render_scale += delta,
        SettingField::ShadowResolution => {
            s.shadow_resolution = s.shadow_resolution.stepped(delta);
        }
        SettingField::DynamicLights => {
            // `NaN as i32`/`NaN.round()` is 0, so a NaN delta is a safe no-op.
            // i64 math keeps a saturated `delta.round() as i32` from overflowing
            // when added to the current count before the clamp back into range.
            let step = delta.round() as i32;
            let v = s.dynamic_lights as i64 + step as i64;
            let v = v.clamp(DL_MIN as i64, DL_MAX as i64);
            s.dynamic_lights = v as u32;
        }
    }
    clamp(s);
}

/// Draft/commit editor so the Settings screen can Apply or Cancel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsEditor {
    committed: Settings,
    draft: Settings,
}

impl SettingsEditor {
    /// Start editing from `initial` (clamped on the way in).
    pub fn new(initial: Settings) -> Self {
        let mut c = initial;
        clamp(&mut c);
        SettingsEditor { committed: c, draft: c }
    }

    /// The current (uncommitted) draft.
    pub fn draft(&self) -> Settings {
        self.draft
    }

    /// The last committed settings.
    pub fn committed(&self) -> Settings {
        self.committed
    }

    /// Nudge a draft field (clamped).
    pub fn adjust(&mut self, f: SettingField, delta: f32) {
        adjust(&mut self.draft, f, delta);
    }

    /// Commit the draft as the new committed settings.
    pub fn apply(&mut self) {
        self.committed = self.draft;
    }

    /// Discard draft edits, restoring the committed settings.
    pub fn revert(&mut self) {
        self.draft = self.committed;
    }

    /// Whether the draft differs from the committed settings.
    pub fn dirty(&self) -> bool {
        self.draft != self.committed
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn upscale_factor_and_mode_track_render_scale() {
        use super::{upscale_factor, upscale_mode};
        assert!((upscale_factor(0.25) - 4.0).abs() < 1e-3, "quarter-res → 4x");
        assert!((upscale_factor(0.5) - 2.0).abs() < 1e-3, "half-res → 2x");
        assert!((upscale_factor(1.0) - 1.0).abs() < 1e-3, "native → 1x");
        assert_eq!(upscale_mode(0.25), 4);
        assert_eq!(upscale_mode(0.5), 2);
        assert_eq!(upscale_mode(1.0), 1);
        // floor: an out-of-range tiny scale clamps to the 25% / 4x floor, never higher.
        assert_eq!(upscale_mode(0.05), 4);
        assert!(upscale_factor(0.05) <= 4.0 + 1e-3);
    }

    use super::*;

    fn in_range(s: &Settings) -> bool {
        (RD_MIN..=RD_MAX).contains(&s.render_distance)
            && (FOV_MIN..=FOV_MAX).contains(&s.fov_deg)
            && (0.0..=1.0).contains(&s.master_volume01)
            && (0.0..=1.0).contains(&s.mouse_sensitivity)
            && s.fov_deg.is_finite()
            && s.master_volume01.is_finite()
            && s.mouse_sensitivity.is_finite()
            // r330-menus
            && (RS_MIN..=RS_MAX).contains(&s.render_scale)
            && s.render_scale.is_finite()
            && (DL_MIN..=DL_MAX).contains(&s.dynamic_lights)
    }

    #[test]
    fn defaults_are_in_range_and_clamp_idempotent() {
        let d = defaults();
        assert!(in_range(&d));
        let mut c = d;
        clamp(&mut c);
        assert_eq!(c, d, "defaults already in range ⇒ clamp is a no-op");
    }

    #[test]
    fn adjust_always_clamps_into_range() {
        let fields = [
            SettingField::RenderDistance,
            SettingField::Fov,
            SettingField::Volume,
            SettingField::Sensitivity,
        ];
        for &f in &fields {
            for &delta in &[-1000.0f32, -3.3, -1.0, 0.0, 1.0, 7.7, 1000.0] {
                let mut s = defaults();
                adjust(&mut s, f, delta);
                assert!(in_range(&s), "field {f:?} delta {delta} ⇒ out of range {s:?}");
            }
        }
        // explicit extremes
        let mut s = defaults();
        adjust(&mut s, SettingField::RenderDistance, 100.0);
        assert_eq!(s.render_distance, RD_MAX);
        adjust(&mut s, SettingField::RenderDistance, -100.0);
        assert_eq!(s.render_distance, RD_MIN);
        adjust(&mut s, SettingField::Fov, 1000.0);
        assert_eq!(s.fov_deg, FOV_MAX);
        adjust(&mut s, SettingField::Volume, 5.0);
        assert_eq!(s.master_volume01, 1.0);
        adjust(&mut s, SettingField::Volume, -5.0);
        assert_eq!(s.master_volume01, 0.0);
    }

    #[test]
    fn render_distance_steps_by_rounded_delta() {
        let mut s = defaults(); // rd = 8
        adjust(&mut s, SettingField::RenderDistance, 1.0);
        assert_eq!(s.render_distance, 9);
        adjust(&mut s, SettingField::RenderDistance, -2.0);
        assert_eq!(s.render_distance, 7);
        adjust(&mut s, SettingField::RenderDistance, 0.4); // rounds to 0
        assert_eq!(s.render_distance, 7);
        adjust(&mut s, SettingField::RenderDistance, 0.6); // rounds to +1
        assert_eq!(s.render_distance, 8);
    }

    #[test]
    fn bool_fields_toggle_on_nonzero_delta() {
        let mut s = defaults();
        let (v0, i0) = (s.vsync, s.invert_y);
        adjust(&mut s, SettingField::Vsync, 1.0);
        assert_eq!(s.vsync, !v0, "vsync toggled");
        adjust(&mut s, SettingField::Vsync, 0.0);
        assert_eq!(s.vsync, !v0, "zero delta ⇒ no toggle");
        adjust(&mut s, SettingField::InvertY, -1.0);
        assert_eq!(s.invert_y, !i0, "invert_y toggled");
        // NaN delta is a no-op for bools
        adjust(&mut s, SettingField::InvertY, f32::NAN);
        assert_eq!(s.invert_y, !i0, "NaN ⇒ no toggle");
    }

    #[test]
    fn nan_delta_keeps_numeric_fields_finite_and_in_range() {
        let mut s = defaults();
        adjust(&mut s, SettingField::Fov, f32::NAN);
        adjust(&mut s, SettingField::Volume, f32::NAN);
        adjust(&mut s, SettingField::Sensitivity, f32::NAN);
        adjust(&mut s, SettingField::RenderDistance, f32::NAN);
        assert!(in_range(&s), "NaN delta sanitized ⇒ still in range {s:?}");
        assert_eq!(s.render_distance, 8, "NaN render-distance delta is a no-op");
    }

    #[test]
    fn editor_apply_commits_and_clears_dirty() {
        let mut ed = SettingsEditor::new(defaults());
        assert!(!ed.dirty(), "fresh editor is clean");
        ed.adjust(SettingField::Fov, 5.0);
        assert!(ed.dirty(), "edited draft is dirty");
        assert_ne!(ed.draft(), ed.committed());
        ed.apply();
        assert!(!ed.dirty(), "apply clears dirty");
        assert_eq!(ed.committed(), ed.draft(), "apply commits the draft");
        assert_eq!(ed.committed().fov_deg, 95.0);
    }

    #[test]
    fn editor_revert_restores_committed() {
        let mut ed = SettingsEditor::new(defaults());
        ed.adjust(SettingField::Volume, -0.3);
        ed.adjust(SettingField::Vsync, 1.0);
        assert!(ed.dirty());
        ed.revert();
        assert!(!ed.dirty(), "revert clears dirty");
        assert_eq!(ed.draft(), defaults(), "revert restores committed values");
    }

    #[test]
    fn new_clamps_incoming_and_deterministic() {
        let wild = Settings {
            render_distance: 250,
            vsync: false,
            fov_deg: 999.0,
            master_volume01: -2.0,
            mouse_sensitivity: f32::NAN,
            invert_y: true,
            // r330-menus: out-of-range render_scale / dynamic_lights too.
            render_scale: 99.0,
            shadow_resolution: ShadowRes::R16384,
            dynamic_lights: 9999,
        };
        let ed = SettingsEditor::new(wild);
        assert!(in_range(&ed.committed()) && in_range(&ed.draft()), "new() clamps");
        // deterministic
        let mut a = defaults();
        let mut b = defaults();
        adjust(&mut a, SettingField::Sensitivity, 0.25);
        adjust(&mut b, SettingField::Sensitivity, 0.25);
        assert_eq!(a, b);
    }

    // ---- r330-menus tests ---------------------------------------------------

    #[test]
    fn r330_new_fields_stay_in_range_across_fuzz() {
        let fields = [SettingField::RenderScale, SettingField::DynamicLights];
        for &f in &fields {
            for &delta in &[-1e9f32, -10.0, -0.4, 0.0, 0.4, 10.0, 1e9, f32::NAN] {
                let mut s = defaults();
                adjust(&mut s, f, delta);
                assert!(in_range(&s), "field {f:?} delta {delta} ⇒ out of range {s:?}");
            }
        }
    }

    #[test]
    fn r330_render_scale_clamps_to_bounds() {
        let mut s = defaults();
        adjust(&mut s, SettingField::RenderScale, 100.0);
        assert_eq!(s.render_scale, RS_MAX);
        adjust(&mut s, SettingField::RenderScale, -100.0);
        assert_eq!(s.render_scale, RS_MIN);
        // a small in-range nudge moves by exactly the delta.
        let mut s = defaults(); // render_scale = 1.0
        adjust(&mut s, SettingField::RenderScale, 0.25);
        assert_eq!(s.render_scale, 1.25);
    }

    #[test]
    fn r330_dynamic_lights_steps_and_saturates() {
        let mut s = defaults(); // dynamic_lights = 16
        adjust(&mut s, SettingField::DynamicLights, 1.0);
        assert_eq!(s.dynamic_lights, 17);
        adjust(&mut s, SettingField::DynamicLights, -2.0);
        assert_eq!(s.dynamic_lights, 15);
        adjust(&mut s, SettingField::DynamicLights, 0.4); // rounds to 0 ⇒ no-op
        assert_eq!(s.dynamic_lights, 15);
        adjust(&mut s, SettingField::DynamicLights, 1e9); // saturates at DL_MAX
        assert_eq!(s.dynamic_lights, DL_MAX);
        adjust(&mut s, SettingField::DynamicLights, -1e9); // saturates at DL_MIN
        assert_eq!(s.dynamic_lights, DL_MIN);
    }

    #[test]
    fn r330_shadow_resolution_ladder_steps_and_saturates() {
        // edge mapping
        assert_eq!(ShadowRes::R4096.as_u32(), 4096);
        assert_eq!(ShadowRes::R8192.as_u32(), 8192);
        assert_eq!(ShadowRes::R16384.as_u32(), 16384);
        // step up from the bottom, saturating at the top
        let mut s = defaults();
        s.shadow_resolution = ShadowRes::R4096;
        adjust(&mut s, SettingField::ShadowResolution, 1.0);
        assert_eq!(s.shadow_resolution, ShadowRes::R8192);
        adjust(&mut s, SettingField::ShadowResolution, 1.0);
        assert_eq!(s.shadow_resolution, ShadowRes::R16384);
        adjust(&mut s, SettingField::ShadowResolution, 1.0);
        assert_eq!(s.shadow_resolution, ShadowRes::R16384, "saturates at top");
        // step down, saturating at the bottom
        adjust(&mut s, SettingField::ShadowResolution, -1.0);
        assert_eq!(s.shadow_resolution, ShadowRes::R8192);
        adjust(&mut s, SettingField::ShadowResolution, -1.0);
        adjust(&mut s, SettingField::ShadowResolution, -1.0);
        assert_eq!(s.shadow_resolution, ShadowRes::R4096, "saturates at bottom");
        // 0.0 and NaN deltas are no-ops
        let before = s.shadow_resolution;
        adjust(&mut s, SettingField::ShadowResolution, 0.0);
        adjust(&mut s, SettingField::ShadowResolution, f32::NAN);
        assert_eq!(s.shadow_resolution, before, "0/NaN ⇒ no ladder move");
    }

    #[test]
    fn r330_nan_delta_keeps_new_fields_finite_and_in_range() {
        let mut s = defaults();
        let before = s;
        adjust(&mut s, SettingField::RenderScale, f32::NAN);
        adjust(&mut s, SettingField::DynamicLights, f32::NAN);
        adjust(&mut s, SettingField::ShadowResolution, f32::NAN);
        assert!(in_range(&s), "NaN delta sanitized ⇒ still in range {s:?}");
        // render_scale is an f32: a NaN delta sanitizes to the finite band
        // midpoint (same NaN-safe rule as fov/volume), never NaN/out-of-range.
        assert!(s.render_scale.is_finite());
        assert_eq!(s.render_scale, (RS_MIN + RS_MAX) * 0.5, "NaN render_scale ⇒ midpoint");
        // the integer/enum knobs treat a NaN delta as a true no-op.
        assert_eq!(s.dynamic_lights, before.dynamic_lights, "NaN dyn-lights ⇒ no-op");
        assert_eq!(s.shadow_resolution, before.shadow_resolution, "NaN shadow ⇒ no-op");
    }

    #[test]
    fn r330_apply_kind_classification() {
        // Pipeline/swapchain-affecting knobs need a rebuild.
        for f in [
            SettingField::RenderScale,
            SettingField::ShadowResolution,
            SettingField::DynamicLights,
            SettingField::Vsync,
        ] {
            assert_eq!(apply_kind(f), ApplyKind::NeedsPipelineRebuild, "{f:?} rebuilds");
        }
        // Hot-applying knobs.
        for f in [
            SettingField::RenderDistance,
            SettingField::Fov,
            SettingField::Volume,
            SettingField::Sensitivity,
            SettingField::InvertY,
        ] {
            assert_eq!(apply_kind(f), ApplyKind::Live, "{f:?} is live");
        }
    }
}
