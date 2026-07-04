//! livebg — live animated backgrounds, extracted from snapclone (the founder) into the shared
//! kit so any egui app can use them: localweather, av1tuner on mobile, snapclone itself.
//!
//! An immediate-mode design: [`LiveBackground`] holds a tiny animation phase advanced each
//! frame by [`LiveBackground::tick`]; [`LiveBackground::paint`] draws the current frame with
//! egui's painter using palette colours (so it re-themes and respects the display filter via
//! the caller). No retained scene, no allocation per frame beyond a few shapes. The motion
//! math is pure and unit-tested.

use crate::PaletteColors;
use egui::{Color32, Painter, Pos2, Rect, Stroke};

/// The animated background style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgStyle {
    /// Soft flowing aurora bands.
    Aurora,
    /// Layered sine waves drifting sideways.
    Waves,
    /// Slow drifting starfield/particles.
    Starfield,
    /// A slowly cycling two-colour gradient.
    Gradient,
}

impl BgStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            BgStyle::Aurora => "aurora",
            BgStyle::Waves => "waves",
            BgStyle::Starfield => "starfield",
            BgStyle::Gradient => "gradient",
        }
    }
    pub fn parse(s: &str) -> BgStyle {
        match s {
            "waves" => BgStyle::Waves,
            "starfield" => BgStyle::Starfield,
            "gradient" => BgStyle::Gradient,
            _ => BgStyle::Aurora,
        }
    }
    pub fn all() -> [BgStyle; 4] {
        [BgStyle::Aurora, BgStyle::Waves, BgStyle::Starfield, BgStyle::Gradient]
    }
}

/// A live animated background.
#[derive(Debug, Clone)]
pub struct LiveBackground {
    pub style: BgStyle,
    /// Animation accumulator (seconds * speed).
    pub phase: f32,
    /// Motion speed multiplier (0 = static).
    pub speed: f32,
    /// Overall intensity/opacity 0..1.
    pub intensity: f32,
}

impl LiveBackground {
    pub fn new(style: BgStyle) -> LiveBackground {
        LiveBackground { style, phase: 0.0, speed: 1.0, intensity: 1.0 }
    }

    /// Advance the animation by `dt` seconds. Call once per frame.
    pub fn tick(&mut self, dt: f32) {
        self.phase += dt * self.speed;
        // keep phase bounded so long-running apps don't lose float precision
        if self.phase > 10_000.0 {
            self.phase -= 10_000.0;
        }
    }

    /// Paint the current frame into `rect`.
    pub fn paint(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        match self.style {
            BgStyle::Gradient => self.paint_gradient(painter, rect, colors),
            BgStyle::Waves => self.paint_waves(painter, rect, colors),
            BgStyle::Aurora => self.paint_aurora(painter, rect, colors),
            BgStyle::Starfield => self.paint_starfield(painter, rect, colors),
        }
    }

    fn a(&self, base: f32) -> u8 {
        (base * self.intensity.clamp(0.0, 1.0)).clamp(0.0, 255.0) as u8
    }

    fn paint_gradient(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        let t = mix_factor(self.phase, 0);
        let top = lerp_color(colors.bg_base, colors.accent, t * 0.35);
        let bot = lerp_color(colors.bg_base, colors.accent_soft, (1.0 - t) * 0.35);
        // vertical gradient via horizontal strips
        let strips = 24;
        for i in 0..strips {
            let f = i as f32 / (strips - 1) as f32;
            let y0 = rect.top() + f * rect.height();
            let y1 = y0 + rect.height() / strips as f32 + 1.0;
            let c = lerp_color(top, bot, f);
            painter.rect_filled(Rect::from_min_max(egui::pos2(rect.left(), y0), egui::pos2(rect.right(), y1)), 0.0, c);
        }
    }

