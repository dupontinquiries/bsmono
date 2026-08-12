//! The viewport: f64 world center + zoom, converting to/from f32 screen
//! coordinates. This is the CoordSpaceHelper idea from InfiniPaint reduced to
//! what CADscope v1 needs (no rotation, f64 not bignum) — but kept as the
//! *only* door between world and screen, so either can be upgraded later
//! without touching callers.
//!
//! Conventions: screen y grows down (egui/GTK), world y likewise (no flip);
//! `zoom` is screen-pixels-per-world-unit.

use crate::{RectD, Vec2d, Vec2f};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// World point at the center of the viewport.
    pub center: Vec2d,
    /// Screen px per world unit. Always > 0.
    pub zoom: f64,
    /// Viewport size in screen px (set each frame from the widget rect).
    pub viewport: Vec2f,
}

impl Camera {
    pub const MIN_ZOOM: f64 = 1e-9;
    pub const MAX_ZOOM: f64 = 1e12;

    pub fn new(viewport: Vec2f) -> Self {
        Camera {
            center: Vec2d::ZERO,
            zoom: 1.0,
            viewport,
        }
    }

    #[inline]
    pub fn to_screen(&self, w: Vec2d) -> Vec2f {
        Vec2f::new(
            ((w.x - self.center.x) * self.zoom + self.viewport.x as f64 * 0.5) as f32,
            ((w.y - self.center.y) * self.zoom + self.viewport.y as f64 * 0.5) as f32,
        )
    }

    #[inline]
    pub fn to_world(&self, s: Vec2f) -> Vec2d {
        Vec2d::new(
            (s.x as f64 - self.viewport.x as f64 * 0.5) / self.zoom + self.center.x,
            (s.y as f64 - self.viewport.y as f64 * 0.5) / self.zoom + self.center.y,
        )
    }

    /// Pan by a *screen-space* delta (what touchkit's `Pan { delta }` gives you).
    pub fn pan_screen(&mut self, delta: Vec2f) {
        self.center.x -= delta.x as f64 / self.zoom;
        self.center.y -= delta.y as f64 / self.zoom;
    }

    /// Multiply zoom by `factor`, keeping the world point under screen
    /// position `anchor` fixed (what touchkit's `Pinch { rel, center }` wants).
    pub fn zoom_about(&mut self, factor: f64, anchor: Vec2f) {
        let before = self.to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let after = self.to_world(anchor);
        self.center.x += before.x - after.x;
        self.center.y += before.y - after.y;
    }

    /// World rect currently visible (for canvas culling queries).
    pub fn visible_world(&self) -> RectD {
        let a = self.to_world(Vec2f::ZERO);
        let b = self.to_world(self.viewport);
        RectD {
            min: Vec2d::new(a.x.min(b.x), a.y.min(b.y)),
            max: Vec2d::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// Fit a world rect into the viewport with `margin_frac` (e.g. 0.1) border.
    pub fn fit(&mut self, r: RectD, margin_frac: f64) {
        if r.is_empty() || self.viewport.x <= 0.0 || self.viewport.y <= 0.0 {
            return;
        }
        self.center = r.center();
        let w = r.width().max(1e-12);
        let h = r.height().max(1e-12);
        let avail = 1.0 - margin_frac * 2.0;
        let zx = self.viewport.x as f64 * avail / w;
        let zy = self.viewport.y as f64 * avail / h;
        self.zoom = zx.min(zy).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut c = Camera::new(Vec2f::new(800.0, 600.0));
        c.center = Vec2d::new(123.0, -45.0);
        c.zoom = 3.5;
        let w = Vec2d::new(140.25, -12.5);
        let back = c.to_world(c.to_screen(w));
        assert!(back.dist(w) < 1e-4);
    }

    #[test]
    fn zoom_about_keeps_anchor_fixed() {
        let mut c = Camera::new(Vec2f::new(800.0, 600.0));
        c.center = Vec2d::new(10.0, 10.0);
        let anchor = Vec2f::new(200.0, 150.0);
        let before = c.to_world(anchor);
        c.zoom_about(2.5, anchor);
        let after = c.to_world(anchor);
        assert!(before.dist(after) < 1e-9);
        assert!((c.zoom - 2.5).abs() < 1e-12);
    }

    #[test]
    fn pan_moves_world_opposite() {
        let mut c = Camera::new(Vec2f::new(100.0, 100.0));
        c.zoom = 2.0;
        let w0 = c.to_world(Vec2f::new(50.0, 50.0));
        c.pan_screen(Vec2f::new(10.0, 0.0)); // content dragged right
        let w1 = c.to_world(Vec2f::new(50.0, 50.0));
        assert!((w0.x - w1.x - 5.0).abs() < 1e-9); // center moved left in world by 10/zoom
    }

    #[test]
    fn fit_contains_rect() {
        let mut c = Camera::new(Vec2f::new(400.0, 300.0));
        let mut r = RectD::EMPTY;
        r.include(Vec2d::new(-50.0, -20.0));
        r.include(Vec2d::new(150.0, 80.0));
        c.fit(r, 0.1);
        let vis = c.visible_world();
        assert!(vis.min.x <= r.min.x && vis.max.x >= r.max.x);
        assert!(vis.min.y <= r.min.y && vis.max.y >= r.max.y);
    }
}
