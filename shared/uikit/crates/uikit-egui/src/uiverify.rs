//! uiverify — render a frame to PIXELS in a test, and assert things about the picture.
//!
//! `layout_probe` answers questions about SHAPES: do two labels overlap, is anything off
//! screen, did a widget get clipped. That catches a great deal and it cannot catch the class
//! of fault where every rect is in the right place and the result is still unreadable —
//! text painted the same colour as its background, a pane that draws nothing at all, a
//! surface that went blank after a state change. Those are the ones that survive a refactor
//! and get found by a person looking at a phone.
//!
//! This rasterises the frame and asks about the image.
//!
//! ## Properties, not golden images
//!
//! A stored PNG compared byte-for-byte fails on a font update, a renderer version, a
//! different antialiasing decision — and a suite that cries wolf gets its baselines
//! regenerated without being read, which is worse than no suite. So the assertions here are
//! PROPERTIES: is there ink, is there contrast, did this region change. A property survives
//! a font bump; a hash does not.
//!
//! ## What this rasteriser does NOT do
//!
//! Glyphs are drawn from a font atlas texture, and nothing here samples one — a text quad
//! fills with its vertex colour. So a line of text rasterises as a solid bar of the text's
//! colour rather than as letters.
//!
//! That is enough for "is anything there", "is it the right colour", "is this pane blank",
//! and "did this region change" — and it is NOT enough to check what the text says or how it
//! is kerned. `layout_probe::texts` already answers the first of those, and nothing here
//! should be read as checking the second. Stated plainly because a rasteriser that silently
//! approximates is one whose passes mean less than they appear to.

use egui::epaint::{ClippedShape, Color32, Rect, Vec2};

/// A rasterised frame: RGBA8, row-major, `w * h * 4`.
#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.w, self.h)
    }
}

impl Frame {
    pub fn get(&self, x: usize, y: usize) -> Color32 {
        let i = (y * self.w + x) * 4;
        Color32::from_rgba_unmultiplied(self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3])
    }

    /// Fraction of pixels that differ from the background colour.
    ///
    /// "Is anything drawn here", as a number. A pane that renders nothing scores 0, which is
    /// the check that catches a surface going blank — the failure `layout_probe` cannot see,
    /// because a pane that draws no shapes has no shapes to be wrong about.
    pub fn ink(&self, bg: Color32) -> f32 {
        self.ink_in(Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(self.w as f32, self.h as f32)), bg)
    }

    /// [`Frame::ink`] over one region, so a test can ask about the pane it cares about
    /// rather than the whole window.
    pub fn ink_in(&self, r: Rect, bg: Color32) -> f32 {
        let (x0, y0, x1, y1) = clamp_rect(r, self.w, self.h);
        let mut n = 0usize;
        let mut total = 0usize;
        for y in y0..y1 {
            for x in x0..x1 {
                total += 1;
                if !near(self.get(x, y), bg, 12) {
                    n += 1;
                }
            }
        }
        if total == 0 { 0.0 } else { n as f32 / total as f32 }
    }

    /// The most common colour in a region — the region's effective background.
    pub fn dominant(&self, r: Rect) -> Color32 {
        let (x0, y0, x1, y1) = clamp_rect(r, self.w, self.h);
        let mut counts: std::collections::HashMap<[u8; 4], usize> = Default::default();
        for y in y0..y1 {
            for x in x0..x1 {
                let c = self.get(x, y);
                *counts.entry([c.r(), c.g(), c.b(), c.a()]).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(c, _)| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]))
            .unwrap_or(Color32::TRANSPARENT)
    }

    /// WCAG relative-luminance contrast between the region's dominant colour and its ink.
    ///
    /// The number people quote: 1.0 is invisible, 4.5 is the AA threshold for body text.
    /// Computed against the region's OWN dominant colour rather than a theme value, because
    /// the fault being hunted is text that matches whatever is actually behind it — and a
    /// check that asked the theme would agree with the theme while the screen stayed
    /// unreadable.
    pub fn contrast_in(&self, r: Rect) -> f32 {
        let bg = self.dominant(r);
        let (x0, y0, x1, y1) = clamp_rect(r, self.w, self.h);
        // The ink colour furthest from the background is the text; averaging would be pulled
        // back toward the background by antialiased edges.
        let mut best = bg;
        let mut best_d = 0.0f32;
        for y in y0..y1 {
            for x in x0..x1 {
                let c = self.get(x, y);
                let d = (lum(c) - lum(bg)).abs();
                if d > best_d {
                    best_d = d;
                    best = c;
                }
            }
        }
        contrast(bg, best)
    }

    /// Fraction of pixels that differ between two frames of the same size.
    ///
    /// For "did pressing this change anything" and "did this region stay put" — the
    /// long-horizon check that a refactor left the screen alone.
    pub fn diff(&self, other: &Frame) -> f32 {
        if self.w != other.w || self.h != other.h {
            return 1.0;
        }
        let mut n = 0usize;
        for i in (0..self.px.len()).step_by(4) {
            let a = &self.px[i..i + 4];
            let b = &other.px[i..i + 4];
            if (0..4).any(|k| a[k].abs_diff(b[k]) > 8) {
                n += 1;
            }
        }
        n as f32 / (self.w * self.h) as f32
    }
}

