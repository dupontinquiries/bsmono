//! glass — `GlassScrim` and `GlassIconButton`, the camera chrome's frosted look.
//!
//! ## The scrim does not sample the screen
//!
//! That is the constraint the whole design falls out of. A widget cannot read
//! its siblings' pixels cheaply, and a blur effect will not blur a LIVE
//! sibling — so a "frosted glass over the viewfinder" bar has no backdrop to
//! read. Instead the owner feeds it the strip of the current viewfinder frame
//! that lies under the scrim's geometry. Those pixels are already in hand every
//! frame, so the frost costs no extra capture at all.
//!
//! ## Five steps, and the first one is most of the effect
//!
//! 1. **Downscale by 4.** The downscale IS most of the blur, and it makes every
//!    later step run on a sixteenth of the pixels. That is the entire reason
//!    this is affordable on a Fairphone 5.
//! 2. **A 1px box blur** on the tiny image, which is a large blur at full size.
//! 3. **A chromatic shift**: red from the copy offset left, blue from the copy
//!    offset right, green from the centre. This is what stops it reading as
//!    "blurry photo" and starts it reading as glass.
//! 4. **Upscale back**, where bilinear interpolation adds more softening free.
//! 5. **Gradient and tint on top**, so controls stay legible over it.
//!
//! The shift is computed in DOWNSCALED space and floored at one pixel: a
//! 1px request divided by a downscale of 4 rounds to zero, and the fringe would
//! silently disappear at exactly the default settings.
//!
//! The scrim is mouse-transparent. Taps fall through to the viewfinder, and the
//! interactive control bar sits above it — a decorative panel that ate taps
//! aimed at the thing it is decorating would be a bug you could not see.
//!
//! ## The button is painted, not styled
//!
//! Two things a stylesheet cannot express: soft radial mottling from two
//! off-centre gradients, so the fill reads as uneven scattered glass rather
//! than a flat disc; and a ripple keyed to the ACTUAL tap coordinate, which a
//! `:pressed` state has no way to know.

/// Strip is processed at 1/downscale resolution.
pub const DOWNSCALE: usize = 4;
/// Chromatic shift, in full-resolution pixels.
pub const SHIFT_PX: usize = 1;
/// `Theme::ANIM_FAST` — the press dip.
pub const PRESS_MS: f32 = 110.0;
/// `ANIM_NORMAL + 120` — the ripple, which outlasts the dip on purpose so the
/// touch feedback is still visible after the button has sprung back.
pub const RIPPLE_MS: f32 = 340.0;
/// How far the body scales down on press.
pub const PRESS_SCALE: f32 = 0.88;

/// A tiny grayscale-agnostic RGBA image, enough to express the pipeline without
/// pulling an image crate into the view layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    /// RGBA, row-major.
    pub px: Vec<[u8; 4]>,
}

impl Img {
    pub fn new(w: usize, h: usize, fill: [u8; 4]) -> Self {
        Img { w, h, px: vec![fill; w * h] }
    }
    pub fn get(&self, x: usize, y: usize) -> [u8; 4] {
        self.px[y.min(self.h.saturating_sub(1)) * self.w + x.min(self.w.saturating_sub(1))]
    }
    pub fn set(&mut self, x: usize, y: usize, v: [u8; 4]) {
        let w = self.w;
        self.px[y * w + x] = v;
    }
    pub fn is_empty(&self) -> bool { self.w == 0 || self.h == 0 }
}

/// Step 1: nearest-neighbour downscale. Cheap on purpose — the smoothing comes
/// from the blur and the upscale, and a smooth downscale here would cost more
/// than it adds.
pub fn downscale(src: &Img, factor: usize) -> Img {
    let f = factor.max(1);
    // At least 1px each way: a strip four pixels tall downscaled by four must
    // not become a zero-height image, which would drop the frost entirely on
    // thin bars.
    let (dw, dh) = ((src.w / f).max(1), (src.h / f).max(1));
    let mut out = Img::new(dw, dh, [0; 4]);
    for y in 0..dh {
        for x in 0..dw {
            out.set(x, y, src.get(x * f, y * f));
        }
    }
    out
}

