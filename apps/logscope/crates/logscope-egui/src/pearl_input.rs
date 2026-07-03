//! pearl_input.rs — the composer as a PEARL. The text the user types is rendered as a field of glowing
//! resonance cells (the same 5×7 font + starry-speckle look as the 3D pearls), laid over a real `egui::TextEdit`
//! that still owns input, the caret, selection, and IME (so the Plasma-Mobile focus fix keeps working).
//!
//! The hard requirement is TEMPORAL STABILITY: typing must not recompute the whole field every keystroke (that
//! flickers). We keep a persistent POOL of cells keyed by (slot, glyph-pixel). On each edit we DIFF the desired
//! cells against the pool — shared cells keep their position and twinkle phase, removed cells disperse and fade
//! out, new cells coalesce in from a small offset — so only the DELTA animates while the rest holds rock-steady.

use egui::{Color32, Pos2, Rect, Sense, Vec2};
use std::collections::{HashMap, HashSet};

type Key = (u16, u8, u8); // (char slot, glyph x, glyph y)

struct Cell {
    key: Key,
    home: Pos2,   // target screen position (from layout)
    pos: Pos2,    // current position (eases toward home)
    phase: f32,   // twinkle phase
    speed: f32,   // twinkle speed
    star: bool,   // a white-hot "star" cell
    life: f32,    // 0..1 current brightness/presence
    target: f32,  // 1 = should be present, 0 = fading out
    prov: bool,   // provisional: a live-transcription PREVIEW cell (rendered dimmer/cooler than committed text)
}

#[derive(Default)]
pub struct PearlInput {
    cells: Vec<Cell>,
    caret_t: f32, // smoothed caret intensity for the edit shimmer
    ripple_t: f32, // 1→0 expanding ring after a send
}

impl PearlInput {
    /// Trigger the send ripple (call when the message is committed) — a bright ring sweeps the pearl.
    pub fn pulse(&mut self) { self.ripple_t = 1.0; }

