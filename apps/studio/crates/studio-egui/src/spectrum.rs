//! spectrum — the shared canvas every module draws into.
//!
//! This is the one structural idea worth taking from Ozone: an EQ curve, a
//! multiband crossover, a compressor threshold and the live signal all share
//! **one frequency axis**, so you read them together instead of correlating
//! four separate boxes in your head. Everything else in that interface follows
//! from it — you drag nodes and crossovers *on* the spectrum because they are
//! already there.
//!
//! The look is unendlich's rather than iZotope's: a lit volume with fog and
//! ambient grading rather than flat panels in chrome. Borrowing the grammar and
//! not the skin is what makes this a successor instead of a clone.
//!
//! All the mapping arithmetic is pure and tested here, because an axis that is
//! subtly wrong is the kind of bug you never quite see — you just stop trusting
//! the display, which defeats the entire purpose of putting everything on it.

use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};
use uikit_egui::PaletteColors;

/// The visible frequency and level window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axes {
    pub f_min: f32,
    pub f_max: f32,
    pub db_min: f32,
    pub db_max: f32,
}

impl Default for Axes {
    /// 20 Hz to 20 kHz, −72 to +12 dB: the window every mastering display uses,
    /// because it is the range human hearing and headroom actually occupy.
    fn default() -> Self {
        Axes { f_min: 20.0, f_max: 20_000.0, db_min: -72.0, db_max: 12.0 }
    }
}

impl Axes {
    /// Frequency → normalised 0..1 across the width, **logarithmically**.
    ///
    /// Linear frequency is the classic mistake: it gives half the display to
    /// 10–20 kHz, where almost nothing musical happens, and squashes the two
    /// octaves that carry a mix into a few pixels.
    pub fn f_to_norm(&self, hz: f32) -> f32 {
        let (lo, hi) = (self.f_min.max(1.0).ln(), self.f_max.max(2.0).ln());
        ((hz.max(1.0).ln() - lo) / (hi - lo)).clamp(0.0, 1.0)
    }

    pub fn norm_to_f(&self, n: f32) -> f32 {
        let (lo, hi) = (self.f_min.max(1.0).ln(), self.f_max.max(2.0).ln());
        (lo + n.clamp(0.0, 1.0) * (hi - lo)).exp()
    }

    /// dB → normalised 0..1, where 0 is the TOP (screen coordinates).
    pub fn db_to_norm(&self, db: f32) -> f32 {
        ((self.db_max - db) / (self.db_max - self.db_min)).clamp(0.0, 1.0)
    }

    pub fn norm_to_db(&self, n: f32) -> f32 {
        self.db_max - n.clamp(0.0, 1.0) * (self.db_max - self.db_min)
    }

    pub fn pos(&self, rect: &Rect, hz: f32, db: f32) -> Pos2 {
        Pos2::new(rect.min.x + self.f_to_norm(hz) * rect.width(),
                  rect.min.y + self.db_to_norm(db) * rect.height())
    }

