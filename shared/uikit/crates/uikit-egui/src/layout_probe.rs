//! layout_probe — what a frame LOOKS like, read from its shapes, on a box
//! with no display.
//!
//! `chrome_mode.rs` in voiceterm-egui proved the technique: `Context::run`
//! lays out a full frame headlessly and the result can be measured. That
//! measured a panel's width. This measures the things a screenshot would
//! show wrong and a test never mentioned:
//!
//! * **text over text** — two galleys whose boxes intersect. Nearly always
//!   a collision: a label drawn under a badge, a count over a name.
//! * **clipped text** — a galley wider or taller than the clip rect it was
//!   painted under: the "…" egui does not draw for you, a name cut mid-
//!   letter by a column that lost the width fight.
//! * **off-screen text** — a galley outside the screen rect: a bar that
//!   wrapped under the bottom edge.
//! * **tiny text** — anything under [`MIN_READABLE_PT`] on a compact
//!   surface, the readability floor tokens.rs already names as FONT_XS.
//!
//! egui 0.34 keeps its widget rects crate-private, so this reads the
//! `FullOutput`'s `shapes` instead — which is better for the purpose: a
//! `Shape::Text` is what the person sees, whatever widget produced it.
//!
//! Not a screenshot. It cannot say the frame looks GOOD; it says which
//! text nobody can read. Run it at phone size and at desktop size — the
//! collisions differ — and assert `report.clean()` once a screen is.

use egui::epaint::{ClippedShape, Shape, TextShape};
use egui::{Pos2, Rect};

/// Below this, on a phone, a person leans in. FONT_XS in tokens.rs.
pub const MIN_READABLE_PT: f32 = 10.0;
/// Two galleys that intersect by less than this (points, each axis) are
/// touching, not colliding — a badge's corner kissing a label.
pub const OVERLAP_SLOP: f32 = 3.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub text: String,
    pub rect: Rect,
    pub clip: Rect,
    pub font_pt: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub screen: Rect,
    pub texts: usize,
    /// (a, b, intersection)
    pub overlaps: Vec<(Text, Text, Rect)>,
    /// Text cut on its LEFT/RIGHT by its clip, or hidden entirely:
    /// (text, points lost). A column that lost the width fight.
    pub clipped: Vec<(Text, f32)>,
    /// Text cut on its TOP/BOTTOM only — a scroll region's edge, almost
    /// always. Reported, not counted against `clean()`.
    pub cut_vertically: Vec<(Text, f32)>,
    pub offscreen: Vec<Text>,
    pub tiny: Vec<Text>,
    pub smallest_font_pt: f32,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.overlaps.is_empty() && self.clipped.is_empty() && self.offscreen.is_empty() && self.tiny.is_empty()
    }

    /// One line per finding, for a failing assertion's message.
    pub fn describe(&self) -> String {
        let mut out = Vec::new();
        for (a, b, r) in &self.overlaps {
            out.push(format!("overlap {:.0}×{:.0}: {:?} over {:?}", r.width(), r.height(), a.text, b.text));
        }
        for (t, by) in &self.clipped {
            out.push(format!("clipped by {by:.0}pt: {:?} ({:.0}×{:.0} in {:.0}×{:.0})", t.text, t.rect.width(), t.rect.height(), t.clip.width(), t.clip.height()));
        }
        for (t, by) in &self.cut_vertically {
            out.push(format!("(scroll edge) cut {by:.0}pt: {:?}", t.text));
        }
        for t in &self.offscreen {
            out.push(format!("off-screen: {:?} at {:?}", t.text, t.rect.min));
        }
        for t in &self.tiny {
            out.push(format!("tiny {:.1}pt: {:?}", t.font_pt, t.text));
        }
        if out.is_empty() { "clean".into() } else { out.join("\n") }
    }
}

/// Extract every text shape, positioned, from a frame's output.
pub fn texts(shapes: &[ClippedShape]) -> Vec<Text> {
    let mut out = Vec::new();
    for cs in shapes {
        collect(&cs.shape, cs.clip_rect, &mut out);
    }
    out
}

fn collect(shape: &Shape, clip: Rect, out: &mut Vec<Text>) {
    match shape {
        Shape::Text(TextShape { pos, galley, .. }) => {
            let text = galley.text().to_string();
            if text.trim().is_empty() { return; }
            let rect = galley.rect.translate(pos.to_vec2());
            let font_pt = galley.job.sections.first().map(|s| s.format.font_id.size).unwrap_or(0.0);
            out.push(Text { text, rect, clip, font_pt });
        }
        Shape::Vec(v) => for s in v { collect(s, clip, out); },
        _ => {}
    }
}

