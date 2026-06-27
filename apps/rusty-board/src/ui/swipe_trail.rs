//! Swipe trail overlay — draws the active gesture path on top of the
//! keyboard with a soft fading gradient.
//!
//! Implementation strategy:
//! - We use a `gtk4::DrawingArea` as the trail surface.
//! - The keyboard view places it inside a `gtk4::Overlay`, layered above
//!   the key grid.
//! - The DrawingArea passes pointer events through to the keyboard below
//!   (via `set_can_target(false)`), so gestures still register on the keys.
//! - On every frame during an active swipe, we update the points and call
//!   `queue_draw()`. Cairo renders a smooth, anti-aliased path with width
//!   and alpha tapering toward the tail.
//!
//! Visual style:
//! - Quadratic Bézier smoothing between consecutive points.
//! - Stroke width tapers from 7px at the head to 2px at the tail.
//! - Alpha tapers from 0.85 to 0.05 along the same axis.
//! - Color comes from the configured accent.

use cairo::{Context, LineCap, LineJoin};
use gtk4::prelude::*;
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::gestures::swipe::SwipePoint;

/// Process-wide opt-in for debug trail styling. When OFF (default),
/// trails always render Solid in the configured accent color — what
/// normal users want. When ON, the trail's style/color reflects the
/// gesture category (set via `SwipeTrail::set_style`), so you can SEE
/// on-device which branch a gesture fell into: dotted = cardinal,
/// striped = swipe-word, desaturated = normal/ignored, etc.
///
/// Toggled from settings or config (`appearance.debug_trail_styling`).
static TRAIL_DEBUG_STYLING: AtomicBool = AtomicBool::new(false);

pub fn set_trail_debug_styling(on: bool) {
    TRAIL_DEBUG_STYLING.store(on, Ordering::Relaxed);
}

pub fn trail_debug_styling() -> bool {
    TRAIL_DEBUG_STYLING.load(Ordering::Relaxed)
}

/// Visual treatment for the swipe trail. In debug-styling mode each
/// gesture category gets a distinct look:
/// - `Solid`       — normal accent line (default / swipe-source when
///                    debug styling is off)
/// - `Desaturated` — muted gray; "nothing useful here" (Normal-tap
///                    drag, off-key start, ignored)
/// - `Dotted`      — round dots; cardinal-grid / steno cell swipes
/// - `Striped`     — dashed segments; freeform swipe-typed words
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailStyle {
    Solid,
    Desaturated,
    Dotted,
    Striped,
}

impl TrailStyle {
    /// Map a source-key behavior label to a debug trail style, so the
    /// live trail reflects the gesture category as the user draws.
    pub fn for_behavior_label(label: &str) -> Self {
        match label {
            "cardinal" => TrailStyle::Dotted,
            "swipe" => TrailStyle::Striped,
            "normal" | "none" => TrailStyle::Desaturated,
            _ => TrailStyle::Solid,
        }
    }
}

#[derive(Clone)]
pub struct SwipeTrail {
    drawing_area: gtk4::DrawingArea,
    state: Arc<RwLock<TrailState>>,
}

struct TrailState {
    /// Path points in widget-local coordinates.
    points: Vec<(f64, f64)>,
    /// Trail color as RGB in [0, 1].
    rgb: (f64, f64, f64),
    /// Current visibility (fades out after the gesture ends).
    alpha: f64,
    /// Fade generation. Bumped whenever a new gesture starts (points
    /// pushed). A running fade-out timer captures the generation at
    /// spawn; if it no longer matches when the timer fires, a new
    /// gesture has begun and the stale timer breaks instead of
    /// fighting the fresh trail's alpha. Prevents stacked 60Hz
    /// timers when the user swipes rapidly.
    fade_gen: u64,
    /// Current visual treatment. Only honored when debug styling is on
    /// (otherwise the draw path forces Solid). Set per-gesture.
    style: TrailStyle,
}

impl Default for TrailState {
    fn default() -> Self {
        Self {
            points: Vec::with_capacity(128),
            rgb: (0.21, 0.52, 0.89), // GNOME blue (#3584e4)
            alpha: 1.0,
            fade_gen: 0,
            style: TrailStyle::Solid,
        }
    }
}