    pub fn from_pos(&self, rect: &Rect, p: Pos2) -> (f32, f32) {
        let n = ((p.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
        let m = ((p.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
        (self.norm_to_f(n), self.norm_to_db(m))
    }
}

/// Octave gridlines worth labelling. Decade marks plus the 2/5 subdivisions —
/// enough to read a frequency off the display without so many lines that the
/// spectrum disappears behind them.
pub const GRID_HZ: [f32; 13] = [
    20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
    10_000.0, 20_000.0, 30.0, 300.0, 3000.0,
];

/// A draggable EQ node on the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
    pub enabled: bool,
}

/// Which node is under a pointer, if any.
///
/// Searched nearest-first in SCREEN space rather than frequency space, because
/// two nodes an octave apart down at 40 Hz are close on screen while two an
/// octave apart at 10 kHz are far — grabbing by frequency distance picks the
/// wrong one exactly where nodes are most crowded.
pub fn node_at(nodes: &[Node], axes: &Axes, rect: &Rect, p: Pos2, radius: f32) -> Option<usize> {
    nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let d = (axes.pos(rect, n.hz, n.gain_db) - p).length();
            (d <= radius).then_some((d, i))
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|(_, i)| i)
}

/// Move a crossover handle, keeping the band order and a minimum width.
///
/// Crossovers that can cross each other produce bands of negative width and a
/// multiband that silently drops audio, so they are clamped against their
/// neighbours — and the clamp is in **octaves**, not Hz, because 200 Hz of
/// separation is generous at 400 Hz and invisible at 12 kHz.
pub fn move_crossover(crossovers: &mut [f32], i: usize, to_hz: f32, min_octaves: f32) -> f32 {
    if i >= crossovers.len() { return to_hz; }
    let ratio = 2f32.powf(min_octaves.max(0.05));
    let lo = if i == 0 { 20.0 } else { crossovers[i - 1] * ratio };
    let hi = if i + 1 == crossovers.len() { 20_000.0 } else { crossovers[i + 1] / ratio };
    let clamped = to_hz.clamp(lo.min(hi), hi.max(lo));
    crossovers[i] = clamped;
    clamped
}

/// Sample a spectrum into one value per pixel column, in dB.
///
/// Averaging in the **power** domain per column, not the dB domain: a column
/// covering many bins at high frequency would otherwise under-read whenever one
/// bin is hot, which is precisely the partial you are looking at.
pub fn columns(spec: &studio_fx::analysis::Spectrum, axes: &Axes, width: usize) -> Vec<f32> {
    (0..width.max(1))
        .map(|x| {
            let n0 = x as f32 / width.max(1) as f32;
            let n1 = (x + 1) as f32 / width.max(1) as f32;
            spec.band_db(axes.norm_to_f(n0), axes.norm_to_f(n1).max(axes.norm_to_f(n0) * 1.001))
        })
        .collect()
}

/// Draw the canvas: unendlich's lit volume, then the grid, then the signal.
///
/// Returns the rect so callers can paint their own overlays into the same
/// space — which is the entire point of the shared canvas.
pub fn canvas(
    ui: &mut Ui,
    axes: &Axes,
    colors: &PaletteColors,
    height: f32,
    spec: Option<&studio_fx::analysis::Spectrum>,
) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), egui::Sense::hover());
    let p = ui.painter_at(rect);

    // ── the lit volume ──────────────────────────────────────────────────────
    // unendlich's atmosphere rather than a flat panel: a vertical fog gradient
    // with the "sun" low and warm, so the display has depth and the spectrum
    // sits IN something instead of on top of it.
    let steps = 24;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let band = Rect::from_min_max(
            Pos2::new(rect.min.x, rect.min.y + t * rect.height()),
            Pos2::new(rect.max.x, rect.min.y + (t + 1.0 / steps as f32) * rect.height()),
        );
        // darker at the top, warming toward the floor where the "sun" sits
        let warm = (t * t) * 0.35;
        let c = Color32::from_rgb(
            (colors.bg_base.r() as f32 * (0.55 + warm)) as u8,
            (colors.bg_base.g() as f32 * (0.55 + warm * 0.8)) as u8,
            (colors.bg_base.b() as f32 * (0.62 + warm * 0.4)) as u8,
        );
        p.rect_filled(band, 0.0, c);
    }

    // ── grid ────────────────────────────────────────────────────────────────
    for hz in GRID_HZ {
        if hz < axes.f_min || hz > axes.f_max { continue; }
        let x = axes.pos(&rect, hz, 0.0).x;
        p.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
                       Stroke::new(1.0, colors.border.gamma_multiply(0.35)));
        let label = if hz >= 1000.0 { format!("{:.0}k", hz / 1000.0) } else { format!("{hz:.0}") };
        p.text(Pos2::new(x + 3.0, rect.max.y - 13.0), egui::Align2::LEFT_TOP,
               label, egui::FontId::monospace(9.0), colors.text_weak.gamma_multiply(0.8));
    }
    for db in [0.0f32, -12.0, -24.0, -36.0, -48.0, -60.0] {
        if db < axes.db_min || db > axes.db_max { continue; }
        let y = axes.pos(&rect, axes.f_min, db).y;
        p.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
                       Stroke::new(1.0, colors.border.gamma_multiply(if db == 0.0 { 0.6 } else { 0.22 })));
    }

    // ── the signal ──────────────────────────────────────────────────────────
    if let Some(s) = spec {
        let w = rect.width() as usize;
        let cols = columns(s, axes, w);
        // filled to the floor, so the spectrum reads as a mass in the volume
        // rather than a thin line floating in it
        for (i, db) in cols.iter().enumerate() {
            let x = rect.min.x + i as f32;
            let y = rect.min.y + axes.db_to_norm(*db) * rect.height();
            let t = 1.0 - axes.db_to_norm(*db);
            p.line_segment([Pos2::new(x, rect.max.y), Pos2::new(x, y)],
                           Stroke::new(1.0, colors.accent.gamma_multiply(0.18 + t * 0.5)));
        }
    }
    rect
}

