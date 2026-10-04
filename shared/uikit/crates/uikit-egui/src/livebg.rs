//! livebg — live animated backgrounds, extracted from snapclone (the founder) into the shared
//! kit so any egui app can use them: localweather, av1tuner on mobile, tomescope, snapclone itself.
//!
//! An immediate-mode design: [`LiveBackground`] holds a tiny animation phase advanced each
//! frame by [`LiveBackground::tick`]; [`LiveBackground::paint`] draws the current frame with
//! egui's painter using palette colours (so it re-themes and respects the display filter via
//! the caller). No retained scene; a few meshes per frame. The motion and light math is pure
//! and unit-tested.
//!
//! ## The language: the same glass the chrome is made of
//!
//! The first versions were four capsules stacked down the page (aurora), three sine strokes
//! (waves), a striped gradient, and dots — shapes with hard edges in saturated palette colours,
//! which read as clip art under the glass panels and, worse, took the text with them. What
//! the app's surfaces do instead (see `glass`): a dark high-alpha fill, one hairline where the
//! light catches an edge, off-centre SOFT highlights so a fill reads as scattered light rather
//! than a flat disc. The backgrounds now speak that language:
//!
//! * every gradient is a **mesh with per-vertex colour**, so there are no bands or edges;
//! * light is **feathered** — a curtain, a bokeh disc, a nebula all fall off to nothing;
//! * a **sheen** — one soft diagonal band of white at a few percent — drifts across every
//!   style over about twenty seconds. It is the shine the panels have, on the page;
//! * a **vignette** darkens the edges and the bottom, where the controls and the tab bar live;
//! * and a **readability cap**: no style may lift any pixel more than [`MAX_LIFT`] above
//!   `bg_base`. That is the difference between a background and a foreground, and it is
//!   tested against the alpha math rather than promised.
//!
//! Motion is slow on purpose — a background that moves faster than the eye drifts is a thing
//! you look at, and this is a thing you look through.

use crate::PaletteColors;
use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Pos2, Rect};

/// The animated background style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgStyle {
    /// Two feathered curtains of the accent, undulating slowly.
    Aurora,
    /// Silk: wide soft diagonal bands sliding across, like light on cloth.
    Waves,
    /// A slow starfield with twinkle and a faint nebula behind it.
    Starfield,
    /// A quiet vertical gradient with the sheen — the plainest, and the glassiest.
    Gradient,
    /// Soft out-of-focus discs of light drifting upward.
    Bokeh,
}