impl SwipeTrail {
    pub fn new() -> Self {
        let drawing_area = gtk4::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .can_target(false) // don't intercept pointer events
            .css_classes(["swipe-trail"])
            .build();

        let state = Arc::new(RwLock::new(TrailState::default()));

        // Wire up the draw callback. GTK4 calls this on every redraw with
        // a fresh Cairo context sized to the widget.
        let state_for_draw = state.clone();
        drawing_area.set_draw_func(move |_area, cr, _width, _height| {
            let s = state_for_draw.read();
            if s.points.len() < 2 || s.alpha <= 0.01 {
                return;
            }
            // Debug styling off → always Solid (normal user experience).
            let style = if trail_debug_styling() {
                s.style
            } else {
                TrailStyle::Solid
            };
            draw_trail(cr, &s.points, s.rgb, s.alpha, style);
        });

        Self {
            drawing_area,
            state,
        }
    }

    pub fn widget(&self) -> &gtk4::DrawingArea {
        &self.drawing_area
    }

    /// Set the trail color (as a 6-char hex without `#`, or any prefix
    /// that the recognizer can parse). Falls back to GNOME blue on parse
    /// error.
    pub fn set_color_hex(&self, hex: &str) {
        let cleaned: String = hex
            .trim_start_matches('#')
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(6)
            .collect();
        if cleaned.len() == 6 {
            let r = u8::from_str_radix(&cleaned[0..2], 16).unwrap_or(53) as f64 / 255.0;
            let g = u8::from_str_radix(&cleaned[2..4], 16).unwrap_or(132) as f64 / 255.0;
            let b = u8::from_str_radix(&cleaned[4..6], 16).unwrap_or(228) as f64 / 255.0;
            self.state.write().rgb = (r, g, b);
        }
    }

    /// Set the trail's visual treatment for the current/next gesture.
    /// Only takes visible effect when debug styling is enabled; stored
    /// regardless so toggling debug styling on mid-gesture is coherent.
    /// Triggers a redraw so a style change is reflected immediately.
    pub fn set_style(&self, style: TrailStyle) {
        {
            let mut s = self.state.write();
            if s.style == style {
                return;
            }
            s.style = style;
        }
        self.drawing_area.queue_draw();
    }

    /// Push a point to the trail (in widget-local coordinates) and
    /// schedule a redraw.
    pub fn push(&self, x: f64, y: f64) {
        {
            let mut s = self.state.write();
            // First point of a new gesture → invalidate any running
            // fade-out timer so it doesn't fight this fresh trail.
            if s.points.is_empty() {
                s.fade_gen = s.fade_gen.wrapping_add(1);
            }
            s.points.push((x, y));
            s.alpha = 1.0;
            // Cap the length to avoid unbounded growth on long
            // gestures, but amortize the drain cost. Vec::drain(0..k)
            // shifts the entire tail down by k positions regardless
            // of whether k is 1 or 32 — the per-push cost was
            // proportional to the cap (256 entries memmoved per push
            // beyond the cap). On a long character-draw stroke at
            // ~60Hz that's 60 × 256 = 15k+ element moves/sec just for
            // cap maintenance.
            //
            // Letting the Vec grow to cap + 32 before trimming back
            // to cap reduces drain frequency by ~32x while keeping
            // the effective trail window at ~256. The 32-point
            // overshoot is invisible (~128px of extra path stored
            // briefly, rendered at low alpha as the band-stripe taper).
            const TRAIL_CAP: usize = 256;
            const TRAIL_TRIM_SLACK: usize = 32;
            if s.points.len() > TRAIL_CAP + TRAIL_TRIM_SLACK {
                let drop = s.points.len() - TRAIL_CAP;
                s.points.drain(0..drop);
            }
        }
        self.drawing_area.queue_draw();
    }

    /// Replace the current trail with a list of points (e.g. supplied by
    /// the gesture controller in one batch).
    pub fn set_points(&self, points: &[SwipePoint], origin: (f64, f64)) {
        {
            let mut s = self.state.write();
            // New trail content invalidates any running fade timer.
            s.fade_gen = s.fade_gen.wrapping_add(1);
            s.points.clear();
            s.points.extend(
                points
                    .iter()
                    .map(|p| (p.x - origin.0, p.y - origin.1)),
            );
            s.alpha = 1.0;
        }
        self.drawing_area.queue_draw();
    }