/// Draw an EQ curve and its nodes over the canvas.
pub fn eq_overlay(
    ui: &Ui,
    rect: &Rect,
    axes: &Axes,
    eq: &studio_fx::Eq,
    rate: f32,
    nodes: &[Node],
    selected: Option<usize>,
    colors: &PaletteColors,
) {
    let p = ui.painter_at(*rect);
    let w = rect.width().max(1.0) as usize;
    let mut pts = Vec::with_capacity(w);
    for x in 0..w {
        let hz = axes.norm_to_f(x as f32 / w as f32);
        let db = eq.magnitude_db(rate, hz);
        pts.push(axes.pos(rect, hz, db));
    }
    for pair in pts.windows(2) {
        p.line_segment([pair[0], pair[1]], Stroke::new(2.0, colors.accent));
    }
    for (i, n) in nodes.iter().enumerate() {
        let c = axes.pos(rect, n.hz, n.gain_db);
        let on = Some(i) == selected;
        let col = if n.enabled { colors.accent } else { colors.text_weak };
        p.circle_filled(c, if on { 7.0 } else { 5.0 }, col);
        p.circle_stroke(c, if on { 7.0 } else { 5.0 }, Stroke::new(1.5, colors.bg_base));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect { Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(800.0, 300.0)) }

    #[test]
    fn the_frequency_axis_is_logarithmic_and_round_trips() {
        // Linear frequency is the classic mistake: it gives half the display to
        // 10-20 kHz where almost nothing musical happens, and squashes the two
        // octaves that carry a mix into a few pixels.
        let a = Axes::default();
        for hz in [20.0f32, 100.0, 440.0, 1000.0, 5000.0, 20_000.0] {
            let back = a.norm_to_f(a.f_to_norm(hz));
            assert!((back / hz - 1.0).abs() < 0.01, "{hz} Hz round-tripped to {back}");
        }
        // equal octaves must occupy equal width — the definition of log spacing
        let oct1 = a.f_to_norm(200.0) - a.f_to_norm(100.0);
        let oct2 = a.f_to_norm(8000.0) - a.f_to_norm(4000.0);
        assert!((oct1 - oct2).abs() < 1e-4, "octaves must be equal width: {oct1} vs {oct2}");
        // and 1 kHz sits past the middle, as it does on every real analyser
        assert!(a.f_to_norm(1000.0) > 0.5);
    }

    #[test]
    fn the_level_axis_puts_loud_at_the_top() {
        let a = Axes::default();
        assert!(a.db_to_norm(a.db_max) < a.db_to_norm(a.db_min), "0 is the top in screen space");
        assert!((a.norm_to_db(a.db_to_norm(-24.0)) + 24.0).abs() < 1e-3);
        // out-of-range values clamp instead of drawing off-canvas
        assert_eq!(a.db_to_norm(999.0), 0.0);
        assert_eq!(a.db_to_norm(-999.0), 1.0);
    }

    #[test]
    fn a_point_maps_to_the_canvas_and_back() {
        let (a, r) = (Axes::default(), rect());
        for (hz, db) in [(100.0f32, -12.0f32), (1000.0, 0.0), (8000.0, -40.0)] {
            let p = a.pos(&r, hz, db);
            assert!(r.contains(p), "({hz},{db}) landed outside the canvas");
            let (bh, bd) = a.from_pos(&r, p);
            assert!((bh / hz - 1.0).abs() < 0.02, "{hz} -> {bh}");
            assert!((bd - db).abs() < 0.5, "{db} -> {bd}");
        }
    }

    #[test]
    fn grabbing_a_node_picks_the_nearest_on_screen_not_in_frequency() {
        // Two nodes an octave apart are CLOSE on screen at 40 Hz and FAR at
        // 10 kHz. Searching by frequency distance picks the wrong one exactly
        // where nodes are most crowded.
        let (a, r) = (Axes::default(), rect());
        let nodes = vec![
            Node { hz: 60.0, gain_db: 0.0, q: 1.0, enabled: true },
            Node { hz: 120.0, gain_db: 0.0, q: 1.0, enabled: true },
            Node { hz: 10_000.0, gain_db: 0.0, q: 1.0, enabled: true },
        ];
        // right on the second node
        let p = a.pos(&r, 120.0, 0.0);
        assert_eq!(node_at(&nodes, &a, &r, p, 12.0), Some(1));
        // nearer the first than the second, in SCREEN space
        let between = a.pos(&r, 70.0, 0.0);
        assert_eq!(node_at(&nodes, &a, &r, between, 40.0), Some(0));
        // nowhere near anything
        assert_eq!(node_at(&nodes, &a, &r, a.pos(&r, 2000.0, -50.0), 12.0), None);
    }

    #[test]
    fn crossovers_cannot_cross_and_keep_a_musical_minimum_width() {
        // Crossovers that pass each other produce bands of negative width and a
        // multiband that silently drops audio.
        let mut x = vec![200.0f32, 1000.0, 5000.0];
        // shove the middle one past its upper neighbour
        let got = move_crossover(&mut x, 1, 99_000.0, 0.5);
        assert!(got < 5000.0, "clamped below the next crossover, got {got}");
        assert!(x[0] < x[1] && x[1] < x[2], "order preserved: {x:?}");
        // and past its lower one
        let got = move_crossover(&mut x, 1, 1.0, 0.5);
        assert!(got > 200.0, "clamped above the previous crossover, got {got}");
        // the clamp is in OCTAVES, so the gap it enforces scales with frequency
        let mut low = vec![100.0f32, 200.0];
        let mut high = vec![5000.0f32, 10_000.0];
        move_crossover(&mut low, 1, 0.0, 1.0);
        move_crossover(&mut high, 1, 0.0, 1.0);
        let gap_low = low[1] - low[0];
        let gap_high = high[1] - high[0];
        assert!(gap_high > gap_low * 10.0,
                "an octave is worth more Hz up high: {gap_low} vs {gap_high}");
        // an out-of-range index is ignored rather than panicking
        let mut one = vec![1000.0f32];
        assert_eq!(move_crossover(&mut one, 9, 500.0, 0.5), 500.0);
    }

    #[test]
    fn columns_cover_the_axis_without_gaps_or_overlap() {
        use studio_fx::analysis::Spectrum;
        let n = 4096;
        let block: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin() * 0.5)
            .collect();
        let s = Spectrum::analyze(&block, 48_000.0);
        let a = Axes::default();
        let cols = columns(&s, &a, 600);
        assert_eq!(cols.len(), 600, "one value per pixel column");
        assert!(cols.iter().all(|v| v.is_finite()), "no NaN columns");
        // the 1 kHz tone must show up in the column that covers 1 kHz
        let at_1k = (a.f_to_norm(1000.0) * 600.0) as usize;
        let near = cols[at_1k.saturating_sub(2)..(at_1k + 3).min(600)]
            .iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let far = cols[(a.f_to_norm(120.0) * 600.0) as usize];
        assert!(near > far + 30.0, "the tone should dominate its column: {near} vs {far}");
    }
}