    /// Draw the pearl backdrop + the temporally-stable resonance text for `text`, in a band `height` tall.
    /// Returns the band rect (the caller overlays the real TextEdit on it). `t` is the animation clock.
    pub fn show(&mut self, ui: &mut egui::Ui, text: &str, preview: &str, t: f32, height: f32, accent: Color32) -> Rect {
        let avail = ui.available_width();
        let (rect, _resp) = ui.allocate_exact_size(Vec2::new(avail, height), Sense::hover());
        let p = ui.painter_at(rect);

        let cr = (height * 0.5).min(120.0) as u8;            // stadium ends → reads round, not a box
        let center = rect.center();
        let (ar, ag, ab) = (accent.r(), accent.g(), accent.b());
        // a soft radial-gradient blob (egui has no gradients/blur, so fake them by stacking faded circles)
        let glow = |c: Pos2, r0: f32, col: (u8, u8, u8), peak: f32, n: usize| {
            for i in 0..n {
                let f = i as f32 / n as f32;
                let a = (peak * (1.0 - f) * (1.0 - f) * 255.0) as u8;
                if a == 0 { continue; }
                p.circle_filled(c, r0 * (0.14 + 0.86 * f), Color32::from_rgba_unmultiplied(col.0, col.1, col.2, a));
            }
        };
        // ---- OUTER BLOOM — the pearl bleeds light outward (soft, edgeless; no UI border) ----
        for i in 0..8 {
            let f = i as f32 / 8.0;
            let grow = 1.0 + f * 30.0;
            let a = (18.0 * (1.0 - f) * (1.0 - f)) as u8;
            p.rect_filled(rect.expand(grow), egui::CornerRadius::same((cr as f32 + grow) as u8),
                          Color32::from_rgba_unmultiplied(ar, ag, ab, a));
        }
        // ---- TRANSLUCENT GLASSY BODY — you see INTO it (low alpha), not an opaque panel ----
        p.rect_filled(rect, egui::CornerRadius::same(cr), Color32::from_rgba_unmultiplied(5, 12, 18, 118));
        // subsurface interior light + a domed gloss (upper-left) + a counter-sheen → a rounded, lit form
        glow(Pos2::new(center.x, center.y + height * 0.16), rect.width() * 0.5, (16, 56, 74), 0.11, 9);
        glow(Pos2::new(rect.left() + rect.width() * 0.30, rect.top() + height * 0.30), rect.width() * 0.30, (205, 236, 255), 0.13, 10);
        glow(Pos2::new(rect.right() - rect.width() * 0.16, rect.bottom() - height * 0.20), rect.width() * 0.20, (ar, ag, ab), 0.08, 8);
        // ---- CRACKED-PEARL surface: a jittered, slowly-modulating triangulated facet web with NACRE-
        // iridescent edges (the chosen node design), so the composer reads as the same cracked pearl that
        // holds each turn in the 3D scene. Drawn faint, behind the resonance text. ----
        let nacre = |frac: f32, a: f32| -> Color32 {
            let ph = frac * std::f32::consts::TAU + t * 0.3;
            let r = (0.5 + 0.5 * (ph).cos()) * 255.0;
            let g = (0.5 + 0.5 * (ph + 2.094).cos()) * 255.0;
            let b = (0.5 + 0.5 * (ph + 4.188).cos()) * 255.0;
            Color32::from_rgba_unmultiplied(r as u8, g as u8, b as u8, (a * 255.0).clamp(0.0, 255.0) as u8)
        };
        let fcn = 16usize; let frn = 5usize; // facet grid cols × rows
        let inner = rect.shrink(height * 0.14);
        let cellw = inner.width() / fcn as f32; let cellh = inner.height() / frn as f32;
        let node = |i: usize, j: usize| -> Pos2 {
            let jx = (hash(i as i32 * 7 + j as i32 * 13, 21) - 0.5) * cellw * 0.5;
            let jy = (hash(i as i32 * 11 + j as i32 * 5, 22) - 0.5) * cellh * 0.5;
            let wob = (t * 0.5 + (i + j) as f32 * 0.7).sin() * 1.0; // slow surface modulation
            Pos2::new(inner.left() + i as f32 * cellw + jx + wob, inner.top() + j as f32 * cellh + jy)
        };
        // DOF: the cracked surface sits BEHIND the focal text, so draw it soft — each edge smeared across a
        // few offset passes (fake bokeh) and dim, so it reads as out-of-focus relief rather than crisp vector.
        let blur = [Vec2::new(0.0, 0.0), Vec2::new(1.6, 0.9), Vec2::new(-1.6, -0.9), Vec2::new(1.3, -1.1), Vec2::new(-1.1, 1.2)];
        for j in 0..frn {
            for i in 0..fcn {
                let frac = (j as f32 + 0.5) / frn as f32;
                let p00 = node(i, j);
                for (bi, off) in blur.iter().enumerate() {
                    let edge = nacre(frac, if bi == 0 { 0.12 } else { 0.045 });
                    let (a, b) = (*off, *off);
                    if i + 1 <= fcn { p.line_segment([p00 + a, node(i + 1, j) + b], egui::Stroke::new(0.9, edge)); }
                    if j + 1 <= frn { p.line_segment([p00 + a, node(i, j + 1) + b], egui::Stroke::new(0.9, edge)); }
                }
            }
        }
        // ---- send ripple: a bright nacre ring sweeping outward from centre after a commit ----
        if self.ripple_t > 0.001 {
            self.ripple_t *= 0.92;
            let cen = rect.center();
            let prog = 1.0 - self.ripple_t;
            let rad = prog * rect.width() * 0.6;
            let a = self.ripple_t * 0.7;
            p.circle_stroke(cen, rad, egui::Stroke::new(2.0, nacre(prog, a)));
            p.circle_stroke(cen, rad * 0.7, egui::Stroke::new(1.2, nacre(prog + 0.3, a * 0.7)));
        }


        // ---- FILM GRAIN — fine luminance noise so the surface reads as captured, not flat vector ----
        let gb = (t * 12.0) as i32; // reseed each ~1/12s so grain shimmers like film
        for k in 0..150 {
            let gx = rect.left() + hash(k, gb ^ 0x51) * rect.width();
            let gy = rect.top() + hash(k, gb ^ 0x77) * rect.height();
            let a = (5.0 + 11.0 * hash(k, gb ^ 0x33)) as u8;
            let col = if hash(k, gb ^ 0x9e) > 0.5 { Color32::from_rgba_unmultiplied(220, 235, 245, a) } else { Color32::from_rgba_unmultiplied(0, 0, 0, a) };
            p.circle_filled(Pos2::new(gx, gy), 0.8, col);
        }

        // ---- layout the desired text cells into slot homes (wraps to the box width) ----
        let scale = 3.0;                       // px per font pixel
        let cw = 6.0 * scale;                  // char advance
        let lh = 9.0 * scale;                  // line height
        let pad_x = 16.0; let pad_y = 12.0;
        let max_cols = (((rect.width() - 2.0 * pad_x) / cw).floor() as usize).max(1);
        let mut desired: HashMap<Key, Pos2> = HashMap::new();
        let mut prov_keys: HashSet<Key> = HashSet::new();
        let mut caret_home = Pos2::new(rect.left() + pad_x, rect.top() + pad_y + 3.0 * scale);
        // committed `text`, then the live-transcription `preview` (dimmer, separated by a space) — so the word
        // forming from speech appears on the pearl in real time before it's committed into the composer.
        let committed = text.chars().count();
        let mut combined: Vec<(char, bool)> = text.chars().map(|c| (c, false)).collect();
        if !preview.is_empty() {
            if !combined.is_empty() { combined.push((' ', true)); }
            combined.extend(preview.chars().map(|c| (c, true)));
        }
        for (i, (ch, prov)) in combined.into_iter().take(180).enumerate() {
            let col = i % max_cols; let row = i / max_cols;
            let ox = rect.left() + pad_x + col as f32 * cw;
            let oy = rect.top() + pad_y + row as f32 * lh;
            if i < committed { caret_home = Pos2::new(ox + cw, oy + 3.0 * scale); } // caret trails committed text only
            if ch == ' ' { continue; }
            let g = crate::mesh3d::glyph5x7(ch);
            for gx in 0..5usize {
                for gy in 0..7usize {
                    if (g[gx] >> gy) & 1 == 1 {
                        let key = (i as u16, gx as u8, gy as u8);
                        desired.insert(key, Pos2::new(ox + gx as f32 * scale, oy + gy as f32 * scale));
                        if prov { prov_keys.insert(key); }
                    }
                }
            }
        }

        // ---- diff against the pool: keep shared, fade out missing, spawn new (the temporal stability) ----
        let mut active: HashSet<Key> = HashSet::new();
        for c in &mut self.cells {
            if let Some(&home) = desired.get(&c.key) {
                c.home = home; c.target = 1.0; c.prov = prov_keys.contains(&c.key); active.insert(c.key);
            } else {
                c.target = 0.0; // disperse out, but keep animating until faded
            }
        }
        for (k, home) in &desired {
            if !active.contains(k) {
                // coalesce in from a small random offset around the home
                let off = Vec2::new((hash(k.0 as i32 * 7 + k.1 as i32, 11) - 0.5) * 14.0,
                                    (hash(k.0 as i32 * 13 + k.2 as i32, 12) - 0.5) * 14.0);
                self.cells.push(Cell {
                    key: *k, home: *home, pos: *home + off,
                    phase: hash(k.0 as i32 + k.1 as i32 * 31, 5) * std::f32::consts::TAU,
                    speed: 1.0 + 1.8 * hash(k.0 as i32 + k.2 as i32 * 17, 6),
                    star: hash(k.0 as i32 * 3 + k.1 as i32 + k.2 as i32, 9) > 0.74,
                    life: 0.0, target: 1.0, prov: prov_keys.contains(k),
                });
            }
        }

        // ---- integrate + render every cell — soft bloomy cores, lens CHROMATIC ABERRATION, non-uniform ----
        self.cells.retain_mut(|c| {
            c.life += (c.target - c.life) * 0.20;        // ease presence
            c.pos += (c.home - c.pos) * 0.28;            // ease toward home (smooth reflow on edits)
            if c.target == 0.0 && c.life < 0.03 { return false; }
            let tw = 0.5 + 0.5 * (t * c.speed + c.phase).sin();
            let uni = 0.72 + 0.55 * hash(c.key.0 as i32 * 5 + c.key.1 as i32, 8); // non-uniform size/brightness
            let bright = c.life * (0.40 + 0.60 * tw) * uni;
            // provisional (live-preview) cells read as a cooler, dimmer ghost until the utterance commits
            let (cre, cg, cb) = if c.prov { (198, 230, 255) } else if c.star { (236, 248, 255) } else { (140, 226, 255) };
            let a = (bright * 230.0 * if c.prov { 0.5 } else { 1.0 }).clamp(0.0, 255.0) as u8;
            let rad = scale * (if c.star { 0.62 } else { 0.5 }) * uni;
            // lens chromatic aberration: red/blue fringes that grow toward the edges of the frame
            let dx = ((c.pos.x - center.x).abs() * 0.010 + 0.5).min(2.3);
            let fr_a = (a as f32 * 0.28) as u8;
            p.circle_filled(Pos2::new(c.pos.x + dx, c.pos.y), rad, Color32::from_rgba_unmultiplied(255, 70, 70, fr_a));
            p.circle_filled(Pos2::new(c.pos.x - dx, c.pos.y), rad, Color32::from_rgba_unmultiplied(70, 100, 255, fr_a));
            // bloom halos (light bleed) → soft core, never a crisp pixel
            p.circle_filled(c.pos, rad * 3.2, Color32::from_rgba_unmultiplied(cre, cg, cb, (a as f32 * 0.09) as u8));
            p.circle_filled(c.pos, rad * 1.8, Color32::from_rgba_unmultiplied(cre, cg, cb, (a as f32 * 0.22) as u8));
            p.circle_filled(c.pos, rad, Color32::from_rgba_unmultiplied(cre, cg, cb, a));
            true
        });

        // ---- caret: a soft glowing bar (layered widths), not a crisp 1px line ----
        self.caret_t += (1.0 - self.caret_t) * 0.1;
        let cpu = 0.5 + 0.5 * (t * 4.0).sin();
        let cay = 70.0 + 80.0 * cpu;
        for w in [3.6f32, 2.0, 1.0] {
            let aa = (cay / w).min(255.0) as u8;
            p.line_segment([caret_home + Vec2::new(0.0, -5.0 * scale), caret_home + Vec2::new(0.0, 4.0 * scale)],
                           egui::Stroke::new(w, Color32::from_rgba_unmultiplied(ar, ag, ab, aa)));
        }

        // ---- VIGNETTE — darken the rounded ends + perimeter so the body falls off into the scene (no UI box
        // edge). Combined with the bloom + DOF this is what turns "a panel with text" into "a lit object". ----
        glow(Pos2::new(rect.left() + 2.0, center.y), height * 0.9, (0, 0, 0), 0.34, 7);
        glow(Pos2::new(rect.right() - 2.0, center.y), height * 0.9, (0, 0, 0), 0.34, 7);
        for s in 0..6 {
            let f = s as f32 / 6.0;
            let a = (34.0 * (1.0 - f)) as u8;
            p.rect_stroke(rect.shrink(1.0 + f * 4.0), egui::CornerRadius::same(cr), egui::Stroke::new(2.0, Color32::from_rgba_unmultiplied(0, 0, 0, a)), egui::StrokeKind::Inside);
        }

        // hint when empty
        if text.is_empty() {
            p.text(Pos2::new(rect.left() + pad_x, rect.center().y), egui::Align2::LEFT_CENTER,
                   "speak into the pearl…", egui::FontId::monospace(12.0),
                   Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 90));
        }
        rect
    }
}

#[inline]
fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841);
    h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 12;
    (h & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}
