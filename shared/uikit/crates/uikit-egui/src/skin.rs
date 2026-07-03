//! Skin — the L4 "what the look is" token struct, ported from
//! `apps/logscope/crates/logscope-egui/src/theme.rs::Skin`.
//!
//! The whole point of the L0→L4 sandwich (see crate docs) is that a *style*
//! iteration only ever edits these values: L2 painters read them, L3 widget
//! names are unchanged, call sites are unchanged. New look, no ripple.
//!
//! `Skin::carved()` reproduces logscope's shipped "carved material": rounded,
//! translucent accent-tinted fills with a lit top bevel + shadowed bottom
//! bevel for relief, a faint material grain, an animated specular sheen, and
//! engraved (carved-into-the-surface) text. Values are the exact ones from the
//! original `carved()` so the two apps render identically.

/// All-numeric style tokens for the carved surface. Opacities are 0..=255.
#[derive(Debug, Clone, Copy)]
pub struct Skin {
    /// Corner radius (px).
    pub radius: u8,
    /// Translucent base fill opacity by interaction state.
    pub fill_rest: u8,
    pub fill_hover: u8,
    pub fill_down: u8,
    pub fill_off: u8,
    /// How much the accent tints the fill (0..1).
    pub accent_tint: f32,
    /// Material speckle strength (alpha).
    pub grain_alpha: u8,
    /// Relief edges: lit top, shadowed bottom (alpha).
    pub bevel_lit: u8,
    pub bevel_shadow: u8,
    /// Wet specular highlight strength + sweep speed.
    pub sheen: f32,
    pub sheen_speed: f32,
    /// Mineral-vein text-glow scale.
    pub vein: f32,
    /// Carved-text relief: shadow below the ink + lit lip above (alpha).
    pub engrave_shadow: u8,
    pub engrave_lip: u8,
}

impl Skin {
    /// The shipped logscope look. Exact values from
    /// `logscope-egui theme::Skin::carved()`.
    pub const fn carved() -> Self {
        Skin {
            radius: 8,
            fill_rest: 104,
            fill_hover: 174,
            fill_down: 165,
            fill_off: 70,
            accent_tint: 0.20,
            grain_alpha: 26,
            bevel_lit: 26,
            bevel_shadow: 58,
            sheen: 1.0,
            sheen_speed: 1.3,
            vein: 1.0,
            engrave_shadow: 130,
            engrave_lip: 40,
        }
    }

    /// A flat fallback (sheen/grain/vein off, tiny radius) for low-power
    /// targets — e.g. the RISC-V boards where overdraw is precious. Same call
    /// sites, cheaper paint.
    pub const fn flat() -> Self {
        Skin {
            radius: 4,
            sheen: 0.0,
            grain_alpha: 0,
            vein: 0.0,
            bevel_lit: 18,
            bevel_shadow: 36,
            ..Skin::carved()
        }
    }
}

impl Default for Skin {
    fn default() -> Self {
        Skin::carved()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carved_tokens_are_sane() {
        let s = Skin::carved();
        assert!(s.fill_rest < s.fill_hover);
        assert!(s.fill_off <= s.fill_rest);
        assert!(s.radius > 0 && s.grain_alpha > 0 && s.bevel_lit > 0 && s.bevel_shadow > 0);
        assert!(s.sheen > 0.0 && s.sheen_speed > 0.0 && s.vein > 0.0);
        assert!(s.engrave_shadow > 0 && s.engrave_lip > 0 && (0.0..=1.0).contains(&s.accent_tint));
    }

    #[test]
    fn flat_drops_the_expensive_bits() {
        let f = Skin::flat();
        assert_eq!(f.sheen, 0.0);
        assert_eq!(f.grain_alpha, 0);
        assert!(f.radius < Skin::carved().radius);
    }
}