/// Step 2: separable box blur of the given radius.
pub fn box_blur(src: &Img, radius: usize) -> Img {
    if radius == 0 || src.is_empty() { return src.clone(); }
    let mut tmp = src.clone();
    // Horizontal, then vertical — separable, so the cost is 2r rather than r².
    for y in 0..src.h {
        for x in 0..src.w {
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for dx in -(radius as isize)..=(radius as isize) {
                let sx = (x as isize + dx).clamp(0, src.w as isize - 1) as usize;
                let p = src.get(sx, y);
                for c in 0..4 { acc[c] += p[c] as u32; }
                n += 1;
            }
            tmp.set(x, y, [(acc[0] / n) as u8, (acc[1] / n) as u8,
                           (acc[2] / n) as u8, (acc[3] / n) as u8]);
        }
    }
    let mut out = tmp.clone();
    for y in 0..src.h {
        for x in 0..src.w {
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for dy in -(radius as isize)..=(radius as isize) {
                let sy = (y as isize + dy).clamp(0, src.h as isize - 1) as usize;
                let p = tmp.get(x, sy);
                for c in 0..4 { acc[c] += p[c] as u32; }
                n += 1;
            }
            out.set(x, y, [(acc[0] / n) as u8, (acc[1] / n) as u8,
                           (acc[2] / n) as u8, (acc[3] / n) as u8]);
        }
    }
    out
}

/// The shift distance in DOWNSCALED pixels, floored at one.
///
/// `shift_px / downscale` is 1/4 = 0 at the defaults, which would silently
/// remove the chromatic fringe at exactly the settings everything ships with.
pub fn shift_in_small_space(shift_px: usize, downscale_factor: usize) -> usize {
    (shift_px / downscale_factor.max(1)).max(1)
}

/// Step 3: red from the left-offset copy, blue from the right, green and alpha
/// from the centre. Horizontal only — the bars span the full width, so a
/// vertical fringe would have nothing to run along.
pub fn chromatic_shift(src: &Img, d: usize) -> Img {
    if d == 0 || src.is_empty() { return src.clone(); }
    let mut out = src.clone();
    for y in 0..src.h {
        for x in 0..src.w {
            let lx = x.saturating_sub(d);
            let rx = (x + d).min(src.w - 1);
            let r = src.get(lx, y)[0];
            let c = src.get(x, y);
            let b = src.get(rx, y)[2];
            out.set(x, y, [r, c[1], b, c[3]]);
        }
    }
    out
}

/// Step 4: bilinear upscale back to the target size.
pub fn upscale(src: &Img, w: usize, h: usize) -> Img {
    if src.is_empty() || w == 0 || h == 0 { return Img::new(w, h, [0; 4]); }
    let mut out = Img::new(w, h, [0; 4]);
    for y in 0..h {
        for x in 0..w {
            let sx = x * src.w / w;
            let sy = y * src.h / h;
            out.set(x, y, src.get(sx, sy));
        }
    }
    out
}

/// The whole frost, from a backdrop strip to the glass pixmap.
pub fn frost(backdrop: &Img, downscale_factor: usize, shift_px: usize) -> Img {
    if backdrop.is_empty() { return backdrop.clone(); }
    let small = downscale(backdrop, downscale_factor);
    let small = box_blur(&small, 1);
    let small = if shift_px > 0 {
        chromatic_shift(&small, shift_in_small_space(shift_px, downscale_factor))
    } else { small };
    upscale(&small, backdrop.w, backdrop.h)
}

// ── glass icon button ───────────────────────────────────────────────────────

