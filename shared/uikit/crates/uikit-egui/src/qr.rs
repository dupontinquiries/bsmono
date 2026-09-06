//! qr — a theme-aware, custom-drawn QR-shaped identicon, and the expanded overlay for it.
//! `QrCodeRenderer.cpp` ported from snapclone into the kit, so any app that shows an id
//! can show it the same way.
//!
//! ## This is a QR-shaped identicon, and calling it that matters
//!
//! The C++ draws finder patterns, timing rows and a data field derived from the id — a
//! **Version 1 layout** at 21×21 — but there is no Reed-Solomon encoder, no mask
//! evaluation and no format information. A real QR reader will not decode it. It is a
//! recognisable, deterministic visual for an id, and this keeps that honestly:
//! [`Layout::modules`] is the drawing, and the caption in the app says what it is not.
//!
//! ## Three things that are load-bearing (from the C++)
//!
//! * **Cell size is an integer, floored, with a floor of 2.** A fractional module means the
//!   renderer resamples and the edges blur — the C++ names this as *"visible as the soft
//!   edges on the FP5 QR"*. Two is the minimum where a module survives at all.
//! * **Rendered at device pixel ratio.** A phone is 2–3x; drawing at logical size and
//!   letting the toolkit upscale is exactly the blur above. Here the cell is floored in
//!   physical pixels from `ctx.pixels_per_point()` and converted back to points.
//! * **A WCAG 4.5:1 contrast guard.** Unusual for a QR and completely right: a camera has
//!   to separate the modules, and a themed palette can easily put foreground and
//!   background close together. When the theme fails the check it falls back to pure
//!   black or white rather than to something nearer.
//!
//! ## The drawing is the app's, not a printer's
//!
//! The C++ fills squares. This draws the three finder eyes as rounded squares with a
//! rounded core, and the data modules as dots — horizontal runs of them joined into
//! pills — on a glass card with the hairline the rest of the chrome has. Module centres
//! are where a scanner samples, and every centre is exactly where the square version put
//! it; only the corners changed. Theme colours: the card colour behind, the text colour
//! for the modules (the accent on a pale card, as the C++ does), then the guard.
//!
//! ## The expanded overlay, and the tap that opens it
//!
//! Tapping the small code opens it full-screen: a 95% scrim of the base colour, the code
//! at 80% of the width (capped at 70% of the height) on its card, the id in mono at 24,
//! a hint, a Close button at 48. Two guards from the C++ ("V32.30", "flash open and
//! shut"): releases are ignored for 300 ms after opening, so the opening tap's own
//! release cannot close it; and only a tap on the SCRIM closes — not on the code, which
//! is where a finger naturally rests. Escape closes. It fades in over 220 ms, OutCubic,
//! one fade — the nested second fade the C++ removed for flickering is not reproduced.

use crate::PaletteColors;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Ui};

/// Version 1 QR: 21×21 modules.
pub const MODULES: usize = 21;
/// Divisor used to pick a cell size, leaving room for a quiet border.
pub const CELL_DIVISOR: u32 = 22;
/// Smallest cell that still renders as a module.
pub const MIN_CELL: u32 = 2;
/// Contrast floor, WCAG AA for normal text.
pub const MIN_CONTRAST: f32 = 4.5;

/// Pixel size of one module, and the resulting image size.
///
/// Integer and floored, because a fractional module resamples and blurs the
/// edges — the FP5 symptom the C++ calls out.
pub fn cell_size(px: u32, dpr: f32) -> u32 {
    let scaled = (px as f32 * dpr.max(0.1)) as u32;
    (scaled / CELL_DIVISOR).max(MIN_CELL)
}

pub fn image_size(px: u32, dpr: f32) -> u32 { cell_size(px, dpr) * MODULES as u32 }

/// Relative luminance, sRGB.
pub fn luminance(c: [u8; 3]) -> f32 {
    let ch = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
}

/// WCAG contrast ratio between two colours.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (l1, l2) = (luminance(a), luminance(b));
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

/// Pick a foreground that a camera can separate from `bg`.
///
/// Falls back to pure black or white rather than to something merely better: a
/// QR that *nearly* scans is worse than one that obviously does, because the
/// user retries instead of moving the phone.
pub fn readable_fg(bg: [u8; 3], preferred: [u8; 3]) -> [u8; 3] {
    if contrast(bg, preferred) >= MIN_CONTRAST { return preferred; }
    if luminance(bg) > 0.5 { [0, 0, 0] } else { [255, 255, 255] }
}