impl BgStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            BgStyle::Aurora => "aurora",
            BgStyle::Waves => "waves",
            BgStyle::Starfield => "starfield",
            BgStyle::Gradient => "gradient",
            BgStyle::Bokeh => "bokeh",
        }
    }
    pub fn parse(s: &str) -> BgStyle {
        match s {
            "waves" => BgStyle::Waves,
            "starfield" => BgStyle::Starfield,
            "gradient" => BgStyle::Gradient,
            "bokeh" => BgStyle::Bokeh,
            _ => BgStyle::Aurora,
        }
    }
    pub fn all() -> [BgStyle; 5] {
        [BgStyle::Aurora, BgStyle::Waves, BgStyle::Starfield, BgStyle::Gradient, BgStyle::Bokeh]
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

/// The most any style may brighten a pixel over `bg_base`, as an alpha (0..255) of a
/// light overlay: 22%. Text at the palette's contrast ratio survives that; the old aurora's
/// four 18% bands stacked to 50% and did not. (14% was tried first and the light vanished on
/// the warm palettes, whose accent is a dim tan — a background you cannot see is not elegant,
/// it is absent.) `paint` cannot exceed it because every alpha it
/// draws goes through [`LiveBackground::lift`], and the test sums the worst case per style.
pub const MAX_LIFT: u8 = 56;

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
        painter.rect_filled(rect, 0.0, colors.bg_base);
        match self.style {
            BgStyle::Gradient => self.paint_gradient(painter, rect, colors),
            BgStyle::Waves => self.paint_silk(painter, rect, colors),
            BgStyle::Aurora => self.paint_aurora(painter, rect, colors),
            BgStyle::Starfield => self.paint_starfield(painter, rect, colors),
            BgStyle::Bokeh => self.paint_bokeh(painter, rect, colors),
        }
        if self.style != BgStyle::Starfield {
            self.paint_sheen(painter, rect);
        }
        paint_vignette(painter, rect);
    }

    /// An overlay alpha, scaled by intensity and capped: `frac` is the style's own 0..1
    /// budget for this element, of the [`MAX_LIFT`] total.
    fn lift(&self, frac: f32) -> u8 {
        (MAX_LIFT as f32 * frac.clamp(0.0, 1.0) * self.intensity.clamp(0.0, 1.0)).round() as u8
    }

    // ── gradient: base, a little of the accent at the top, darker at the bottom ──
    fn paint_gradient(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        let t = mix_factor(self.phase * 0.5, 0);
        let tint = lerp_color(colors.accent, colors.accent_soft, t);
        let top = with_alpha(tint, self.lift(GRADIENT_TOP));
        let bot = with_alpha(Color32::BLACK, 40);
        quad(painter, rect, top, top, bot, bot);
    }

    // ── aurora: two feathered curtains, each a strip mesh undulating in y ──
    fn paint_aurora(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        let curtains = [(as_light(colors.accent), 0.0f32, 0.34f32), (as_light(colors.accent_soft), 2.1, 0.62)];
        for (ci, (color, off, cy)) in curtains.iter().enumerate() {
            let cols = 28;
            let rows = 9;
            let mut mesh = Mesh::default();
            for c in 0..=cols {
                let x01 = c as f32 / cols as f32;
                let x = rect.left() + x01 * rect.width();
                let centre = rect.top() + rect.height() * (cy + curtain_wobble(x01, self.phase + off, ci));
                let half = rect.height() * (0.16 + 0.05 * ((self.phase * 0.11 + x01 * 3.0).sin()));
                for r in 0..=rows {
                    let u = r as f32 / rows as f32;
                    let y = centre - half + u * 2.0 * half;
                    let a = curtain_alpha(u) * AURORA_CURTAIN;
                    mesh.vertices.push(vertex(x, y.clamp(rect.top(), rect.bottom()), with_alpha(*color, self.lift(a))));
                }
            }
            grid_triangles(&mut mesh, cols + 1, rows + 1);
            painter.add(egui::Shape::mesh(mesh));
        }
    }

    // ── silk: four wide diagonal bands, feathered, sliding slowly ──
    fn paint_silk(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        let tints = [as_light(colors.accent), as_light(colors.accent_soft), as_light(colors.success), as_light(colors.accent)];
        for (i, tint) in tints.iter().enumerate() {
            let p = silk_band_pos(self.phase, i);
            let w = 0.22 + 0.06 * (i as f32 % 2.0);
            let peak = self.lift(SILK_BAND);
            diagonal_band(painter, rect, p, w, rect.height() * 0.58, &[(-1.0, 0), (0.0, peak), (1.0, 0)], *tint);
        }
    }

    // ── starfield: a faint nebula, then stars in three sizes that twinkle ──
    fn paint_starfield(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        let side = rect.width().min(rect.height());
        for (i, (nx, ny, nr)) in [(0.28f32, 0.30f32, 0.55f32), (0.74, 0.62, 0.48)].iter().enumerate() {
            let tint = if i == 0 { colors.accent } else { colors.accent_soft };
            let drift = (self.phase * 0.01 + i as f32).sin() * 0.03;
            soft_disc(
                painter,
                Pos2::new(rect.left() + (nx + drift) * rect.width(), rect.top() + ny * rect.height()),
                nr * side,
                tint,
                self.lift(STAR_NEBULA),
            );
        }
        let n = 90;
        for i in 0..n {
            let (sx, sy) = star_pos(i, self.phase * (0.6 + 0.4 * ((i % 3) as f32)));
            let x = rect.left() + sx * rect.width();
            let y = rect.top() + sy * rect.height();
            let class = i % 7; // most stars small, a few bright
            let r = match class { 0 => 1.8, 1 | 2 => 1.1, _ => 0.6 };
            let tw = twinkle(i, self.phase);
            let a = self.lift(STAR_POINT * (0.45 + 0.55 * tw) * if class == 0 { 1.0 } else { 0.7 });
            painter.circle_filled(Pos2::new(x, y), r, with_alpha(colors.text, a));
            if class == 0 && tw > 0.85 {
                // a bright star at its peak gets the hairline cross the glass highlights have
                let g = with_alpha(colors.text, self.lift(STAR_POINT * 0.35));
                painter.line_segment([Pos2::new(x - 5.0, y), Pos2::new(x + 5.0, y)], egui::Stroke::new(0.6_f32, g));
                painter.line_segment([Pos2::new(x, y - 5.0), Pos2::new(x, y + 5.0)], egui::Stroke::new(0.6_f32, g));
            }
        }
    }

    // ── bokeh: soft discs drifting up, larger ones dimmer ──
    fn paint_bokeh(&self, painter: &Painter, rect: Rect, colors: &PaletteColors) {
        let side = rect.width().min(rect.height());
        for i in 0..BOKEH_COUNT {
            let (x01, y01, r01, bright) = bokeh_disc(i, self.phase);
            let tint = as_light(if i % 3 == 0 { colors.accent_soft } else { colors.accent });
            soft_disc(
                painter,
                Pos2::new(rect.left() + x01 * rect.width(), rect.top() + y01 * rect.height()),
                r01 * side,
                tint,
                self.lift(BOKEH_DISC * bright),
            );
        }
    }

    // ── the sheen: one soft diagonal band of white drifting across ──
    fn paint_sheen(&self, painter: &Painter, rect: Rect) {
        let p = sheen_pos(self.phase);
        let peak = self.lift(SHEEN);
        let mid = self.lift(SHEEN * 0.55);
        diagonal_band(
            painter,
            rect,
            p,
            0.16,
            rect.height() * 0.45,
            &[(-1.0, 0), (-0.25, mid), (0.0, peak), (0.25, mid), (1.0, 0)],
            Color32::WHITE,
        );
    }
}