    /// Begin fading out the trail. Call when the gesture ends; the trail
    /// will animate to invisible over ~280ms and then clear itself.
    pub fn fade_out(&self) {
        let state = self.state.clone();
        let area = self.drawing_area.clone();
        // Capture the generation at spawn. If a new gesture starts
        // mid-fade, `push` bumps the generation and this timer sees
        // the mismatch and breaks, leaving the new gesture's trail
        // alone. Prevents stacked fade timers fighting over alpha.
        let my_gen = state.read().fade_gen;
        // 33ms (~30Hz) tick with 0.12 step per tick: ~280ms duration,
        // ~9 frames. The fade isn't user-driven — it's an automatic
        // dissipation effect — and the per-frame Cairo stroke work
        // is the heaviest per-frame thing we do on phone GPU. 30Hz
        // looks identical to 60Hz for a trail that's already in motion
        // (motion blur masks any stepping), while halving the
        // per-fade render cost.
        glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
            let mut done = false;
            {
                let mut s = state.write();
                if s.fade_gen != my_gen {
                    // A newer gesture owns the trail now — stop.
                    return glib::ControlFlow::Break;
                }
                s.alpha -= 0.12;
                if s.alpha <= 0.0 {
                    s.alpha = 0.0;
                    s.points.clear();
                    done = true;
                }
            }
            area.queue_draw();
            if done {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Hard clear — for cancellation.
    pub fn clear(&self) {
        {
            let mut s = self.state.write();
            s.points.clear();
            s.alpha = 0.0;
        }
        self.drawing_area.queue_draw();
    }
}

impl Default for SwipeTrail {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the trail. Cairo's `stroke()` applies one width/alpha
/// per call, but a tapered look needs varying widths along the
/// path. Rather than stroking each segment individually (which
/// previously cost O(N) Cairo render calls and was a measurable
/// bottleneck on phone), we **quantize** widths into a fixed
/// number of bands and stroke each band as a single sub-path.
/// 6 bands per layer (outer halo + inner stroke) → constant 12
/// strokes regardless of path length.
///
/// Visually this is nearly indistinguishable from the per-segment
/// taper for 60-point paths; the 6-band quantization granularity
/// is smaller than the eye can see at the relevant widths.
fn draw_trail(
    cr: &Context,
    points: &[(f64, f64)],
    rgb: (f64, f64, f64),
    alpha_mul: f64,
    style: TrailStyle,
) {
    let n = points.len();
    if n < 2 {
        return;
    }

    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);
    // Antialias::Good (over Best) cuts Cairo's per-stroke cost
    // significantly on phone GPUs while producing a visually
    // indistinguishable trail at the widths we use here. Best is
    // designed for static print-quality rendering, not 60Hz
    // touchscreen feedback.
    cr.set_antialias(cairo::Antialias::Good);

    // Apply the style's color transform. Desaturated lerps toward the
    // luminance gray so it reads as "muted / not a productive gesture."
    let rgb = match style {
        TrailStyle::Desaturated => {
            // Rec. 601 luma; lerp 65% toward gray.
            let y = 0.299 * rgb.0 + 0.587 * rgb.1 + 0.114 * rgb.2;
            let m = 0.65;
            (
                rgb.0 + (y - rgb.0) * m,
                rgb.1 + (y - rgb.1) * m,
                rgb.2 + (y - rgb.2) * m,
            )
        }
        _ => rgb,
    };

    // Stroke a single layer in `bands` quantized width/alpha bands.
    // `min_width`/`max_width` and `min_alpha`/`max_alpha` are the
    // taper endpoints. Inner-band index 0 is the tail (faint, thin);
    // last band is the head (bright, thick).
    fn stroke_layer(
        cr: &Context,
        points: &[(f64, f64)],
        rgb: (f64, f64, f64),
        alpha_mul: f64,
        min_width: f64,
        max_width: f64,
        min_alpha: f64,
        max_alpha: f64,
        bands: usize,
    ) {
        let n = points.len();
        if n < 2 || bands == 0 {
            return;
        }
        let bands_f = bands as f64;
        let denom = (n - 1) as f64;
        for b in 0..bands {
            let t = (b as f64 + 0.5) / bands_f;
            let alpha = (min_alpha + t * (max_alpha - min_alpha)) * alpha_mul;
            let width = min_width + t * (max_width - min_width);
            cr.set_source_rgba(rgb.0, rgb.1, rgb.2, alpha);
            cr.set_line_width(width);

            let band_start = (b as f64 / bands_f * denom) as usize;
            let band_end = (((b + 1) as f64 / bands_f * denom).ceil() as usize).min(n - 1);
            if band_end <= band_start {
                continue;
            }

            let (x0, y0) = points[band_start];
            cr.move_to(x0, y0);
            for i in band_start..band_end {
                let (x1, y1) = points[i];
                let (x2, y2) = points[i + 1];
                let mx = (x1 + x2) * 0.5;
                let my = (y1 + y2) * 0.5;
                cr.line_to(mx, my);
            }
            let _ = cr.stroke();
        }
    }

    // Style → Cairo dash. Round caps (set above) turn a near-zero
    // "on" length into a round dot. Striped uses a longer on/off for
    // dashes. Solid/Desaturated draw continuous. The dash is set on
    // the context before the band strokes so every band inherits it.
    match style {
        TrailStyle::Dotted => {
            // [on, off]: tiny on with round cap = dot ~line_width wide,
            // spaced ~14px. Offset 0.
            cr.set_dash(&[0.1, 14.0], 0.0);
        }
        TrailStyle::Striped => {
            cr.set_dash(&[10.0, 8.0], 0.0);
        }
        TrailStyle::Solid | TrailStyle::Desaturated => {
            cr.set_dash(&[], 0.0);
        }
    }

    // Outer halo: wide, faint.
    stroke_layer(cr, points, rgb, alpha_mul, 4.0, 18.0, 0.04, 0.22, 3);
    // Inner core: narrow, bright.
    stroke_layer(cr, points, rgb, alpha_mul, 1.5, 7.0, 0.05, 0.90, 3);

    // Reset dash before the head dot so the dot fill isn't affected
    // (fills ignore dash, but defensive for any future stroke).
    cr.set_dash(&[], 0.0);

    // Brighter dot at the head so the tip is visible even when
    // stationary. Suppressed for Dotted/Striped so the pattern reads
    // cleanly without a solid blob at the end.
    if matches!(style, TrailStyle::Solid | TrailStyle::Desaturated) {
        if let Some(&(hx, hy)) = points.last() {
            cr.set_source_rgba(rgb.0, rgb.1, rgb.2, 0.95 * alpha_mul);
            cr.arc(hx, hy, 5.5, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();

            // White inner highlight for that "wet" look.
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.7 * alpha_mul);
            cr.arc(hx, hy, 2.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }
}

#[cfg(test)]
mod tests {
    /// Sanity check the hex-parsing helper logic. We can't construct a
    /// SwipeTrail without GTK initialized, so we just verify the parse
    /// rules in isolation.
    #[test]
    fn hex_parsing_strips_non_hex_chars() {
        let input = "garbage_input";
        let cleaned: String = input
            .trim_start_matches('#')
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(6)
            .collect();
        // "garbage_input" has only 'a' 'a' 'e' 'a' = 4 hex chars → won't
        // reach the 6-char branch; color stays at default.
        assert!(cleaned.len() < 6);
    }

    #[test]
    fn hex_parsing_accepts_six_chars_with_hash() {
        let cleaned: String = "#3584e4"
            .trim_start_matches('#')
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(6)
            .collect();
        assert_eq!(cleaned, "3584e4");
    }

    #[test]
    fn behavior_label_maps_to_expected_style() {
        use super::TrailStyle;
        assert_eq!(TrailStyle::for_behavior_label("cardinal"), TrailStyle::Dotted);
        assert_eq!(TrailStyle::for_behavior_label("swipe"), TrailStyle::Striped);
        assert_eq!(TrailStyle::for_behavior_label("normal"), TrailStyle::Desaturated);
        assert_eq!(TrailStyle::for_behavior_label("none"), TrailStyle::Desaturated);
        // Unknown labels fall back to Solid.
        assert_eq!(TrailStyle::for_behavior_label("anything-else"), TrailStyle::Solid);
    }

    #[test]
    fn debug_styling_toggle_roundtrips() {
        use super::{set_trail_debug_styling, trail_debug_styling};
        // Default is off.
        set_trail_debug_styling(false);
        assert!(!trail_debug_styling());
        set_trail_debug_styling(true);
        assert!(trail_debug_styling());
        // Reset so we don't leak state into other tests sharing the
        // process-wide atomic.
        set_trail_debug_styling(false);
    }
}