/// The module grid, row-major, `true` where a module is dark.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub modules: Vec<bool>,
}

impl Layout {
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= MODULES || y >= MODULES { return false; }
        self.modules[y * MODULES + x]
    }

    /// Is this coordinate inside a finder pattern or its separator?
    ///
    /// Data must never be written here: the finders are what a scanner locates
    /// first, and a stray dark module beside one breaks detection outright
    /// rather than corrupting a byte.
    pub fn is_reserved(x: usize, y: usize) -> bool {
        let corner = |ox: usize, oy: usize| x >= ox && x < ox + 8 && y >= oy && y < oy + 8;
        corner(0, 0)
            || (x >= MODULES - 8 && y < 8)
            || (x < 8 && y >= MODULES - 8)
            // Timing rows.
            || x == 6 || y == 6
    }
}

/// Build the layout for an id.
///
/// Deterministic: one id always renders the same square, which is the property
/// that makes it usable as an identicon at all.
pub fn layout_for(id: &str) -> Layout {
    let mut m = vec![false; MODULES * MODULES];
    let mut set = |x: usize, y: usize, on: bool| {
        if x < MODULES && y < MODULES { m[y * MODULES + x] = on; }
    };

    // Three finder patterns: 7×7 dark, 5×5 light, 3×3 dark.
    for (ox, oy) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
        for dy in 0..7 {
            for dx in 0..7 {
                let ring = dx == 0 || dy == 0 || dx == 6 || dy == 6;
                let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                set(ox + dx, oy + dy, ring || core);
            }
        }
    }

    // Timing patterns: alternating modules along row and column 6.
    for i in 8..MODULES - 8 {
        let on = i % 2 == 0;
        set(i, 6, on);
        set(6, i, on);
    }

    // Data field, derived from the id. Not an encoding — see the module docs.
    let bytes = id.as_bytes();
    if !bytes.is_empty() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        for y in 0..MODULES {
            for x in 0..MODULES {
                if Layout::is_reserved(x, y) { continue; }
                h ^= h << 13; h ^= h >> 7; h ^= h << 17;
                set(x, y, (h >> 33) & 1 == 1);
            }
        }
    }
    Layout { modules: m }
}


// ── theme ──────────────────────────────────────────────────────────────────────────────

fn rgb(c: Color32) -> [u8; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r, g, b]
}

/// The code's colours from the palette: the card behind, the text colour for the modules
/// (the accent on a pale card, as the C++'s `lightness() > 200` rule), then the guard.
pub fn theme_colors(c: &PaletteColors) -> (Color32, Color32) {
    let bg = rgb(c.bg_card);
    let pale = luminance(bg) > 0.55;
    let preferred = rgb(if pale { c.accent } else { c.text });
    let fg = readable_fg(bg, preferred);
    (Color32::from_rgb(bg[0], bg[1], bg[2]), Color32::from_rgb(fg[0], fg[1], fg[2]))
}

// ── drawing ────────────────────────────────────────────────────────────────────────────

/// Card padding around the modules, in points (the C++ `padding: 8px`).
pub const CARD_PAD: f32 = 8.0;
/// Card corner radius (the C++ `border-radius: 12px`).
pub const CARD_RADIUS: u8 = 12;

/// Draw the code for `layout` at `side` points on a glass card. Returns the response of
/// the whole card (`Sense::click`, so the caller can open the expanded view on a tap).
pub fn qr_ui(ui: &mut Ui, layout: &Layout, side: f32, c: &PaletteColors) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
    paint(ui.painter(), rect, layout, ui.ctx().pixels_per_point(), c);
    resp
}

