//! overlay — floating panels that read as floating.
//!
//! A panel painted with `rect_filled(area, r, bg_raised)` and nothing else
//! has three problems that all read as "crowded", and every app in this
//! tree has hit at least one:
//!
//! 1. **No separation from what is behind it.** `bg_raised` against
//!    `bg_base` is a few points of luminance in most palettes. Without a
//!    border, and without dimming what is behind, the panel and the page
//!    are one busy field. [`scrim`] and the card's stroke fix that.
//! 2. **Padding that is one number.** `area.shrink(12.0)` gives a text
//!    field 12 pt of air on every side and gives the ROWS under it 12 pt
//!    too — but rows need less at the sides and more between each other.
//!    [`CardStyle`] separates the pad from the gap, and [`card`] applies
//!    both by setting `item_spacing` on the child.
//! 3. **Nothing keeps it on screen.** A caption anchored under a focused
//!    widget near the bottom edge draws off the bottom. [`fit_on_screen`]
//!    is the clamp, and it is pure, so it is a test rather than a report
//!    from a phone.
//!
//! The geometry here is pure and tested; what it paints is checkable with
//! `layout_probe` from the calling app.

use crate::PaletteColors;
use egui::{Color32, Rect, Ui, Vec2};

/// How a floating card is drawn. The defaults are the house values:
/// `SPACE_MD` of padding, `SPACE_SM` between rows, `RADIUS_MD` corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardStyle {
    /// Air between the card's edge and its content, all four sides.
    pub pad: f32,
    /// Vertical air between the content's own rows.
    pub gap: f32,
    pub radius: f32,
    /// A 1-pt stroke in the palette's border colour. The single biggest
    /// contributor to "this is a panel, not more page".
    pub border: bool,
    /// A soft drop shadow under the card.
    pub shadow: bool,
}

impl Default for CardStyle {
    fn default() -> Self {
        CardStyle {
            pad: crate::tokens::SPACE_MD as f32,
            gap: crate::tokens::SPACE_SM as f32,
            radius: crate::tokens::RADIUS_MD as f32,
            border: true,
            shadow: true,
        }
    }
}

impl CardStyle {
    /// Tighter: for a caption or a tooltip, where the card is a label's
    /// worth of content and full padding would dwarf it.
    pub fn compact() -> Self {
        CardStyle {
            pad: crate::tokens::SPACE_XS as f32,
            gap: 2.0,
            radius: crate::tokens::RADIUS_SM as f32,
            border: true,
            shadow: false,
        }
    }

    /// The content rect inside a card occupying `outer`.
    pub fn inner(&self, outer: Rect) -> Rect {
        // shrink2 rather than shrink so a future asymmetric pad is a
        // one-line change here and not at every call site.
        let r = outer.shrink2(Vec2::splat(self.pad));
        // A card smaller than its own padding would invert; give back an
        // empty rect at the centre instead of a negative one.
        if r.width() <= 0.0 || r.height() <= 0.0 {
            Rect::from_center_size(outer.center(), Vec2::ZERO)
        } else {
            r
        }
    }

    /// The outer size a card needs to hold `content`.
    pub fn outer_size(&self, content: Vec2) -> Vec2 {
        content + Vec2::splat(self.pad * 2.0)
    }
}

/// Dim everything behind a floating panel. `strength01` is how far toward
/// opaque black: 0.45 is the house default — enough that the page reads as
/// "behind", not so much that it reads as gone.
///
/// Painted at the current layer, so call it before the card.
pub fn scrim(ui: &Ui, area: Rect, strength01: f32) {
    let a = (strength01.clamp(0.0, 1.0) * 255.0) as u8;
    ui.painter().rect_filled(area, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, a));
}

/// Paint a card's background at `rect` and return a `Ui` for its content,
/// inset by the style's padding and with the style's row spacing applied.
///
/// The content is clipped to the inner rect: a row too wide is cut at the
/// padding rather than drawn over the card's own border.
pub fn card(ui: &mut Ui, rect: Rect, c: &PaletteColors, style: CardStyle) -> Ui {
    if style.shadow {
        // Two offset fills rather than a blur: cheap, and enough to lift
        // the card off the page at these radii.
        let s1 = rect.translate(Vec2::new(0.0, 2.0)).expand(1.0);
        let s2 = rect.translate(Vec2::new(0.0, 5.0)).expand(3.0);
        ui.painter().rect_filled(s2, style.radius + 3.0, Color32::from_black_alpha(26));
        ui.painter().rect_filled(s1, style.radius + 1.0, Color32::from_black_alpha(38));
    }
    ui.painter().rect_filled(rect, style.radius, c.bg_raised);
    if style.border {
        ui.painter().rect_stroke(rect, style.radius, egui::Stroke::new(1.0_f32, c.border),
                                 egui::StrokeKind::Inside);
    }
    let inner = style.inner(rect);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner);
    child.spacing_mut().item_spacing.y = style.gap;
    child
}