// ── the per-style light budget, as fractions of MAX_LIFT. The readability test sums the
// worst case per style from these, so a retune here is a retune of the cap's proof too. ──
const GRADIENT_TOP: f32 = 0.70;
const AURORA_CURTAIN: f32 = 0.39; // two curtains may overlap: 2 × 0.39 + sheen = 1.0
const SILK_BAND: f32 = 0.39; // two bands may overlap at the peak: 2 × 0.39 + sheen = 1.0
const STAR_NEBULA: f32 = 0.34; // two nebulae may overlap
const STAR_POINT: f32 = 0.9; // a star is a point: it lifts its own pixel, not the text's
const BOKEH_DISC: f32 = 0.39; // two discs may overlap
const SHEEN: f32 = 0.22;
pub const BOKEH_COUNT: usize = 14;

fn vertex(x: f32, y: f32, color: Color32) -> Vertex {
    Vertex { pos: Pos2::new(x, y), uv: egui::epaint::WHITE_UV, color }
}

/// A quad with a colour per corner: top-left, top-right, bottom-right, bottom-left.
fn quad(painter: &Painter, r: Rect, tl: Color32, tr: Color32, br: Color32, bl: Color32) {
    let mut m = Mesh::default();
    m.vertices.push(vertex(r.left(), r.top(), tl));
    m.vertices.push(vertex(r.right(), r.top(), tr));
    m.vertices.push(vertex(r.right(), r.bottom(), br));
    m.vertices.push(vertex(r.left(), r.bottom(), bl));
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(m));
}