    fn paint_waves(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        painter.rect_filled(rect, 0.0, colors.bg_base);
        let layers = [(0.0f32, colors.accent, 0.18f32), (1.3, colors.accent_soft, 0.14), (2.1, colors.success, 0.10)];
        for (li, (off, color, amp_frac)) in layers.iter().enumerate() {
            let amp = rect.height() * amp_frac;
            let base_y = rect.top() + rect.height() * (0.55 + 0.12 * li as f32);
            let wl = rect.width() / (1.5 + li as f32);
            let n = 48;
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let x = rect.left() + rect.width() * i as f32 / n as f32;
                    let y = wave_y(base_y, x - rect.left(), self.phase + off, amp, wl);
                    egui::pos2(x, y)
                })
                .collect();
            let mut c = *color;
            c = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), self.a(90.0));
            painter.add(egui::Shape::line(pts, Stroke::new(2.0, c)));
        }
    }

    fn paint_aurora(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        painter.rect_filled(rect, 0.0, colors.bg_base);
        let bands = [colors.accent, colors.accent_soft, colors.success, colors.warning];
        for (i, color) in bands.iter().enumerate() {
            let f = mix_factor(self.phase, i);
            let cy = rect.top() + rect.height() * (0.2 + 0.6 * f);
            let h = rect.height() * 0.22;
            let c = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), self.a(46.0));
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(rect.left(), cy - h / 2.0), egui::pos2(rect.right(), cy + h / 2.0)),
                h / 2.0,
                c,
            );
        }
    }

    fn paint_starfield(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        painter.rect_filled(rect, 0.0, colors.bg_base);
        let n = 80;
        for i in 0..n {
            let (sx, sy) = star_pos(i, self.phase);
            let x = rect.left() + sx * rect.width();
            let y = rect.top() + sy * rect.height();
            let r = 0.6 + 1.6 * ((i % 5) as f32 / 5.0);
            let c = Color32::from_rgba_unmultiplied(colors.text.r(), colors.text.g(), colors.text.b(), self.a(120.0));
            painter.circle_filled(egui::pos2(x, y), r, c);
        }
    }
}

// ── pure motion math (unit-tested) ───────────────────────────────────────────

/// Sine wave height at `x` for a given phase.
pub fn wave_y(base_y: f32, x: f32, phase: f32, amp: f32, wavelength: f32) -> f32 {
    base_y + amp * (x / wavelength.max(1.0) * std::f32::consts::TAU + phase).sin()
}

/// A 0..1 oscillating mix factor for band/gradient `i` at `phase`.
pub fn mix_factor(phase: f32, i: usize) -> f32 {
    ((phase * 0.3 + i as f32 * 0.7).sin() * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// Deterministic star position (0..1, 0..1) for index `i` at `phase` (drifts + wraps).
pub fn star_pos(i: usize, phase: f32) -> (f32, f32) {
    let fx = ((i as f32 * 12.9898).sin() * 43758.547).fract().abs();
    let fy = ((i as f32 * 78.233).sin() * 12543.13).fract().abs();
    let drift = (phase * 0.02 + fy) % 1.0;
    (fx, (fy + drift) % 1.0)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances_and_wraps_phase() {
        let mut bg = LiveBackground::new(BgStyle::Aurora);
        bg.tick(0.5);
        assert!((bg.phase - 0.5).abs() < 1e-6);
        bg.speed = 2.0;
        bg.tick(1.0);
        assert!((bg.phase - 2.5).abs() < 1e-6);
        bg.phase = 10_001.0;
        bg.tick(0.0);
        assert!(bg.phase < 10_000.0, "phase wraps to preserve precision");
    }

    #[test]
    fn wave_oscillates_within_amplitude() {
        let amp = 10.0;
        for x in 0..100 {
            let y = wave_y(50.0, x as f32, 0.0, amp, 40.0);
            assert!((y - 50.0).abs() <= amp + 1e-3);
        }
    }

    #[test]
    fn mix_factor_stays_unit() {
        for i in 0..5 {
            for p in 0..20 {
                let m = mix_factor(p as f32 * 0.3, i);
                assert!((0.0..=1.0).contains(&m));
            }
        }
    }

    #[test]
    fn stars_stay_in_bounds_and_move() {
        let (x0, y0) = star_pos(3, 0.0);
        let (x1, y1) = star_pos(3, 5.0);
        assert!((0.0..=1.0).contains(&x0) && (0.0..=1.0).contains(&y0));
        assert!((0.0..=1.0).contains(&x1) && (0.0..=1.0).contains(&y1));
        assert_eq!(x0, x1, "x is stable per star");
        assert_ne!(y0, y1, "y drifts over time");
    }

    #[test]
    fn style_roundtrips() {
        for s in BgStyle::all() {
            assert_eq!(BgStyle::parse(s.as_str()), s);
        }
    }
}