/// A hairline between two stacked regions of a card, with the style's gap
/// either side. `ui.separator()` inside a card draws edge to edge of the
/// content rect, which is right; this exists so the air around it is the
/// card's gap and not egui's default.
pub fn divider(ui: &mut Ui, c: &PaletteColors, gap: f32) {
    ui.add_space(gap);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, c.border);
    ui.add_space(gap);
}

/// Move `rect` so it lies wholly inside `screen`, keeping `margin` from
/// every edge. Translation only — never resized — unless it cannot fit,
/// in which case it is pinned to the top-left of the available area so
/// the START of the content is what survives.
pub fn fit_on_screen(rect: Rect, screen: Rect, margin: f32) -> Rect {
    let avail = screen.shrink(margin);
    let mut r = rect;
    if r.width() > avail.width() || r.height() > avail.height() {
        return Rect::from_min_size(avail.min, r.size());
    }
    if r.max.x > avail.max.x { r = r.translate(Vec2::new(avail.max.x - r.max.x, 0.0)); }
    if r.min.x < avail.min.x { r = r.translate(Vec2::new(avail.min.x - r.min.x, 0.0)); }
    if r.max.y > avail.max.y { r = r.translate(Vec2::new(0.0, avail.max.y - r.max.y)); }
    if r.min.y < avail.min.y { r = r.translate(Vec2::new(0.0, avail.min.y - r.min.y)); }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    #[test]
    fn padding_is_symmetric_and_survives_an_absurdly_small_card() {
        let st = CardStyle::default();
        let inner = st.inner(r(10.0, 10.0, 200.0, 100.0));
        assert_eq!(inner.min.x - 10.0, st.pad);
        assert_eq!(210.0 - inner.max.x, st.pad);
        assert_eq!(inner.min.y - 10.0, st.pad);
        assert_eq!(110.0 - inner.max.y, st.pad);
        // Narrower than twice the padding: an empty rect, never an inverted one.
        let tiny = st.inner(r(0.0, 0.0, 4.0, 4.0));
        assert!(tiny.width() >= 0.0 && tiny.height() >= 0.0, "{tiny:?}");
        assert!(!tiny.is_negative());
    }

    #[test]
    fn outer_size_round_trips_the_content_it_was_asked_to_hold() {
        let st = CardStyle::default();
        let content = egui::vec2(240.0, 90.0);
        let outer = Rect::from_min_size(egui::pos2(5.0, 5.0), st.outer_size(content));
        let back = st.inner(outer).size();
        assert!((back.x - content.x).abs() < 1e-4 && (back.y - content.y).abs() < 1e-4, "{back:?}");
    }

    #[test]
    fn a_card_near_an_edge_is_pushed_fully_on_screen() {
        let screen = r(0.0, 0.0, 400.0, 800.0);
        // Off the bottom AND the right.
        let off = r(330.0, 770.0, 120.0, 60.0);
        let fixed = fit_on_screen(off, screen, 8.0);
        assert!(screen.shrink(8.0).contains_rect(fixed), "{fixed:?}");
        assert_eq!(fixed.size(), off.size(), "moved, never resized");
        // Off the top-left too.
        let off2 = r(-40.0, -30.0, 100.0, 50.0);
        let fixed2 = fit_on_screen(off2, screen, 8.0);
        assert_eq!(fixed2.min, egui::pos2(8.0, 8.0));
        // Already inside: untouched.
        let inside = r(100.0, 100.0, 50.0, 50.0);
        assert_eq!(fit_on_screen(inside, screen, 8.0), inside);
    }

    #[test]
    fn something_too_big_to_fit_keeps_its_start_visible() {
        let screen = r(0.0, 0.0, 200.0, 200.0);
        let huge = r(50.0, 50.0, 900.0, 900.0);
        let fixed = fit_on_screen(huge, screen, 10.0);
        assert_eq!(fixed.min, egui::pos2(10.0, 10.0), "pinned to the top-left, start visible");
    }

    #[test]
    fn the_compact_style_is_tighter_but_still_bordered() {
        let c = CardStyle::compact();
        let d = CardStyle::default();
        assert!(c.pad < d.pad && c.gap < d.gap && c.radius < d.radius);
        assert!(c.border, "a tooltip still needs its edge to read");
        assert!(!c.shadow);
    }

    #[test]
    fn a_card_gives_its_child_the_styles_gap_and_clips_to_the_padding() {
        let ctx = egui::Context::default();
        let mut seen = None;
        let st = CardStyle::default();
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(r(0.0, 0.0, 400.0, 400.0)), ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let area = r(20.0, 20.0, 300.0, 200.0);
                let colors = PaletteColors::from_palette(snapclone_theme::palette_by_id("midnight"));
                let child = card(ui, area, &colors, st);
                seen = Some((child.clip_rect(), child.spacing().item_spacing.y, child.max_rect()));
            });
        });
        let (clip, gap, max) = seen.unwrap();
        assert_eq!(gap, st.gap);
        assert_eq!(max, st.inner(r(20.0, 20.0, 300.0, 200.0)));
        assert!(clip.width() <= 300.0 - st.pad * 2.0 + 0.01, "clipped to the padding: {clip:?}");
    }
}
