//! A glass panel over the video.
//!
//! Two problems, and only one of them is decoration.
//!
//! # egui has no backdrop filter
//!
//! There is no `backdrop-filter: blur()` here and no way to sample the
//! framebuffer from a widget. So the blur is drawn rather than filtered: the
//! frame texture is painted again under the panel a handful of times, each
//! offset by a pixel or two and at a fraction of the alpha, which averages
//! neighbouring pixels the same way a box blur does. It costs a few extra
//! textured quads over the panel area only.
//!
//! **What this achieves, measured by screenshot at each step.** Painting the
//! blur in a layer *behind* a translucent `Window` does nothing: the window
//! paints its own frame over it and the raw video shows through on top,
//! drowning the blur. The panel therefore has to be ONE layer — an `Area` that
//! composites backdrop and tint into the same rect with no frame above them —
//! which is what `settings::show_over` now does.
//!
//! Even then the first radius was invisible: 3 points against 60-point colour
//! bands moves nothing a viewer can see. At 14 points the band edges become
//! gradients. That is genuine softening and it is honestly ghosting rather
//! than a frost — six offset copies average neighbours, they do not convolve a
//! kernel. A real separable blur needs the glow path and a render target, and
//! would make a settings panel depend on the GPU feature.
//!
//! It is an approximation and looks like one at large radii, which is why
//! [`GlassStyle::taps`] is small. The honest alternative — a real separable
//! blur in a paint callback — needs the glow path and a render target, and
//! would make a settings panel depend on the GPU feature.
//!
//! # Readability is a number, not a taste
//!
//! A translucent panel over *arbitrary video* is the easiest way to ship
//! unreadable text: it looks fine over the dark clip it was designed against
//! and vanishes over a snow scene. The scrim opacity here is chosen so the
//! text clears a contrast floor over BOTH extremes — a white frame and a black
//! one — and [`contrast_ratio`] is the WCAG formula so the check is arithmetic
//! rather than an opinion. `readable_over_any_video` fails if a future tweak to
//! the tint breaks it.

use egui::{Color32, Rect, Ui};

#[derive(Copy, Clone, Debug)]
pub struct GlassStyle {
    /// Panel tint, composited over the blurred video.
    pub tint: Color32,
    /// How many offset copies of the backdrop to draw. More is smoother and
    /// costs a textured quad each; beyond about 6 the gain is invisible.
    pub taps: u8,
    /// Offset of the outermost tap, in points.
    pub radius: f32,
    pub corner_radius: f32,
    pub border: Color32,
    /// Secondary text colour, owned here rather than taken from the palette.
    ///
    /// The usual grey for hints is the thing that fails on glass: at the tint
    /// opacity that lets the backdrop show, a dim grey drops under 3:1 over a
    /// bright frame. Dimming secondary text is a habit from opaque surfaces.
    pub hint_text: Color32,
}

impl Default for GlassStyle {
    fn default() -> Self {
        Self {
            // Alpha 170/255, and every digit of that is from the contrast
            // arithmetic rather than taste. 216 was my guess and it made the
            // panel effectively opaque -- the blur underneath it was invisible,
            // so the whole backdrop was wasted work. Body text only needs 150;
            // the HINT text was the binding constraint, and the answer to that
            // was to stop dimming it rather than to thicken the glass.
            tint: Color32::from_rgba_unmultiplied(20, 18, 26, 170),
            taps: 6,
            // 14 points, not 3. Three was invisible by construction: the
            // offsets have to be a noticeable fraction of the features being
            // blurred, and 3pt against 60pt colour bands moves nothing a
            // viewer can see. Verified by screenshot, not by reasoning.
            radius: 14.0,
            corner_radius: 12.0,
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 46),
            hint_text: Color32::from_gray(205),
        }
    }
}

/// WCAG relative luminance of an opaque colour.
pub fn relative_luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// WCAG contrast ratio between two opaque colours, 1.0 to 21.0.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Composite `over` onto `under`, both straight alpha.
///
/// Needed because the readable colour is not the tint: it is the tint AS SEEN
/// over whatever the video happens to be, and checking the tint alone is how a
/// panel passes review and fails on a bright scene.
pub fn composite(under: Color32, over: Color32) -> Color32 {
    let a = over.a() as f32 / 255.0;
    let mix = |u: u8, o: u8| ((o as f32 * a) + (u as f32 * (1.0 - a))).round() as u8;
    Color32::from_rgb(
        mix(under.r(), over.r()),
        mix(under.g(), over.g()),
        mix(under.b(), over.b()),
    )
}

/// The worst backdrop a video can present: pure white and pure black.
pub const EXTREMES: [Color32; 2] = [Color32::WHITE, Color32::BLACK];

/// WCAG AA for body text.
pub const READABLE: f32 = 4.5;

/// Draw the glass: blurred backdrop, tint, border.
///
/// `backdrop` is the video texture and `uv` the part of it behind `rect`, so
/// the blur samples the actual picture rather than a flat colour. With no
/// texture — nothing playing — the tint alone is drawn, which is correct
/// rather than a fallback: there is nothing to blur.
pub fn draw(
    ui: &Ui,
    rect: Rect,
    style: &GlassStyle,
    backdrop: Option<(egui::TextureId, Rect)>,
) {
    let p = ui.painter();

    if let Some((tex, uv)) = backdrop {
        // Offset copies at a fraction of the alpha each: an average of
        // neighbouring pixels, which is what a box blur is. Drawn from the
        // outside in so the centre tap lands last and dominates.
        let taps = style.taps.max(1) as usize;
        let per = (255.0 / taps as f32) as u8;
        for i in 0..taps {
            let t = i as f32 / taps as f32;
            let off = style.radius * (1.0 - t);
            let angle = t * std::f32::consts::TAU;
            let shift = egui::vec2(angle.cos() * off, angle.sin() * off);
            p.image(
                tex,
                rect.translate(shift),
                uv,
                Color32::from_white_alpha(per),
            );
        }
    }

    p.rect_filled(rect, style.corner_radius, style.tint);
    p.rect_stroke(
        rect,
        style.corner_radius,
        egui::Stroke::new(1.0_f32, style.border),
        egui::StrokeKind::Inside,
    );
}

