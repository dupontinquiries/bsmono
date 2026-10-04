//! track_points — put the track points where they belong, by hand.
//!
//! The automatic pick takes the most textured patches near the emitter, and
//! texture is not the same as subject: beside a patterned poster it rode the
//! poster while the figure walked away (the touch-up test that exists for
//! this). So the operator can place the points: tap the first frame to add
//! one, tap an existing point to select it rather than stack a duplicate, drag
//! to move the one under the finger, and remove with a visible button --
//! a long press would be the convention, but nobody finds it.

/// Points as fractions of the frame, and which is selected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Points { pub pts: Vec<(f32, f32)>, pub selected: Option<usize> }

/// How close (fraction of the frame width) a tap must be to count as ON a point.
pub const GRAB: f32 = 0.04;

impl Points {
    fn nearest(&self, p: (f32, f32)) -> Option<usize> {
        self.pts.iter().enumerate()
            .map(|(i, q)| (i, (q.0 - p.0).hypot(q.1 - p.1)))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
    }
    /// A tap: on a point selects it, elsewhere adds one (and selects it).
    pub fn tap(&mut self, p: (f32, f32)) {
        let p = (p.0.clamp(0.0, 1.0), p.1.clamp(0.0, 1.0));
        match self.nearest(p) {
            Some(i) => self.selected = Some(i),
            None => { self.pts.push(p); self.selected = Some(self.pts.len() - 1); }
        }
    }
    /// A drag that started at `from`: moves the point under the finger by `d`,
    /// kept inside the frame. A drag that starts on empty frame moves nothing
    /// (it is not a way to add points by accident).
    pub fn drag(&mut self, from: (f32, f32), d: (f32, f32)) -> Option<usize> {
        let i = self.nearest(from)?;
        let q = &mut self.pts[i];
        *q = ((q.0 + d.0).clamp(0.0, 1.0), (q.1 + d.1).clamp(0.0, 1.0));
        self.selected = Some(i);
        Some(i)
    }
    pub fn remove_selected(&mut self) {
        if let Some(i) = self.selected.take() { if i < self.pts.len() { self.pts.remove(i); } }
    }
    pub fn clear(&mut self) { self.pts.clear(); self.selected = None; }
    /// The rig needs at least two points to solve a rotation.
    pub fn enough(&self) -> bool { self.pts.len() >= 2 }
}

/// The window: the clip's first rendered frame, the points over it.
pub struct Window {
    pub frame: Vec<u8>,
    pub points: Points,
    pub preset: u8,
    pub drive: Option<studio_core::BusId>,
    texture: Option<egui::TextureHandle>,
    drag_from: Option<(f32, f32)>,
}

impl Window {
    pub fn new(frame: Vec<u8>, preset: u8, drive: Option<studio_core::BusId>) -> Window {
        Window { frame, points: Points::default(), preset, drive, texture: None, drag_from: None }
    }

    /// Draw; returns the points when "ride these points" is pressed.
    pub fn show(&mut self, ui: &mut egui::Ui, layout: studio_egui::touch::ToolLayout) -> Option<Vec<(f32, f32)>> {
        layout.apply(ui);
        let tex = self.texture.get_or_insert_with(|| ui.ctx().load_texture("track-points-frame",
            egui::ColorImage::from_rgb([320, 180], &self.frame), egui::TextureOptions::LINEAR)).clone();
        ui.label("put points on what the particles should follow — on the person, not the wall behind them");
        let w = (ui.available_width() - 8.0).min(640.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, w * 9.0 / 16.0), egui::Sense::click_and_drag());
        let to_frac = |p: egui::Pos2| ((p.x - rect.min.x) / rect.width(), (p.y - rect.min.y) / rect.height());
        if resp.drag_started() { self.drag_from = resp.interact_pointer_pos().map(to_frac); }
        if resp.dragged() {
            if let Some(from) = self.drag_from {
                let d = resp.drag_delta();
                let d = (d.x / rect.width(), d.y / rect.height());
                if self.points.drag(from, d).is_some() { self.drag_from = Some((from.0 + d.0, from.1 + d.1)); }
            }
        }
        if resp.drag_stopped() { self.drag_from = None; }
        if resp.clicked() { if let Some(p) = resp.interact_pointer_pos() { self.points.tap(to_frac(p)); } }
        let painter = ui.painter_at(rect);
        painter.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        // big enough to see under a fingertip; the selected one ringed
        let r = (layout.target * 0.25).max(6.0);
        for (i, p) in self.points.pts.iter().enumerate() {
            let c = egui::pos2(rect.min.x + p.0 * rect.width(), rect.min.y + p.1 * rect.height());
            painter.circle_filled(c, r, egui::Color32::from_rgb(255, 60, 180));
            if self.points.selected == Some(i) { painter.circle_stroke(c, r + 4.0, egui::Stroke::new(2.0_f32, egui::Color32::WHITE)); }
        }
        ui.small("tap to add · tap a point to select it · drag a point to move it");
        let mut go = None;
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(self.points.selected.is_some(), egui::Button::new("remove selected")).clicked() { self.points.remove_selected(); }
            if ui.add_enabled(!self.points.pts.is_empty(), egui::Button::new("clear")).clicked() { self.points.clear(); }
            ui.label(format!("{} point(s)", self.points.pts.len()));
        });
        ui.separator();
        if ui.add_enabled(self.points.enough(), egui::Button::new("ride these points")).clicked() { go = Some(self.points.pts.clone()); }
        if !self.points.enough() { ui.small("at least two: the rig needs two to follow a turn"); }
        go
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tapping_adds_or_selects_never_duplicates() {
        let mut p = Points::default();
        p.tap((0.3, 0.4));
        p.tap((0.6, 0.4));
        assert_eq!(p.pts.len(), 2);
        p.tap((0.305, 0.41));    // on the first point
        assert_eq!((p.pts.len(), p.selected), (2, Some(0)), "a tap on a point selects it");
        p.tap((5.0, -1.0));      // off the frame: clamped to its corner
        assert_eq!(p.pts.last(), Some(&(1.0, 0.0)));
    }

    #[test]
    fn dragging_moves_the_point_under_the_finger_and_only_that() {
        let mut p = Points::default();
        p.tap((0.3, 0.4)); p.tap((0.6, 0.4));
        assert_eq!(p.drag((0.6, 0.4), (0.1, 0.05)), Some(1));
        assert!((p.pts[1].0 - 0.7).abs() < 1e-6 && (p.pts[1].1 - 0.45).abs() < 1e-6);
        assert_eq!(p.pts[0], (0.3, 0.4), "the other point is untouched");
        assert_eq!(p.drag((0.1, 0.9), (0.2, 0.0)), None, "a drag on empty frame moves nothing and adds nothing");
        assert_eq!(p.pts.len(), 2);
        p.drag((0.7, 0.45), (1.0, 1.0));
        assert_eq!(p.pts[1], (1.0, 1.0), "kept inside the frame");
    }

    #[test]
    fn removing_and_the_two_point_minimum() {
        let mut p = Points::default();
        p.tap((0.2, 0.2));
        assert!(!p.enough(), "one point cannot follow a turn");
        p.tap((0.4, 0.2)); p.tap((0.6, 0.2));
        p.tap((0.4, 0.2));   // select the middle one
        p.remove_selected();
        assert_eq!(p.pts, vec![(0.2, 0.2), (0.6, 0.2)]);
        assert_eq!(p.selected, None);
        assert!(p.enough());
        p.clear();
        assert!(p.pts.is_empty() && !p.enough());
    }
}
