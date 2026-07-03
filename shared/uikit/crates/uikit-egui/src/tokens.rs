//! tokens — spacing / type / radius / touch / animation constants.
//!
//! These mirror `snapclone-theme/src/tokens.rs`, which itself mirrors the
//! authoritative C++ `include/ui/Theme.hpp` (hand-tuned down for the FP5 small
//! screen). Re-stated here as plain consts so the egui widgets and the
//! gesture/spring timings read from one place. When Theme.hpp changes, both
//! tables update together.

// Spacing scale (px) — Theme:: SPACE_*
pub const SPACE_XS: i32 = 6;
pub const SPACE_SM: i32 = 10;
pub const SPACE_MD: i32 = 18;
pub const SPACE_LG: i32 = 28;
pub const SPACE_XL: i32 = 43;

// Type scale (px) — Theme:: FONT_* (base 13 after the FP5 tightening)
pub const FONT_XS: i32 = 10;
pub const FONT_SM: i32 = 11;
pub const FONT_BASE: i32 = 13;
pub const FONT_LG: i32 = 16;
pub const FONT_XL: i32 = 20;
pub const FONT_XXL: i32 = 25;

// Corner radius (px)
pub const RADIUS_SM: i32 = 8;
pub const RADIUS_MD: i32 = 14;
pub const RADIUS_LG: i32 = 20;
pub const RADIUS_XL: i32 = 28;
pub const RADIUS_FULL: i32 = 999;

// Touch target sizes (px)
pub const TOUCH_MIN: i32 = 40;
pub const TOUCH_ICON: i32 = 52;
pub const TOUCH_LARGE: i32 = 64;

// Animation durations (ms)
pub const ANIM_FAST: i32 = 110;
pub const ANIM_NORMAL: i32 = 220;
pub const ANIM_SLOW: i32 = 340;

/// OutBack easing overshoot factor for page switches (the spring on snapclone's
/// page transitions). Used by the app's page animator.
pub const SPRING_OVERSHOOT: f64 = 1.08;

/// OutBack easing: overshoots past 1.0 by `overshoot` then settles. `t` in
/// 0..=1. This is the curve snapclone's SwipeNavigator uses for page commits.
pub fn out_back(t: f64, overshoot: f64) -> f64 {
    let c1 = 1.701_58 * overshoot;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_back_endpoints_and_overshoot() {
        assert!((out_back(0.0, SPRING_OVERSHOOT) - 0.0).abs() < 1e-6);
        assert!((out_back(1.0, SPRING_OVERSHOOT) - 1.0).abs() < 1e-6);
        // it should exceed 1.0 somewhere in the middle (the spring overshoot)
        let peak = (1..100)
            .map(|i| out_back(i as f64 / 100.0, SPRING_OVERSHOOT))
            .fold(f64::MIN, f64::max);
        assert!(peak > 1.0, "OutBack should overshoot past 1.0, peak={peak}");
    }
}