fn clamp_rect(r: Rect, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let x0 = r.left().max(0.0) as usize;
    let y0 = r.top().max(0.0) as usize;
    let x1 = (r.right().max(0.0) as usize).min(w);
    let y1 = (r.bottom().max(0.0) as usize).min(h);
    (x0.min(x1), y0.min(y1), x1, y1)
}

fn near(a: Color32, b: Color32, tol: u8) -> bool {
    a.r().abs_diff(b.r()) <= tol && a.g().abs_diff(b.g()) <= tol && a.b().abs_diff(b.b()) <= tol
}

fn srgb_to_lin(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn lum(c: Color32) -> f32 {
    0.2126 * srgb_to_lin(c.r()) + 0.7152 * srgb_to_lin(c.g()) + 0.0722 * srgb_to_lin(c.b())
}

/// WCAG contrast ratio, 1.0 (identical) to 21.0 (black on white).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (l1, l2) = (lum(a), lum(b));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// Rasterise a frame's shapes.
///
/// Flat-shaded triangles with per-vertex colour interpolation and alpha blending. No texture
/// sampling — see the module docs for what that costs.
pub fn rasterize(shapes: Vec<ClippedShape>, size: Vec2, ppp: f32, bg: Color32) -> Frame {
    let (w, h) = (size.x.max(1.0) as usize, size.y.max(1.0) as usize);
    let mut px = vec![0u8; w * h * 4];
    for i in (0..px.len()).step_by(4) {
        px[i] = bg.r();
        px[i + 1] = bg.g();
        px[i + 2] = bg.b();
        px[i + 3] = bg.a();
    }
    let mut f = Frame { w, h, px };
    let mut tess = egui::epaint::Tessellator::new(
        ppp,
        egui::epaint::TessellationOptions::default(),
        [1024, 1024],
        vec![],
    );
    for prim in tess.tessellate_shapes(shapes) {
        if let egui::epaint::Primitive::Mesh(m) = prim.primitive {
            for tri in m.indices.chunks_exact(3) {
                let v = [
                    &m.vertices[tri[0] as usize],
                    &m.vertices[tri[1] as usize],
                    &m.vertices[tri[2] as usize],
                ];
                fill_tri(&mut f, v, prim.clip_rect);
            }
        }
    }
    f
}

/// Barycentric fill of one triangle, clipped to `clip`.
fn fill_tri(f: &mut Frame, v: [&egui::epaint::Vertex; 3], clip: Rect) {
    let p: Vec<(f32, f32)> = v.iter().map(|x| (x.pos.x, x.pos.y)).collect();
    let minx = p.iter().map(|q| q.0).fold(f32::MAX, f32::min).max(clip.left()).max(0.0);
    let maxx = p.iter().map(|q| q.0).fold(f32::MIN, f32::max).min(clip.right()).min(f.w as f32);
    let miny = p.iter().map(|q| q.1).fold(f32::MAX, f32::min).max(clip.top()).max(0.0);
    let maxy = p.iter().map(|q| q.1).fold(f32::MIN, f32::max).min(clip.bottom()).min(f.h as f32);
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 {
        return;
    }
    for y in (miny as usize)..(maxy.ceil() as usize).min(f.h) {
        for x in (minx as usize)..(maxx.ceil() as usize).min(f.w) {
            let q = (x as f32 + 0.5, y as f32 + 0.5);
            let (w0, w1, w2) = (edge(p[1], p[2], q), edge(p[2], p[0], q), edge(p[0], p[1], q));
            // Either winding: egui is not consistent about it, and rejecting one order would
            // silently drop half the geometry.
            let inside = (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
            if !inside {
                continue;
            }
            let (b0, b1, b2) = (w0 / area, w1 / area, w2 / area);
            let col = |g: fn(&egui::epaint::Vertex) -> u8| -> f32 {
                b0 * g(v[0]) as f32 + b1 * g(v[1]) as f32 + b2 * g(v[2]) as f32
            };
            let (r, g, b, a) = (
                col(|v| v.color.r()),
                col(|v| v.color.g()),
                col(|v| v.color.b()),
                col(|v| v.color.a()),
            );
            blend(f, x, y, r, g, b, a / 255.0);
        }
    }
}

fn edge(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn blend(f: &mut Frame, x: usize, y: usize, r: f32, g: f32, b: f32, a: f32) {
    let i = (y * f.w + x) * 4;
    let a = a.clamp(0.0, 1.0);
    for (k, v) in [r, g, b].into_iter().enumerate() {
        let dst = f.px[i + k] as f32;
        f.px[i + k] = (v * a + dst * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    }
    f.px[i + 3] = 255;
}

/// Draw a frame and rasterise it — the whole harness in one call.
pub fn render(size: Vec2, bg: Color32, mut draw: impl FnMut(&egui::Context)) -> Frame {
    let ctx = egui::Context::default();
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
        ..Default::default()
    };
    // Two passes: the first lays out, the second draws against real rects. A one-pass render
    // of anything that measures itself is a picture of the first frame's guesses.
    let _ = ctx.run(input.clone(), &mut draw);
    input.screen_rect = Some(Rect::from_min_size(egui::pos2(0.0, 0.0), size));
    let out = ctx.run(input, &mut draw);
    rasterize(out.shapes, size, 1.0, bg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: Color32 = Color32::from_rgb(20, 20, 24);

    /// The fault `layout_probe` cannot see: a pane that draws nothing has no shapes to be
    /// wrong about, so a shape-level check passes on a blank screen.
    #[test]
    fn a_blank_pane_is_visible_as_blank() {
        let empty = render(Vec2::new(120.0, 80.0), BG, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |_ui| {});
        });
        assert!(empty.ink(BG) < 0.01, "an empty pane had ink: {}", empty.ink(BG));

        let drawn = render(Vec2::new(120.0, 80.0), BG, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                ui.painter().rect_filled(
                    Rect::from_min_size(egui::pos2(10.0, 10.0), Vec2::new(60.0, 40.0)),
                    0.0,
                    Color32::from_rgb(200, 60, 60),
                );
            });
        });
        assert!(drawn.ink(BG) > 0.1, "a filled rect left no ink: {}", drawn.ink(BG));
    }

    /// Text the same colour as what is behind it: every rect in the right place, and nothing
    /// readable. A shape probe cannot see this, and a theme-based check would agree with the
    /// theme while the screen stayed unreadable — so the contrast is measured against the
    /// region's OWN dominant colour.
    #[test]
    fn text_that_matches_its_background_is_caught() {
        let mk = |fg: Color32| {
            render(Vec2::new(160.0, 60.0), BG, move |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                    ui.painter().rect_filled(
                        Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(160.0, 60.0)),
                        0.0,
                        BG,
                    );
                    ui.painter().rect_filled(
                        Rect::from_min_size(egui::pos2(20.0, 20.0), Vec2::new(100.0, 20.0)),
                        0.0,
                        fg,
                    );
                });
            })
        };
        let whole = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(160.0, 60.0));

        let readable = mk(Color32::from_rgb(240, 240, 240));
        assert!(readable.contrast_in(whole) > 4.5, "{}", readable.contrast_in(whole));

        // The same layout, invisible.
        let invisible = mk(BG);
        assert!(
            invisible.contrast_in(whole) < 1.5,
            "background-on-background scored {}",
            invisible.contrast_in(whole)
        );
    }

    /// "Did pressing this change anything", and its long-horizon twin, "did the refactor
    /// leave the screen alone".
    #[test]
    fn a_diff_sees_a_change_and_ignores_a_redraw() {
        let draw = |c: Color32| {
            render(Vec2::new(80.0, 40.0), BG, move |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                    ui.painter().rect_filled(
                        Rect::from_min_size(egui::pos2(5.0, 5.0), Vec2::new(30.0, 20.0)),
                        0.0,
                        c,
                    );
                });
            })
        };
        let a = draw(Color32::from_rgb(200, 60, 60));
        let again = draw(Color32::from_rgb(200, 60, 60));
        assert!(a.diff(&again) < 0.001, "the same frame twice differed: {}", a.diff(&again));

        let b = draw(Color32::from_rgb(60, 200, 60));
        assert!(a.diff(&b) > 0.05, "a colour change went unseen: {}", a.diff(&b));
    }

    /// A region is what a test actually cares about — one pane, not the window.
    #[test]
    fn ink_can_be_asked_about_one_region() {
        let f = render(Vec2::new(100.0, 100.0), BG, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                ui.painter().rect_filled(
                    Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(100.0, 50.0)),
                    0.0,
                    Color32::from_rgb(200, 200, 200),
                );
            });
        });
        let top = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(100.0, 50.0));
        let bottom = Rect::from_min_size(egui::pos2(0.0, 50.0), Vec2::new(100.0, 50.0));
        assert!(f.ink_in(top, BG) > 0.9, "{}", f.ink_in(top, BG));
        assert!(f.ink_in(bottom, BG) < 0.05, "{}", f.ink_in(bottom, BG));
    }

    /// The numbers people quote, so a threshold in a test means what a designer thinks it
    /// means.
    #[test]
    fn contrast_is_the_wcag_ratio() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.1);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-3);
        // Mid grey on white is around the AA line for large text.
        let c = contrast(Color32::from_gray(128), Color32::WHITE);
        assert!(c > 3.5 && c < 4.6, "{c}");
    }
}
