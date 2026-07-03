//! Theme — design tokens as Rust constants.
//!
//! These match the C++ Theme:: constants in Theme.hpp. Centralising them here
//! means any Rust UI backend gets the same visual language as the Qt app.
//!
//! Colors are in CSS hex format ("#rrggbb") so they're compatible with egui,
//! Slint, Tauri/web, and any other framework.

/// The SnapClone design system — warm off-white / dark charcoal palette.
#[derive(Debug, Clone)]
pub struct Theme {
    // ── Backgrounds ──────────────────────────────────────────────────────
    pub bg_deep:    &'static str, // darkest surface  (#111110)
    pub bg_base:    &'static str, // default page bg  (#1c1c1a)
    pub bg_raised:  &'static str, // cards, sheets    (#252523)
    pub bg_card:    &'static str, // content cards    (#2e2e2b)
    pub bg_hover:   &'static str, // hover / pressed  (#38382f)

    // ── Text ─────────────────────────────────────────────────────────────
    pub text_primary:   &'static str,  // main text       (#f5f4ef)
    pub text_secondary: &'static str,  // supporting text (#a8a79e)
    pub text_disabled:  &'static str,  // disabled / hint (#5a5a52)
    pub text_inverse:   &'static str,  // text on accent  (#111110)

    // ── Accent ───────────────────────────────────────────────────────────
    pub accent:     &'static str, // warm sand primary (#d4a853)
    pub accent_dim: &'static str, // pressed accent    (#b8913f)

    // ── Semantic ─────────────────────────────────────────────────────────
    pub success:  &'static str,  // online green      (#4caf50)
    pub warning:  &'static str,  // caution amber     (#ff9800)
    pub error:    &'static str,  // destructive red   (#f44336)

    // ── Spacing (px) ─────────────────────────────────────────────────────
    pub space_xs: f32,  // 4
    pub space_sm: f32,  // 8
    pub space_md: f32,  // 16
    pub space_lg: f32,  // 24
    pub space_xl: f32,  // 32

    // ── Typography ───────────────────────────────────────────────────────
    pub font_sm:   f32, // 12
    pub font_base: f32, // 14
    pub font_lg:   f32, // 18
    pub font_xl:   f32, // 22

    // ── Shape ────────────────────────────────────────────────────────────
    pub radius_sm: f32, // 8
    pub radius_md: f32, // 12
    pub radius_lg: f32, // 16
    pub radius_xl: f32, // 24

    // ── Touch targets ────────────────────────────────────────────────────
    pub touch_min: f32, // 44 (minimum tap target)
}

impl Theme {
    /// The canonical SnapClone dark theme.
    pub const fn dark() -> Self {
        Self {
            bg_deep:    "#111110",
            bg_base:    "#1c1c1a",
            bg_raised:  "#252523",
            bg_card:    "#2e2e2b",
            bg_hover:   "#38382f",
            text_primary:   "#f5f4ef",
            text_secondary: "#a8a79e",
            text_disabled:  "#5a5a52",
            text_inverse:   "#111110",
            accent:     "#d4a853",
            accent_dim: "#b8913f",
            success:  "#4caf50",
            warning:  "#ff9800",
            error:    "#f44336",
            space_xs: 4.0,
            space_sm: 8.0,
            space_md: 16.0,
            space_lg: 24.0,
            space_xl: 32.0,
            font_sm:   12.0,
            font_base: 14.0,
            font_lg:   18.0,
            font_xl:   22.0,
            radius_sm: 8.0,
            radius_md: 12.0,
            radius_lg: 16.0,
            radius_xl: 24.0,
            touch_min: 44.0,
        }
    }
}

/// Global singleton access pattern — use Theme::dark() or provide your own.
pub static DEFAULT_THEME: Theme = Theme::dark();
