//! Glass — the restrained, high-alpha translucent surface treatment ported from rusty-board's GTK
//! `.glass-keyboard` CSS (macOS-Sonoma/iOS-Control-Center style vibrancy, NOT layered gradients/sheen).
//!
//! The GTK version deliberately avoids `backdrop-filter`/blur (GPU-fatal on mobile) and instead gets the
//! glass *feel* from three cheap ingredients: a high-alpha tinted fill (0.78–0.92, not the 0.45–0.55 the
//! first pass used — low alpha forces a full alpha-composite every frame), a single hairline top-edge
//! highlight instead of an inset shadow, and **no transition/animation on the glass surfaces themselves**
//! — presses are an instant colour flip, which reads as snappier, not less polished. This module is the
//! egui-side port of that recipe, for any "system profile" surface (gitmind, rusty-board) that wants the
//! glass look without the performance trap the CSS comment documents.
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui};

/// Blend `base` toward transparent at the given alpha (0–255), the way the CSS `alpha(@color, x)` does.
/// Kept a free function (no egui::Context needed) so the blend math is unit-testable.
pub fn tint(base: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha)
}

/// The alpha levels from the tuned GTK stylesheet, named by role. Kept as one table so a future surface
/// (or a future retune) changes in one place instead of per-callsite magic numbers.
pub struct GlassAlphas;
impl GlassAlphas {
    pub const SURFACE: u8 = 217; // 0.85 — the keyboard/panel root
    pub const KEY: u8 = 199; // 0.78 — a standard key/tile
    pub const KEY_HOVER: u8 = 224; // 0.88
    pub const KEY_MODIFIER: u8 = 140; // 0.55 — modifier/special keys sit subtly darker
    pub const KEY_MODIFIER_HOVER: u8 = 166; // 0.65
    pub const PRESSED_ACCENT: u8 = 191; // 0.75 — solid-feeling accent flip on press
    pub const OPAQUE_STRIP: u8 = 235; // 0.92 — text strips (microterm) stay near-opaque; readability first
    pub const HAIRLINE: u8 = 31; // 0.12 — the top-edge "light catching the bevel" highlight
    pub const BORDER: u8 = 89; // 0.35 — key border
    pub const BORDER_SOFT: u8 = 77; // 0.30 — modifier border / strip border
}

/// Fill colour for the glass root surface (keyboard/panel background) from a palette's card colour.
pub fn surface_fill(bg_card: Color32) -> Color32 {
    tint(bg_card, GlassAlphas::SURFACE)
}

/// Fill colour for a standard glass key/tile in its resting state.
pub fn key_fill(bg_card: Color32) -> Color32 {
    tint(bg_card, GlassAlphas::KEY)
}

/// Fill colour for a standard glass key/tile on hover.
pub fn key_fill_hover(bg_card: Color32) -> Color32 {
    tint(bg_card, GlassAlphas::KEY_HOVER)
}

/// Fill for a modifier/special key — subtly darker/less present than a letter key.
pub fn key_fill_modifier(bg_card: Color32) -> Color32 {
    tint(bg_card, GlassAlphas::KEY_MODIFIER)
}

/// Fill for a modifier/special key on hover.
pub fn key_fill_modifier_hover(bg_card: Color32) -> Color32 {
    tint(bg_card, GlassAlphas::KEY_MODIFIER_HOVER)
}

/// Fill for a pressed key: a solid-feeling accent flip, no transform/animation — matches the CSS's
/// deliberate "instant colour flip, no transition" pressed state.
pub fn key_fill_pressed(accent: Color32) -> Color32 {
    tint(accent, GlassAlphas::PRESSED_ACCENT)
}

/// Border stroke for a standard glass key.
pub fn key_border(border: Color32) -> Stroke {
    Stroke::new(1.0, tint(border, GlassAlphas::BORDER))
}

/// Border stroke for a modifier/special key (softer than a standard key's).
pub fn key_border_soft(border: Color32) -> Stroke {
    Stroke::new(1.0, tint(border, GlassAlphas::BORDER_SOFT))
}