/// Analyse one frame's shapes.
pub fn analyse(screen: Rect, shapes: &[ClippedShape], compact: bool) -> Report {
    let ts = texts(shapes);
    let mut r = Report { screen, texts: ts.len(), overlaps: Vec::new(), clipped: Vec::new(), cut_vertically: Vec::new(), offscreen: Vec::new(), tiny: Vec::new(), smallest_font_pt: f32::MAX };
    for (i, a) in ts.iter().enumerate() {
        r.smallest_font_pt = r.smallest_font_pt.min(a.font_pt);
        if compact && a.font_pt < MIN_READABLE_PT { r.tiny.push(a.clone()); }
        // Clipped: the part of the galley outside its clip, worst side.
        let inter = a.rect.intersect(a.clip);
        if inter.is_negative() || inter.width() <= 0.0 || inter.height() <= 0.0 {
            // Entirely outside its clip: painted nowhere. Off-screen if
            // also outside the screen, else clipped away whole.
            if !screen.intersects(a.rect) { r.offscreen.push(a.clone()); }
            else { r.clipped.push((a.clone(), a.rect.width().max(a.rect.height()))); }
            continue;
        }
        let over_x = a.rect.width() - inter.width();
        let over_y = a.rect.height() - inter.height();
        if over_x > OVERLAP_SLOP { r.clipped.push((a.clone(), over_x)); }
        else if over_y > OVERLAP_SLOP { r.cut_vertically.push((a.clone(), over_y)); }
        if !screen.contains_rect(a.rect) && !screen.intersects(a.rect) { r.offscreen.push(a.clone()); }
        for b in ts.iter().skip(i + 1) {
            // Only the VISIBLE part of each text can collide: a row scrolled
            // half under a bar is clipped there, not drawn over the bar.
            let vis_b = b.rect.intersect(b.clip);
            if vis_b.is_negative() { continue; }
            let x = inter.intersect(vis_b);
            if x.is_negative() || x.width() <= OVERLAP_SLOP || x.height() <= OVERLAP_SLOP { continue; }
            // The same text drawn twice at one spot is a shadow/outline, not
            // a collision; text inside its own clip that another galley's
            // clip does not admit is not visible together either.
            if a.text == b.text && a.rect == b.rect { continue; }
            if !a.clip.intersects(b.clip) { continue; }
            r.overlaps.push((a.clone(), b.clone(), x));
        }
    }
    if r.smallest_font_pt == f32::MAX { r.smallest_font_pt = 0.0; }
    r
}

/// Run `draw` for `frames` frames at `size` and analyse the last one.
/// Several frames because egui sizes panels from the previous pass.
pub fn probe(size: egui::Vec2, frames: usize, compact: bool, mut draw: impl FnMut(&egui::Context)) -> Report {
    let ctx = egui::Context::default();
    let screen = Rect::from_min_size(Pos2::ZERO, size);
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(screen);
    let mut last = None;
    for _ in 0..frames.max(1) {
        let out = ctx.run(input.clone(), |ctx| draw(ctx));
        last = Some(out.shapes);
    }
    analyse(screen, &last.unwrap_or_default(), compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tidy_frame_is_clean_and_a_collision_is_named() {
        let clean = probe(egui::vec2(400.0, 300.0), 2, true, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("one");
                ui.label("two");
            });
        });
        assert!(clean.clean(), "{}", clean.describe());
        assert_eq!(clean.texts, 2);
        assert!(clean.smallest_font_pt >= MIN_READABLE_PT);

        let r = probe(egui::vec2(400.0, 300.0), 2, true, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = ui.painter();
                let font = egui::FontId::proportional(14.0);
                p.text(egui::pos2(20.0, 20.0), egui::Align2::LEFT_TOP, "name of a friend", font.clone(), egui::Color32::WHITE);
                p.text(egui::pos2(60.0, 24.0), egui::Align2::LEFT_TOP, "3", font, egui::Color32::WHITE);
            });
        });
        assert_eq!(r.overlaps.len(), 1, "{}", r.describe());
        assert!(r.describe().contains("over \"3\"") || r.describe().contains("\"3\" over"), "{}", r.describe());
    }

    #[test]
    fn text_past_its_clip_and_tiny_text_are_found() {
        let r = probe(egui::vec2(120.0, 200.0), 2, true, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // A 40-pt wide column holding a long single line: clipped on the right.
                let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(40.0, 30.0));
                ui.painter_at(rect).text(rect.min, egui::Align2::LEFT_TOP, "a considerably longer label than fits",
                    egui::FontId::proportional(14.0), egui::Color32::WHITE);
                ui.add_space(40.0);
                ui.label(egui::RichText::new("fine print").size(7.0));
            });
        });
        assert_eq!(r.clipped.len(), 1, "{}", r.describe());
        assert!(r.clipped[0].1 > 40.0);
        // A row cut by a scroll region's bottom edge is not a collision.
        let v = probe(egui::vec2(200.0, 100.0), 2, true, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(180.0, 8.0));
                ui.painter_at(rect).text(rect.min, egui::Align2::LEFT_TOP, "half a row",
                    egui::FontId::proportional(14.0), egui::Color32::WHITE);
            });
        });
        assert!(v.clipped.is_empty() && v.cut_vertically.len() == 1 && v.clean(), "{}", v.describe());
        assert_eq!(r.tiny.len(), 1, "{}", r.describe());
        assert!((r.smallest_font_pt - 7.0).abs() < 0.1);
        // Not compact: 7 pt is a desktop's business.
        let d = probe(egui::vec2(120.0, 200.0), 2, false, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { ui.label(egui::RichText::new("fine print").size(7.0)); });
        });
        assert!(d.tiny.is_empty());
    }

    #[test]
    fn the_same_text_painted_twice_at_one_spot_is_an_outline_not_a_collision() {
        let r = probe(egui::vec2(200.0, 100.0), 1, false, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = ui.painter();
                let f = egui::FontId::proportional(14.0);
                p.text(egui::pos2(10.0, 10.0), egui::Align2::LEFT_TOP, "hi", f.clone(), egui::Color32::BLACK);
                p.text(egui::pos2(10.0, 10.0), egui::Align2::LEFT_TOP, "hi", f, egui::Color32::WHITE);
            });
        });
        assert!(r.overlaps.is_empty(), "{}", r.describe());
    }
}