/// The painter half of [`qr_ui`], for callers that place the rect themselves.
pub fn paint(painter: &egui::Painter, rect: Rect, layout: &Layout, pixels_per_point: f32, c: &PaletteColors) {
    let (bg, fg) = theme_colors(c);
    // the card: the rest of the chrome's glass, with the hairline along the top
    painter.rect_filled(rect, CornerRadius::same(CARD_RADIUS), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(CARD_RADIUS),
        egui::Stroke::new(1.0, Color32::from_white_alpha(crate::glass::GlassAlphas::HAIRLINE)),
        egui::StrokeKind::Inside,
    );
    // an integer cell in PHYSICAL pixels, back to points: the modules land on pixels
    let inner = rect.shrink(CARD_PAD);
    let ppp = pixels_per_point.max(0.1);
    let cell_px = cell_size(inner.width().min(inner.height()) as u32, ppp);
    let cell = cell_px as f32 / ppp;
    let used = cell * MODULES as f32;
    // centred on the leftover, so flooring does not push the pattern into a corner
    let origin = inner.min + egui::vec2((inner.width() - used) * 0.5, (inner.height() - used) * 0.5);
    let at = |x: usize, y: usize| origin + egui::vec2(x as f32 * cell, y as f32 * cell);

    // finder eyes: rounded outer ring, the light ring, a rounded core
    for (ox, oy) in [(0usize, 0usize), (MODULES - 7, 0), (0, MODULES - 7)] {
        let outer = Rect::from_min_size(at(ox, oy), egui::vec2(7.0 * cell, 7.0 * cell));
        painter.rect_filled(outer, CornerRadius::same((cell * 1.6) as u8), fg);
        let light = Rect::from_min_size(at(ox + 1, oy + 1), egui::vec2(5.0 * cell, 5.0 * cell));
        painter.rect_filled(light, CornerRadius::same((cell * 1.0) as u8), bg);
        let core = Rect::from_min_size(at(ox + 2, oy + 2), egui::vec2(3.0 * cell, 3.0 * cell));
        painter.rect_filled(core, CornerRadius::same((cell * 0.8) as u8), fg);
    }
    // data + timing modules: dots, with horizontal runs joined into pills
    let r = cell * 0.44;
    for y in 0..MODULES {
        let mut x = 0;
        while x < MODULES {
            if in_finder(x, y) || !layout.get(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x + 1 < MODULES && layout.get(x + 1, y) && !in_finder(x + 1, y) {
                x += 1;
            }
            let a = at(start, y) + egui::vec2(cell * 0.5, cell * 0.5);
            let b = at(x, y) + egui::vec2(cell * 0.5, cell * 0.5);
            if start == x {
                painter.circle_filled(a, r, fg);
            } else {
                let pill = Rect::from_two_pos(a - egui::vec2(r, r), b + egui::vec2(r, r));
                painter.rect_filled(pill, CornerRadius::same(r as u8), fg);
            }
            x += 1;
        }
    }
}

/// Inside one of the three 7×7 finder eyes (drawn as shapes, not as modules).
fn in_finder(x: usize, y: usize) -> bool {
    let eye = |ox: usize, oy: usize| x >= ox && x < ox + 7 && y >= oy && y < oy + 7;
    eye(0, 0) || eye(MODULES - 7, 0) || eye(0, MODULES - 7)
}

// ── the expanded overlay ───────────────────────────────────────────────────────────────

/// Releases are ignored this long after opening — the opening tap's own release must not
/// close it (the C++ `m_openGuardActive`).
pub const OPEN_GUARD_S: f64 = 0.30;
/// The fade-in (the C++ 220 ms, OutCubic).
pub const FADE_S: f64 = 0.22;
/// The Close button's height (the C++ `setMinimumHeight(48)`).
pub const CLOSE_HEIGHT: f32 = 48.0;

/// The expanded view's state: when it opened. Pure; the timing rules are tested.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Expanded {
    pub opened_at: f64,
}

impl Expanded {
    pub fn open(now: f64) -> Expanded {
        Expanded { opened_at: now }
    }
    /// 0..1, OutCubic over the fade.
    pub fn alpha(&self, now: f64) -> f32 {
        let t = ((now - self.opened_at) / FADE_S).clamp(0.0, 1.0) as f32;
        1.0 - (1.0 - t).powi(3)
    }
    /// May a release close it yet?
    pub fn can_close(&self, now: f64) -> bool {
        now - self.opened_at >= OPEN_GUARD_S
    }
    /// The code's side for a window: 80% of the usable width, capped at 70% of the usable
    /// height (the C++ `qMin(width - 2m, (height - 2m) * 7 / 10)`), margin 16 under 420 wide.
    pub fn qr_side(window: Rect) -> (f32, f32) {
        let margin = if window.width() < 420.0 { 16.0 } else { 32.0 };
        let side = (window.width() - margin * 2.0).min((window.height() - margin * 2.0) * 0.7);
        (side.max(64.0), margin)
    }
}