/// Fill for a strip that must stay legible (e.g. a text/log strip) rather than glassy.
pub fn opaque_strip_fill(bg_base: Color32) -> Color32 {
    tint(bg_base, GlassAlphas::OPAQUE_STRIP)
}

/// Paint the single hairline top-edge highlight that reads as "light catching the bevel" — the glass
/// surface's one allowed embellishment, drawn as a 1px line rather than an inset shadow (which forces
/// offscreen compositing; see the CSS comment this ports from).
pub fn paint_top_hairline(ui: &Ui, rect: Rect) {
    let stroke = Stroke::new(1.0, Color32::from_white_alpha(GlassAlphas::HAIRLINE));
    ui.painter().hline(rect.x_range(), rect.top(), stroke);
}

/// Draw a glass-style key: flat tinted fill, hairline border, a solid accent flip when pressed, and the
/// same instant-feedback behaviour as the CSS (no easing on the fill colour itself — the *response*
/// object still animates position/scale via the motion kit if a caller wants that, but the glass fill is
/// a hard cut, matching the "transition: none" the mobile-perf rewrite settled on).
pub fn glass_key(ui: &mut Ui, rect: Rect, label: &str, bg_card: Color32, border: Color32, accent: Color32, text: Color32, modifier: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(label), Sense::click());
    let (fill, stroke) = if resp.is_pointer_button_down_on() {
        (key_fill_pressed(accent), Stroke::new(1.0, tint(border, GlassAlphas::BORDER)))
    } else if modifier {
        if resp.hovered() { (key_fill_modifier_hover(bg_card), key_border_soft(border)) } else { (key_fill_modifier(bg_card), key_border_soft(border)) }
    } else if resp.hovered() {
        (key_fill_hover(bg_card), key_border(border))
    } else {
        (key_fill(bg_card), key_border(border))
    };
    ui.painter().rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
    let text_color = if resp.is_pointer_button_down_on() { Color32::WHITE } else { text };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(14.0), text_color);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_preserves_rgb_and_sets_alpha() {
        let base = Color32::from_rgb(0x16, 0x16, 0x19);
        let t = tint(base, 200);
        // Color32 stores premultiplied RGB, so a round-trip through to_srgba_unmultiplied can be off by
        // a rounding unit at partial alpha — assert within tolerance rather than bit-exact.
        let [r, g, b, a] = t.to_srgba_unmultiplied();
        assert!((r as i16 - 0x16).abs() <= 1 && (g as i16 - 0x16).abs() <= 1 && (b as i16 - 0x19).abs() <= 1);
        assert_eq!(a, 200);
    }

    #[test]
    fn key_states_are_distinct_alphas() {
        let card = Color32::from_rgb(30, 30, 34);
        assert_ne!(key_fill(card).a(), key_fill_hover(card).a());
        assert_ne!(key_fill(card).a(), key_fill_modifier(card).a());
        assert!(key_fill_hover(card).a() > key_fill(card).a(), "hover is MORE present, matching the CSS 0.78→0.88");
        assert!(key_fill(card).a() > key_fill_modifier(card).a(), "modifiers sit subtly darker/less present");
    }

    #[test]
    fn pressed_uses_accent_not_card_color() {
        let accent = Color32::from_rgb(0xaa, 0xb0, 0xbb);
        let card = Color32::from_rgb(30, 30, 34);
        let pressed = key_fill_pressed(accent);
        let [pr, pg, pb, _] = pressed.to_srgba_unmultiplied();
        assert_eq!((pr, pg, pb), (accent.r(), accent.g(), accent.b()));
        assert_ne!((pr, pg, pb), (card.r(), card.g(), card.b()));
    }

    #[test]
    fn surface_is_higher_alpha_than_the_tuned_first_pass() {
        // the CSS comment: first pass was 0.45–0.55, the perf rewrite went to 0.78–0.92 — assert we
        // ported the REWRITE's numbers, not the original low-alpha ones.
        assert!(GlassAlphas::SURFACE > 140);
        assert!(GlassAlphas::KEY > 140);
        assert!(GlassAlphas::OPAQUE_STRIP > 200);
    }
}