/// The egui frame to hand a `Window` so its own background does not fight the
/// glass drawn behind it.
/// Is the backdrop actually visible through the tint?
///
/// A "glass" panel opaque enough to hide what is behind it is just a panel,
/// and the blur drawn under it is work nobody sees. This is the other half of
/// the trade `readable_over_any_video` pins.
pub fn backdrop_visible(style: &GlassStyle) -> bool {
    style.tint.a() < 200
}

pub fn window_frame(style: &GlassStyle) -> egui::Frame {
    egui::Frame::NONE
        .fill(style.tint)
        .corner_radius(style.corner_radius)
        .stroke(egui::Stroke::new(1.0_f32, style.border))
        .inner_margin(14.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_luminance_formula_matches_the_reference_points() {
        assert!((relative_luminance(Color32::BLACK)).abs() < 0.001);
        assert!((relative_luminance(Color32::WHITE) - 1.0).abs() < 0.001);
        // Green carries most of the luminance, which is the whole point of the
        // weighting -- an implementation averaging the channels fails here.
        assert!(
            relative_luminance(Color32::GREEN) > relative_luminance(Color32::RED),
            "the channel weights are wrong"
        );
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        let c = contrast_ratio(Color32::WHITE, Color32::BLACK);
        assert!((c - 21.0).abs() < 0.1, "black on white should be 21:1, got {c}");
        assert_eq!(
            contrast_ratio(Color32::WHITE, Color32::BLACK),
            contrast_ratio(Color32::BLACK, Color32::WHITE)
        );
        assert!((contrast_ratio(Color32::RED, Color32::RED) - 1.0).abs() < 0.001);
    }

    #[test]
    fn compositing_respects_alpha_at_both_ends() {
        let opaque = Color32::from_rgba_unmultiplied(10, 20, 30, 255);
        assert_eq!(composite(Color32::WHITE, opaque), Color32::from_rgb(10, 20, 30));
        let clear = Color32::from_rgba_unmultiplied(10, 20, 30, 0);
        assert_eq!(composite(Color32::WHITE, clear), Color32::from_rgb(255, 255, 255));
    }

    /// THE POINT OF THE MODULE. A translucent panel is the easiest way to ship
    /// unreadable text: it looks fine over the dark clip it was designed
    /// against and vanishes over a snow scene. So the tint has to clear the
    /// floor over BOTH extremes, not the one that was convenient.
    #[test]
    fn text_is_readable_over_any_video() {
        let style = GlassStyle::default();
        let text = Color32::WHITE;
        for video in EXTREMES {
            let seen = composite(video, style.tint);
            let ratio = contrast_ratio(text, seen);
            assert!(
                ratio >= READABLE,
                "white text over a {video:?} frame is {ratio:.2}:1, below {READABLE}"
            );
        }
    }

    /// ...and the weak/hint text too, which is the one that actually gets
    /// dropped: it passes on the panel colour alone and fails composited.
    #[test]
    fn hint_text_is_readable_over_any_video() {
        let style = GlassStyle::default();
        let hint = style.hint_text;
        for video in EXTREMES {
            let seen = composite(video, style.tint);
            let ratio = contrast_ratio(hint, seen);
            assert!(
                ratio >= 3.0,
                "hint text over a {video:?} frame is {ratio:.2}:1"
            );
        }
    }

    /// The failure this guards against, stated as a test: a tint light enough
    /// to look nice over dark video is unreadable over bright video.
    #[test]
    fn a_too_transparent_tint_is_caught() {
        let translucent = Color32::from_rgba_unmultiplied(20, 18, 26, 90);
        let seen = composite(Color32::WHITE, translucent);
        assert!(
            contrast_ratio(Color32::WHITE, seen) < READABLE,
            "the check cannot fail, so it is not checking anything"
        );
    }

    /// The other half of the trade. A tint opaque enough to hide the video is
    /// just a panel, and the blur under it is work nobody sees -- which is
    /// exactly what my first guess of 216 produced.
    #[test]
    fn the_backdrop_is_actually_visible() {
        assert!(
            backdrop_visible(&GlassStyle::default()),
            "the tint is opaque enough that the blur behind it is invisible"
        );
    }

    /// Both constraints at once, which is the only way to notice they pull
    /// against each other.
    #[test]
    fn readable_and_translucent_at_the_same_time() {
        let s = GlassStyle::default();
        assert!(backdrop_visible(&s));
        for video in EXTREMES {
            let seen = composite(video, s.tint);
            assert!(contrast_ratio(Color32::WHITE, seen) >= READABLE);
            assert!(contrast_ratio(s.hint_text, seen) >= 3.0);
        }
    }

    #[test]
    fn the_tap_count_stays_cheap() {
        let s = GlassStyle::default();
        assert!(s.taps >= 3, "too few taps to read as a blur");
        assert!(
            s.radius >= 8.0,
            "a radius smaller than the features being blurred does nothing visible"
        );
        assert!(
            s.taps <= 6,
            "each tap is a textured quad over the panel; beyond six the gain is invisible"
        );
    }
}