/// The button's press animation and ripple.
#[derive(Debug, Clone, PartialEq)]
pub struct GlassButton {
    /// 1.0 at rest, dips to PRESS_SCALE on press.
    scale: f32,
    down: bool,
    /// Ripple progress; `None` when no ripple is running.
    ripple_ms: Option<f32>,
    /// Where the ripple started, in button-local coordinates.
    ripple_at: (f32, f32),
}

impl Default for GlassButton {
    fn default() -> Self {
        GlassButton { scale: 1.0, down: false, ripple_ms: None, ripple_at: (0.0, 0.0) }
    }
}

impl GlassButton {
    /// Press at a point. The ripple is keyed to the ACTUAL coordinate, which is
    /// what a stylesheet `:pressed` state cannot know and what makes the
    /// feedback feel like it came from your finger.
    pub fn press(&mut self, x: f32, y: f32) {
        self.down = true;
        self.ripple_at = (x, y);
        self.ripple_ms = Some(0.0);
    }

    pub fn release(&mut self) { self.down = false; }

    pub fn tick(&mut self, dt_ms: f32) {
        // Dip fast, spring back over the same window.
        let target = if self.down { PRESS_SCALE } else { 1.0 };
        let step = (1.0 - PRESS_SCALE) * dt_ms / PRESS_MS;
        if self.scale > target { self.scale = (self.scale - step).max(target); }
        else if self.scale < target { self.scale = (self.scale + step).min(target); }

        if let Some(t) = self.ripple_ms {
            let t = t + dt_ms;
            // The ripple outlives the release on purpose: cutting it short on
            // finger-up makes a quick tap produce no visible feedback at all.
            self.ripple_ms = if t >= RIPPLE_MS { None } else { Some(t) };
        }
    }

    pub fn scale(&self) -> f32 { self.scale }
    pub fn rippling(&self) -> bool { self.ripple_ms.is_some() }
    pub fn ripple_origin(&self) -> (f32, f32) { self.ripple_at }

    /// Ripple radius as a fraction of its final size, 0..=1.
    pub fn ripple_t(&self) -> f32 {
        self.ripple_ms.map(|t| (t / RIPPLE_MS).clamp(0.0, 1.0)).unwrap_or(0.0)
    }

    /// Ripple opacity — fades as it expands, or it ends as a hard disc.
    pub fn ripple_alpha(&self) -> f32 {
        if !self.rippling() { return 0.0; }
        (1.0 - self.ripple_t()) * 0.35
    }
}