/// Draw the expanded overlay. `state` is `Some` while open; it is set to `None` — and
/// `true` returned — on the frame it closes (a tap on the scrim after the guard, Escape,
/// or Close).
pub fn expanded_ui(ctx: &egui::Context, state: &mut Option<Expanded>, id: &str, layout: &Layout, c: &PaletteColors) -> bool {
    let Some(ex) = *state else { return false };
    let now = ctx.input(|i| i.time);
    let a = ex.alpha(now);
    if a < 1.0 {
        ctx.request_repaint();
    }
    let window = ctx.screen_rect();
    let (side, margin) = Expanded::qr_side(window);
    let mut close = false;
    // ABOVE EVERYTHING, including the dialog that opened it: a Foreground
    // Area painted before the dialog's own Foreground layer lost to it, so
    // the scrim came up and the code stayed hidden behind the small one.
    let area = egui::Area::new(egui::Id::new("uikit-qr-expanded"))
        .order(egui::Order::Tooltip)
        .fixed_pos(window.min)
        .interactable(true);
    area.show(ctx, |ui| {
        ui.set_min_size(window.size());
        let scrim = ui.allocate_rect(window, Sense::click());
        ui.painter().rect_filled(window, 0.0, Color32::from_rgba_unmultiplied(c.bg_base.r(), c.bg_base.g(), c.bg_base.b(), (242.0 * a) as u8));
        // the column: code, id, hint, close — centred, margin above and below
        let code_rect = Rect::from_center_size(
            Pos2::new(window.center().x, window.top() + margin + side * 0.5 + (window.height() - margin * 2.0 - side - 140.0).max(0.0) * 0.5),
            egui::vec2(side, side),
        );
        let mut card = ui.new_child(egui::UiBuilder::new().max_rect(code_rect).sense(Sense::click()));
        let _code = qr_ui(&mut card, layout, side, c);
        // fade: paint the base over the top at 1-a, so the whole thing comes up together
        if a < 1.0 {
            ui.painter().rect_filled(window, 0.0, Color32::from_rgba_unmultiplied(c.bg_base.r(), c.bg_base.g(), c.bg_base.b(), (255.0 * (1.0 - a)) as u8));
        }
        let mut y = code_rect.bottom() + 16.0;
        let id_font = egui::FontId::monospace(24.0);
        ui.painter().text(Pos2::new(window.center().x, y), egui::Align2::CENTER_TOP, id, id_font, c.text);
        y += 40.0;
        ui.painter().text(Pos2::new(window.center().x, y), egui::Align2::CENTER_TOP, "Tap outside the code or press Close", egui::FontId::proportional(13.0), c.text_weak);
        y += 30.0;
        let btn_rect = Rect::from_center_size(Pos2::new(window.center().x, (y + CLOSE_HEIGHT * 0.5).min(window.bottom() - margin - CLOSE_HEIGHT * 0.5)), egui::vec2(180.0, CLOSE_HEIGHT));
        let mut brow = ui.new_child(egui::UiBuilder::new().max_rect(btn_rect));
        // the C++ button: 48 high, `padding: 12px 24px` — a wide glass key, not a label-sized one
        let close_resp = brow.add(
            egui::Button::new("\u{2715}  Close")
                .fill(crate::glass::key_fill(c.bg_card))
                .stroke(crate::glass::key_border(c.border))
                .corner_radius(CornerRadius::same(CARD_RADIUS))
                .min_size(btn_rect.size()),
        );
        if close_resp.clicked() {
            close = true;
        }
        // a tap on the scrim, after the guard, and not on the code or the button
        if scrim.clicked() && ex.can_close(now) {
            if let Some(p) = scrim.interact_pointer_pos() {
                if !code_rect.contains(p) && !btn_rect.contains(p) {
                    close = true;
                }
            }
        }
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if close {
        *state = None;
    }
    close
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_ignores_the_opening_taps_release_and_fades_out_cubic() {
        let ex = Expanded::open(10.0);
        assert!(!ex.can_close(10.0) && !ex.can_close(10.29), "inside the guard");
        assert!(ex.can_close(10.30));
        assert_eq!(ex.alpha(10.0), 0.0);
        assert!((ex.alpha(10.0 + FADE_S) - 1.0).abs() < 1e-6);
        let mid = ex.alpha(10.0 + FADE_S * 0.5);
        assert!(mid > 0.8, "OutCubic is front-loaded: {mid}");
    }

    #[test]
    fn the_expanded_code_is_eighty_percent_wide_capped_by_the_height() {
        let (side, m) = Expanded::qr_side(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0)));
        assert_eq!(m, 16.0, "a phone-width window gets the tighter margin");
        assert_eq!(side, 400.0 - 32.0, "width-bound");
        let (side, m) = Expanded::qr_side(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 500.0)));
        assert_eq!(m, 32.0);
        assert!((side - (500.0 - 64.0) * 0.7).abs() < 1e-3, "height-bound");
    }

    #[test]
    fn the_theme_picks_the_text_on_a_dark_card_and_the_accent_on_a_pale_one_then_guards() {
        let c = crate::default_palette();
        let (bg, fg) = theme_colors(&c);
        assert!(contrast(rgb(bg), rgb(fg)) >= MIN_CONTRAST, "a themed code still scans");
        let mut pale = c;
        pale.bg_card = Color32::from_rgb(245, 240, 230);
        pale.accent = Color32::from_rgb(40, 60, 120);
        let (_, fg) = theme_colors(&pale);
        assert_eq!(rgb(fg), [40, 60, 120], "the accent on a pale card, as the C++ does");
        pale.accent = Color32::from_rgb(230, 230, 220);
        let (_, fg) = theme_colors(&pale);
        assert_eq!(rgb(fg), [0, 0, 0], "an accent that would not scan falls back to black");
    }

    #[test]
    fn the_small_code_and_the_overlay_draw_headless() {
        let c = crate::default_palette();
        let layout = layout_for("sc-abcdef01");
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0)));
        let mut ex = Some(Expanded::open(0.0));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = qr_ui(ui, &layout, 168.0, &c);
                assert!(!r.clicked());
            });
            let closed = expanded_ui(ctx, &mut ex, "sc-abcdef01", &layout, &c);
            assert!(!closed, "nothing closes it on a frame with no input");
        });
        assert!(ex.is_some());
        assert!(out.shapes.len() > 40, "{} shapes — the modules were not drawn", out.shapes.len());
    }

    #[test]
    fn a_cell_is_a_whole_number_of_pixels() {
        // A fractional module resamples and blurs the edges — "visible as the
        // soft edges on the FP5 QR".
        for px in [64u32, 100, 220, 333] {
            for dpr in [1.0f32, 1.5, 2.0, 2.625, 3.0] {
                let c = cell_size(px, dpr);
                assert!(c >= MIN_CELL);
                assert_eq!(image_size(px, dpr) % c, 0, "image is a whole number of cells");
            }
        }
    }

    #[test]
    fn a_higher_pixel_ratio_renders_more_pixels() {
        // Drawing at logical size and letting the toolkit upscale is exactly
        // the blur the C++ names.
        assert!(cell_size(200, 3.0) > cell_size(200, 1.0));
    }

    #[test]
    fn a_tiny_request_still_produces_visible_modules() {
        // Below two pixels a module stops rendering as anything.
        assert_eq!(cell_size(10, 1.0), MIN_CELL);
        assert_eq!(cell_size(1, 1.0), MIN_CELL);
        assert_eq!(cell_size(0, 1.0), MIN_CELL, "zero does not divide to zero");
    }

    #[test]
    fn a_zero_or_negative_pixel_ratio_does_not_collapse_the_render() {
        // A platform reporting no DPR yet is reachable during startup.
        assert!(cell_size(200, 0.0) >= MIN_CELL);
        assert!(cell_size(200, -1.0) >= MIN_CELL);
    }

    #[test]
    fn a_low_contrast_theme_falls_back_to_black_or_white() {
        // A camera has to separate the modules, and a QR that NEARLY scans is
        // worse than one that obviously does — the user retries instead of
        // moving the phone.
        let dark_bg = [30, 32, 38];
        let dim_fg = [60, 64, 70];
        assert!(contrast(dark_bg, dim_fg) < MIN_CONTRAST);
        assert_eq!(readable_fg(dark_bg, dim_fg), [255, 255, 255]);

        let light_bg = [245, 245, 245];
        assert_eq!(readable_fg(light_bg, [200, 200, 200]), [0, 0, 0]);
    }

    #[test]
    fn a_theme_that_already_passes_keeps_its_own_colour() {
        // Overriding a palette that works would make every QR look foreign to
        // the app around it.
        let bg = [255, 255, 255];
        let fg = [20, 24, 30];
        assert!(contrast(bg, fg) >= MIN_CONTRAST);
        assert_eq!(readable_fg(bg, fg), fg);
    }

    #[test]
    fn the_contrast_ratio_matches_the_wcag_definition() {
        // Black on white is 21:1 exactly; a formula that is off here is off
        // everywhere.
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([255, 255, 255], [255, 255, 255]) - 1.0).abs() < 0.01);
    }

    #[test]
    fn contrast_is_symmetric() {
        let a = [12, 90, 200];
        let b = [240, 240, 230];
        assert!((contrast(a, b) - contrast(b, a)).abs() < 1e-4);
    }

    #[test]
    fn all_three_finder_patterns_are_present_and_correctly_shaped() {
        // These are what a scanner locates first; getting the ring-and-core
        // shape wrong means nothing detects the code at all.
        let l = layout_for("sc-abc12345");
        for (ox, oy) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
            assert!(l.get(ox, oy), "outer ring at {ox},{oy}");
            assert!(!l.get(ox + 1, oy + 1), "light ring");
            assert!(l.get(ox + 3, oy + 3), "dark core");
        }
    }

    #[test]
    fn the_fourth_corner_is_not_a_finder() {
        // Four finders is not a QR; a scanner uses the missing one to work out
        // the orientation.
        let l = layout_for("sc-abc12345");
        let (ox, oy) = (MODULES - 7, MODULES - 7);
        let ring_complete = (0..7).all(|d| l.get(ox + d, oy));
        assert!(!ring_complete, "bottom-right must not look like a finder");
    }

    #[test]
    fn data_never_lands_in_a_reserved_area() {
        // A stray dark module beside a finder breaks detection outright rather
        // than corrupting a byte.
        for id in ["sc-00000000", "sc-ffffffff", "sc-abc12345"] {
            let l = layout_for(id);
            for (ox, oy) in [(0usize, 0usize), (MODULES - 7, 0), (0, MODULES - 7)] {
                // The separator ring just outside each finder stays light.
                if ox + 7 < MODULES { assert!(!l.get(ox + 7, oy), "{id}: separator"); }
                if oy + 7 < MODULES { assert!(!l.get(ox, oy + 7), "{id}: separator"); }
            }
        }
    }

    #[test]
    fn timing_patterns_alternate() {
        // The row a scanner uses to measure module size.
        let l = layout_for("sc-abc12345");
        for i in 8..MODULES - 8 {
            assert_eq!(l.get(i, 6), i % 2 == 0, "timing row at {i}");
            assert_eq!(l.get(6, i), i % 2 == 0, "timing column at {i}");
        }
    }

    #[test]
    fn one_id_always_renders_the_same_square() {
        // The property that makes it usable as an identicon at all.
        assert_eq!(layout_for("sc-abc12345"), layout_for("sc-abc12345"));
    }

    #[test]
    fn different_ids_render_differently() {
        // Two contacts sharing a square is worse than no square.
        let a = layout_for("sc-abc12345");
        let b = layout_for("sc-abc12346");
        assert_ne!(a, b, "one character apart must differ");
    }

    #[test]
    fn an_empty_id_still_produces_a_valid_frame() {
        // Reachable before an identity exists; the finders should render so the
        // placeholder looks like the real thing rather than an empty box.
        let l = layout_for("");
        assert!(l.get(0, 0));
        assert!(l.get(3, 3));
    }

    #[test]
    fn reading_outside_the_grid_returns_light_rather_than_panicking() {
        let l = layout_for("sc-abc12345");
        assert!(!l.get(MODULES, 0));
        assert!(!l.get(0, MODULES + 100));
    }
}