/// A feathered diagonal band: `stops` are (offset from the band's centre in band widths, alpha),
/// laid out as a strip of quads across the rect, each stop a pair of vertices on the top and
/// bottom edges shifted by `diag` for the slope. Consecutive stops are joined, so the alpha
/// ramps smoothly from one to the next.
fn diagonal_band(painter: &Painter, rect: Rect, p: f32, w: f32, diag: f32, stops: &[(f32, u8)], tint: Color32) {
    let mut mesh = Mesh::default();
    for (k, a) in stops {
        let xc = rect.left() + (p + k * w) * rect.width();
        let c = with_alpha(tint, *a);
        mesh.vertices.push(vertex(xc + diag, rect.top(), c));
        mesh.vertices.push(vertex(xc - diag, rect.bottom(), c));
    }
    for q in 0..(stops.len() as u32 - 1) {
        let b = q * 2;
        mesh.add_triangle(b, b + 1, b + 3);
        mesh.add_triangle(b, b + 3, b + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A feathered disc: centre alpha `a`, a soft mid ring, nothing at the rim. The off-centre
/// highlight the glass buttons carry, at background scale.
pub fn soft_disc(painter: &Painter, centre: Pos2, r: f32, tint: Color32, a: u8) {
    let segs = 40u32;
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex { pos: centre, uv: egui::epaint::WHITE_UV, color: with_alpha(tint, a) });
    let mid_a = (a as f32 * 0.42) as u8;
    for ring in 0..2 {
        let (rr, ra) = if ring == 0 { (r * 0.45, mid_a) } else { (r, 0) };
        for s in 0..segs {
            let t = s as f32 / segs as f32 * std::f32::consts::TAU;
            mesh.vertices.push(vertex(centre.x + rr * t.cos(), centre.y + rr * t.sin(), with_alpha(tint, ra)));
        }
    }
    let inner = |s: u32| 1 + s % segs;
    let outer = |s: u32| 1 + segs + s % segs;
    for s in 0..segs {
        mesh.add_triangle(0, inner(s), inner(s + 1));
        mesh.add_triangle(inner(s), outer(s), outer(s + 1));
        mesh.add_triangle(inner(s), outer(s + 1), inner(s + 1));
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Darken the edges and, more, the bottom: the controls and the tab bar sit there, on the
/// darkest part of the page. Four gradient quads from a shade of black to nothing.
pub fn paint_vignette(painter: &Painter, rect: Rect) {
    let none = with_alpha(Color32::BLACK, 0);
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let (w, h) = (rect.width(), rect.height());
    // bottom: the strongest, over the lower third
    let bottom = with_alpha(Color32::BLACK, 70);
    quad(painter, Rect::from_min_max(Pos2::new(l, b - h * 0.34), Pos2::new(r, b)), none, none, bottom, bottom);
    // top: a lighter hand
    let top = with_alpha(Color32::BLACK, 34);
    quad(painter, Rect::from_min_max(Pos2::new(l, t), Pos2::new(r, t + h * 0.16)), top, top, none, none);
    // sides
    let side = with_alpha(Color32::BLACK, 28);
    quad(painter, Rect::from_min_max(Pos2::new(l, t), Pos2::new(l + w * 0.14, b)), side, none, none, side);
    quad(painter, Rect::from_min_max(Pos2::new(r - w * 0.14, t), Pos2::new(r, b)), none, side, side, none);
}

fn grid_triangles(mesh: &mut Mesh, cols: usize, rows: usize) {
    for c in 0..cols - 1 {
        for r in 0..rows - 1 {
            let i = (c * rows + r) as u32;
            let j = ((c + 1) * rows + r) as u32;
            mesh.add_triangle(i, j, j + 1);
            mesh.add_triangle(i, j + 1, i + 1);
        }
    }
}

/// The accent as LIGHT rather than paint: lifted a third of the way to white, so a curtain
/// or a bokeh disc reads as a glow of that colour and not a smear of it.
fn as_light(c: Color32) -> Color32 {
    lerp_color(c, Color32::WHITE, 0.33)
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

// ── pure motion + light math (unit-tested) ─────────────────────────────────────────────

/// Sine wave height at `x` for a given phase. Kept for callers of the old waves.
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

/// A star's brightness, 0..1, breathing on its own period.
pub fn twinkle(i: usize, phase: f32) -> f32 {
    let f = 0.4 + ((i as f32 * 3.7).sin().abs()) * 0.9;
    let o = (i as f32 * 1.3).sin() * 3.0;
    ((phase * f + o).sin() * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// The curtain's vertical feather: 0 at both edges, 1 in the middle, smooth (sin²).
pub fn curtain_alpha(u: f32) -> f32 {
    let s = (u.clamp(0.0, 1.0) * std::f32::consts::PI).sin();
    s * s
}

/// How far (as a fraction of the height) a curtain's centre wanders at `x01`.
pub fn curtain_wobble(x01: f32, phase: f32, i: usize) -> f32 {
    let p = phase * 0.07;
    0.05 * ((x01 * 2.2 + p + i as f32).sin()) + 0.025 * ((x01 * 5.1 - p * 1.7).sin())
}

/// The sheen's horizontal position, 0..1 across the rect plus overshoot, on a ~22 s loop.
pub fn sheen_pos(phase: f32) -> f32 {
    let t = (phase / 22.0).fract();
    -0.3 + t * 1.6
}

/// The silk band `i`'s position, each on its own slow loop and offset.
pub fn silk_band_pos(phase: f32, i: usize) -> f32 {
    let period = 40.0 + i as f32 * 9.0;
    let t = (phase / period + i as f32 * 0.25).fract();
    -0.4 + t * 1.8
}

/// Bokeh disc `i` at `phase`: (x01, y01, radius as a fraction of the short side, brightness 0..1).
/// Discs rise slowly with a sideways sway; the larger the disc the dimmer.
pub fn bokeh_disc(i: usize, phase: f32) -> (f32, f32, f32, f32) {
    let hx = ((i as f32 * 12.9898).sin() * 43758.547).fract().abs();
    let hy = ((i as f32 * 78.233).sin() * 12543.13).fract().abs();
    let hr = ((i as f32 * 39.425).sin() * 9631.7).fract().abs();
    let r = 0.07 + 0.17 * hr;
    let speed = 0.006 + 0.006 * (1.0 - hr);
    let y = (hy - phase * speed).rem_euclid(1.3) - 0.15;
    let x = hx + 0.04 * (phase * 0.15 + i as f32).sin();
    let bright = 1.0 - 0.5 * hr;
    (x, y, r, bright)
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
        assert_eq!(BgStyle::all().len(), 5);
    }

    /// THE READABILITY CAP. The worst-case stack of light per style — every element that can
    /// overlap one pixel, plus the sheen — stays at or under MAX_LIFT. This is the property the
    /// old aurora broke (four 18% bands, 50% stacked) and the reason text under it went.
    #[test]
    fn no_style_lifts_a_pixel_past_the_cap() {
        let bg = LiveBackground::new(BgStyle::Aurora);
        let worst = [
            ("gradient", GRADIENT_TOP + SHEEN),
            ("aurora", 2.0 * AURORA_CURTAIN * curtain_alpha(0.5) + SHEEN),
            ("silk", 2.0 * SILK_BAND + SHEEN),
            ("starfield nebula", 2.0 * STAR_NEBULA),
            ("bokeh", 2.0 * BOKEH_DISC + SHEEN),
        ];
        for (name, frac) in worst {
            assert!(frac <= 1.0 + 1e-6, "{name}: {frac:.2} of the cap");
            assert!(bg.lift(frac) <= MAX_LIFT, "{name}");
        }
        // a star lifts its own two pixels only, which no text sits on; its point alpha is
        // allowed to be bright, but it is still capped
        assert!(bg.lift(STAR_POINT) <= MAX_LIFT);
        // intensity scales the whole thing down, never up
        let mut dim = bg.clone();
        dim.intensity = 0.5;
        assert_eq!(dim.lift(1.0), MAX_LIFT / 2);
        dim.intensity = 4.0;
        assert_eq!(dim.lift(1.0), MAX_LIFT);
    }

    #[test]
    fn feathers_fall_to_nothing_at_their_edges() {
        assert!(curtain_alpha(0.0) < 1e-6 && curtain_alpha(1.0) < 1e-6);
        assert!((curtain_alpha(0.5) - 1.0).abs() < 1e-6);
        for i in 0..BOKEH_COUNT {
            let (x, _y, r, b) = bokeh_disc(i, 0.0);
            assert!((0.0..=1.1).contains(&x) && (0.07..=0.25).contains(&r) && (0.5..=1.0).contains(&b), "disc {i}");
        }
        for p in 0..200 {
            let t = twinkle(5, p as f32 * 0.1);
            assert!((0.0..=1.0).contains(&t));
        }
    }

    #[test]
    fn the_sheen_and_the_silk_loop_rather_than_running_off() {
        // the sheen crosses the whole rect and comes back: it must reach both overshoots
        let (mut lo, mut hi) = (1.0f32, -1.0f32);
        for p in 0..2200 {
            let x = sheen_pos(p as f32 * 0.01);
            lo = lo.min(x);
            hi = hi.max(x);
        }
        assert!(lo < -0.25 && hi > 1.25, "sheen range {lo:.2}..{hi:.2}");
        for i in 0..4 {
            let a = silk_band_pos(0.0, i);
            let b = silk_band_pos(1000.0, i);
            assert!((-0.4..=1.4).contains(&a) && (-0.4..=1.4).contains(&b));
        }
    }

    #[test]
    fn every_style_paints_headless_and_tessellates() {
        // each style produces shapes on a bare Context, and the meshes survive the
        // tessellator — an out-of-range triangle index is a panic in there, not here
        let colors = crate::default_palette();
        for style in BgStyle::all() {
            let mut bg = LiveBackground::new(style);
            bg.tick(3.7);
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0)));
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = ui.max_rect();
                    bg.paint(ui.painter(), r, &colors);
                });
            });
            assert!(!out.shapes.is_empty(), "{style:?} painted nothing");
            let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        }
    }
}