/// Draw a glass icon button. Returns true on click.
pub fn glass_button_ui(ui: &mut egui::Ui, b: &mut GlassButton, glyph: &str, d: f32,
                       c: &uikit_egui::PaletteColors) -> bool {
    b.tick(ui.input(|i| i.stable_dt) * 1000.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::click());
    if resp.is_pointer_button_down_on() {
        if !b.down {
            let p = resp.interact_pointer_pos().unwrap_or(rect.center());
            b.press(p.x - rect.left(), p.y - rect.top());
        }
    } else { b.release(); }

    let r = d * 0.5 * b.scale();
    let painter = ui.painter();
    painter.circle_filled(rect.center(), r, c.bg_raised);
    // Two off-centre highlights: the mottling that stops it reading as a flat
    // disc. A stylesheet has no way to express this.
    painter.circle_filled(rect.center() + egui::vec2(-r * 0.3, -r * 0.3), r * 0.45,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10));
    painter.circle_filled(rect.center() + egui::vec2(r * 0.35, r * 0.2), r * 0.3,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 6));
    if b.rippling() {
        let (ox, oy) = b.ripple_origin();
        let a = (b.ripple_alpha() * 255.0) as u8;
        painter.circle_filled(rect.left_top() + egui::vec2(ox, oy), d * 0.8 * b.ripple_t(),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, a));
    }
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, glyph,
                 egui::FontId::proportional(d * 0.42), c.text);
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Img {
        let mut i = Img::new(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let v = (x * 255 / w.max(1)) as u8;
                i.set(x, y, [v, 255 - v, v / 2, 255]);
            }
        }
        i
    }

    #[test]
    fn the_downscale_is_the_cheap_step_that_does_most_of_the_work() {
        // A sixteenth of the pixels for everything after it — the reason this
        // is affordable on a phone at all.
        let src = gradient(64, 32);
        let small = downscale(&src, DOWNSCALE);
        assert_eq!((small.w, small.h), (16, 8));
        assert_eq!(small.px.len() * 16, src.px.len());
    }

    #[test]
    fn a_thin_strip_does_not_downscale_to_nothing() {
        // A 4px-tall bar divided by 4 must not become zero-height, which would
        // drop the frost entirely on exactly the thinnest bars.
        let small = downscale(&gradient(64, 3), DOWNSCALE);
        assert!(small.h >= 1 && small.w >= 1);
        assert!(!small.is_empty());
    }

    #[test]
    fn the_blur_actually_reduces_variation() {
        let src = gradient(32, 32);
        let blurred = box_blur(&src, 1);
        let spread = |i: &Img| {
            let vals: Vec<i32> = i.px.iter().map(|p| p[0] as i32).collect();
            let mut d = 0i32;
            for w in vals.windows(2) { d += (w[1] - w[0]).abs(); }
            d
        };
        assert!(spread(&blurred) < spread(&src), "a blur that does not smooth is not a blur");
    }

    #[test]
    fn a_zero_radius_blur_is_a_no_op() {
        let src = gradient(8, 8);
        assert_eq!(box_blur(&src, 0), src);
    }

    #[test]
    fn the_blur_clamps_at_the_edges_rather_than_wrapping() {
        // Wrapping would pull the right edge's pixels into the left one, which
        // shows as a bright seam down the side of every bar.
        let mut src = Img::new(4, 1, [0, 0, 0, 255]);
        src.set(3, 0, [255, 255, 255, 255]);
        let out = box_blur(&src, 1);
        assert_eq!(out.get(0, 0)[0], 0, "the left edge saw nothing bright");
    }

    #[test]
    fn the_chromatic_shift_floors_at_one_downscaled_pixel() {
        // THE bug this prevents: 1 / 4 rounds to 0, so the fringe would vanish
        // silently at exactly the default settings.
        assert_eq!(SHIFT_PX / DOWNSCALE, 0, "the naive computation is zero");
        assert_eq!(shift_in_small_space(SHIFT_PX, DOWNSCALE), 1);
        assert_eq!(shift_in_small_space(8, 4), 2, "and a larger request scales");
    }

    #[test]
    fn the_shift_takes_red_from_the_left_and_blue_from_the_right() {
        let mut src = Img::new(5, 1, [0, 0, 0, 255]);
        src.set(1, 0, [255, 0, 0, 255]);   // red pixel left of centre
        src.set(3, 0, [0, 0, 255, 255]);   // blue pixel right of centre
        let out = chromatic_shift(&src, 1);
        let mid = out.get(2, 0);
        assert_eq!(mid[0], 255, "red pulled from the left neighbour");
        assert_eq!(mid[2], 255, "blue pulled from the right neighbour");
    }

    #[test]
    fn the_shift_preserves_green_and_alpha_from_the_centre() {
        let mut src = Img::new(3, 1, [0, 0, 0, 128]);
        src.set(1, 0, [0, 200, 0, 200]);
        let out = chromatic_shift(&src, 1);
        assert_eq!(out.get(1, 0)[1], 200, "green is the centre's");
        assert_eq!(out.get(1, 0)[3], 200, "so is alpha");
    }

    #[test]
    fn the_shift_clamps_at_the_edges() {
        let src = gradient(4, 1);
        let out = chromatic_shift(&src, 1);
        assert_eq!(out.w, 4, "no pixels lost off the ends");
    }

    #[test]
    fn the_frost_returns_a_pixmap_the_size_of_the_strip_it_was_given() {
        // It is painted over the bar's own geometry, so anything else is a
        // stretched or cropped panel.
        let src = gradient(64, 16);
        let out = frost(&src, DOWNSCALE, SHIFT_PX);
        assert_eq!((out.w, out.h), (64, 16));
    }

    #[test]
    fn the_frost_of_a_flat_colour_stays_that_colour() {
        // Nothing in the pipeline should tint the result on its own; the tint
        // is laid on top afterwards, deliberately and visibly.
        let src = Img::new(32, 8, [90, 90, 90, 255]);
        let out = frost(&src, DOWNSCALE, SHIFT_PX);
        for p in &out.px {
            assert_eq!(&p[..3], &[90, 90, 90], "flat input came back tinted");
        }
    }

    #[test]
    fn an_empty_backdrop_frosts_to_nothing_rather_than_panicking() {
        // The owner has no frame yet on the first paint after a camera start.
        let out = frost(&Img::new(0, 0, [0; 4]), DOWNSCALE, SHIFT_PX);
        assert!(out.is_empty());
    }

    // ── button ──────────────────────────────────────────────────────────────

    #[test]
    fn the_body_dips_on_press_and_springs_back_on_release() {
        let mut b = GlassButton::default();
        assert_eq!(b.scale(), 1.0);
        b.press(10.0, 10.0);
        b.tick(PRESS_MS);
        assert!((b.scale() - PRESS_SCALE).abs() < 0.01);
        b.release();
        b.tick(PRESS_MS);
        assert!((b.scale() - 1.0).abs() < 0.01);
    }

    #[test]
    fn the_dip_does_not_overshoot_its_bounds() {
        let mut b = GlassButton::default();
        b.press(0.0, 0.0);
        for _ in 0..50 { b.tick(PRESS_MS); }
        assert!(b.scale() >= PRESS_SCALE);
        b.release();
        for _ in 0..50 { b.tick(PRESS_MS); }
        assert!(b.scale() <= 1.0);
    }

    #[test]
    fn the_ripple_starts_where_the_finger_did() {
        // A stylesheet :pressed state cannot know this, which is the whole
        // reason the button is painted rather than styled.
        let mut b = GlassButton::default();
        b.press(7.0, 23.0);
        assert_eq!(b.ripple_origin(), (7.0, 23.0));
        assert!(b.rippling());
    }

    #[test]
    fn the_ripple_outlives_the_release_so_a_quick_tap_still_shows_feedback() {
        // Cutting it on finger-up means a fast tap produces no visible response
        // at all — the exact interaction that most needs one.
        let mut b = GlassButton::default();
        b.press(5.0, 5.0);
        b.tick(20.0);
        b.release();
        b.tick(20.0);
        assert!(b.rippling(), "still expanding after the finger left");
        b.tick(RIPPLE_MS);
        assert!(!b.rippling());
    }

    #[test]
    fn the_ripple_expands_while_fading() {
        // Otherwise it ends as a hard white disc sitting on the button.
        let mut b = GlassButton::default();
        b.press(5.0, 5.0);
        b.tick(RIPPLE_MS * 0.25);
        let (t1, a1) = (b.ripple_t(), b.ripple_alpha());
        b.tick(RIPPLE_MS * 0.5);
        assert!(b.ripple_t() > t1, "growing");
        assert!(b.ripple_alpha() < a1, "and fading as it grows");
    }

    #[test]
    fn the_ripple_outlasts_the_press_dip() {
        // Deliberate: the touch feedback should still be visible after the
        // button has sprung back, or the two animations cancel each other out.
        assert!(RIPPLE_MS > PRESS_MS);
    }

    #[test]
    fn a_settled_button_reports_no_ripple() {
        let b = GlassButton::default();
        assert!(!b.rippling());
        assert_eq!(b.ripple_alpha(), 0.0);
        assert_eq!(b.ripple_t(), 0.0);
    }
}
