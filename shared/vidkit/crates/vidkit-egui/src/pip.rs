//! Picture-in-picture, the way a phone does it.
//!
//! The video shrinks to a floating window in a corner while the rest of the
//! app -- chapters, transcript search -- takes the screen. Drag it; let go
//! and it glides to the nearest corner, or to where a fling was heading.
//! Push or fling it off the side and it hides, leaving a small tab on that
//! edge; the sound carries on. Tap the tab and it comes back. Tap the window
//! and it expands to full screen.
//!
//! Everything that decides where the window goes is pure ([`Pip::rect`],
//! [`Pip::release`]), tested without a frame. [`Pip::show`] is the thin egui
//! part: it lays out, handles the drag, and hands back a rect for the caller
//! to paint the video into -- the caller owns how video is painted (GPU
//! callback or texture), exactly as with the full-screen view.

use egui::{Pos2, Rect, Sense, Vec2};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    fn right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }
    fn bottom(self) -> bool {
        matches!(self, Corner::BottomLeft | Corner::BottomRight)
    }
    fn from(right: bool, bottom: bool) -> Self {
        match (right, bottom) {
            (false, false) => Corner::TopLeft,
            (true, false) => Corner::TopRight,
            (false, true) => Corner::BottomLeft,
            (true, true) => Corner::BottomRight,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Edge {
    Left,
    Right,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum PipAction {
    /// Tapped the window: back to full screen.
    Expand,
    /// Tapped the tab: out of hiding.
    Unhide,
    Hid(Edge),
}

/// Tunables, in points and points per second.
#[derive(Copy, Clone, Debug)]
pub struct PipStyle {
    /// Window width as a fraction of the short side of the screen.
    pub width_frac: f32,
    pub margin: f32,
    /// Visible part of the tab when hidden.
    pub tab: f32,
    /// A release this fast sideways hides, wherever it is.
    pub fling_hide: f32,
    /// How far ahead a fling's velocity is projected when picking a corner.
    pub project_s: f32,
}

impl Default for PipStyle {
    fn default() -> Self {
        // 45% of the short side: on a 1080-wide phone in portrait, about the
        // size Android and iOS use. A 28-pt tab is a comfortable thumb target
        // while covering almost nothing.
        Self { width_frac: 0.45, margin: 12.0, tab: 28.0, fling_hide: 1600.0, project_s: 0.18 }
    }
}

#[derive(Clone, Debug)]
pub struct Pip {
    pub corner: Corner,
    pub hidden: Option<Edge>,
    /// Where the window's centre is while being dragged.
    drag: Option<Pos2>,
    pub style: PipStyle,
}

impl Default for Pip {
    fn default() -> Self {
        Self { corner: Corner::BottomRight, hidden: None, drag: None, style: PipStyle::default() }
    }
}

impl Pip {
    pub fn size(&self, screen: Rect, aspect: f32) -> Vec2 {
        let w = screen.width().min(screen.height()) * self.style.width_frac;
        let aspect = if aspect.is_finite() && aspect > 0.1 { aspect } else { 16.0 / 9.0 };
        Vec2::new(w, w / aspect)
    }

    /// Where the window is. Pure.
    pub fn rect(&self, screen: Rect, aspect: f32) -> Rect {
        let size = self.size(screen, aspect);
        let m = self.style.margin;
        if let Some(c) = self.drag {
            return Rect::from_center_size(c, size);
        }
        let y = if self.corner.bottom() { screen.bottom() - m - size.y } else { screen.top() + m };
        let x = match self.hidden {
            // Off the edge but for the tab's width, so the tab IS the window's
            // edge and a tap there has something to pull back.
            Some(Edge::Left) => screen.left() - size.x + self.style.tab,
            Some(Edge::Right) => screen.right() - self.style.tab,
            None if self.corner.right() => screen.right() - m - size.x,
            None => screen.left() + m,
        };
        Rect::from_min_size(Pos2::new(x, y), size)
    }

    /// The visible tab while hidden.
    pub fn tab_rect(&self, screen: Rect, aspect: f32) -> Option<Rect> {
        self.hidden?;
        Some(self.rect(screen, aspect).intersect(screen))
    }

    pub fn begin_drag(&mut self, screen: Rect, aspect: f32) {
        if self.drag.is_none() {
            self.drag = Some(self.rect(screen, aspect).center());
        }
    }

    pub fn drag_by(&mut self, delta: Vec2) {
        if let Some(c) = &mut self.drag {
            *c += delta;
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Let go. Pure: decides the corner, or hides. Returns the edge if it hid.
    ///
    /// Hides when the window's centre is past the screen's edge, or when a
    /// sideways fling is fast enough -- the gesture phones use. Otherwise the
    /// velocity is projected a short way ahead and the nearest corner to
    /// THAT wins, so a flick toward the top-left lands top-left even from
    /// just right of centre.
    pub fn release(&mut self, screen: Rect, velocity: Vec2) -> Option<Edge> {
        let Some(c) = self.drag.take() else { return None };
        let v = if velocity.x.is_finite() && velocity.y.is_finite() { velocity } else { Vec2::ZERO };
        let hide = if c.x < screen.left() || (v.x < -self.style.fling_hide && v.x.abs() > v.y.abs()) {
            Some(Edge::Left)
        } else if c.x > screen.right() || (v.x > self.style.fling_hide && v.x.abs() > v.y.abs()) {
            Some(Edge::Right)
        } else {
            None
        };
        let target = c + v * self.style.project_s;
        let bottom = target.y > screen.center().y;
        match hide {
            Some(e) => {
                self.corner = Corner::from(e == Edge::Right, bottom);
                self.hidden = Some(e);
            }
            None => {
                self.corner = Corner::from(target.x > screen.center().x, bottom);
                self.hidden = None;
            }
        }
        hide
    }

    pub fn unhide(&mut self) {
        self.hidden = None;
    }

    /// Lay out, handle input, and call `paint` with the window's rect so the
    /// caller paints the video INSIDE the window's layer (under its frame,
    /// over the page). Clipped to `screen`, so a hidden window's video shows
    /// only in the tab's sliver... which the tab then covers.
    pub fn show(&mut self, ctx: &egui::Context, screen: Rect, aspect: f32, paint: impl FnOnce(&mut egui::Ui, Rect)) -> (Rect, Option<PipAction>) {
        let rect = self.rect(screen, aspect);
        let mut action = None;
        let area = egui::Area::new(egui::Id::new("vidkit-pip"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            // egui keeps an Area on screen by default, which silently pulled a
            // HIDDEN window back into full view with the tab drawn on top --
            // seen in a screenshot, since the model's rects were right.
            .constrain(false)
            .interactable(true);
        area.show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(rect.size(), Sense::click_and_drag());
            if resp.drag_started() {
                self.begin_drag(screen, aspect);
            }
            if resp.dragged() {
                self.drag_by(resp.drag_delta());
            }
            if resp.drag_stopped() {
                let v = ui.input(|i| i.pointer.velocity());
                if let Some(e) = self.release(screen, v) {
                    action = Some(PipAction::Hid(e));
                }
            } else if resp.clicked() {
                action = Some(if self.hidden.is_some() {
                    self.unhide();
                    PipAction::Unhide
                } else {
                    PipAction::Expand
                });
            }
            ui.set_clip_rect(screen);
            ui.painter().rect_filled(r.expand(1.0), 10.0, egui::Color32::from_black_alpha(140));
            paint(ui, r);
            let edge = ui.visuals().widgets.inactive.bg_stroke.color;
            ui.painter().rect_stroke(r, 10.0, egui::Stroke::new(1.0_f32, edge), egui::StrokeKind::Outside);
        });
        if self.hidden.is_some() && !self.dragging() {
            // The tab: a grip on the visible sliver, so it reads as a handle
            // rather than a cropped video.
            if let Some(t) = self.tab_rect(screen, aspect) {
                let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("vidkit-pip-tab")));
                p.rect_filled(t, 8.0, egui::Color32::from_black_alpha(200));
                let x = t.center().x;
                for dy in [-6.0, 0.0, 6.0] {
                    p.line_segment([Pos2::new(x - 5.0, t.center().y + dy), Pos2::new(x + 5.0, t.center().y + dy)], egui::Stroke::new(2.0_f32, egui::Color32::from_gray(200)));
                }
            }
        }
        if self.dragging() {
            ctx.request_repaint();
        }
        (self.rect(screen, aspect), action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 800.0))
    }

    fn drop_at(p: &mut Pip, centre: Pos2, v: Vec2) -> Option<Edge> {
        p.begin_drag(phone(), 16.0 / 9.0);
        p.drag = Some(centre);
        p.release(phone(), v)
    }

    #[test]
    fn it_rests_inside_the_screen_in_its_corner_at_phone_size() {
        let p = Pip::default();
        let r = p.rect(phone(), 16.0 / 9.0);
        assert!(phone().contains_rect(r), "{r:?}");
        assert_eq!(r.width(), 180.0, "45% of the short side");
        assert!((r.right() - (400.0 - 12.0)).abs() < 0.01 && (r.bottom() - (800.0 - 12.0)).abs() < 0.01);
        // A portrait video is taller than wide, and still fits.
        let tall = p.rect(phone(), 9.0 / 16.0);
        assert!(tall.height() > tall.width() && phone().contains_rect(tall));
    }

    #[test]
    fn let_go_it_snaps_to_the_nearest_corner() {
        let mut p = Pip::default();
        assert_eq!(drop_at(&mut p, Pos2::new(100.0, 150.0), Vec2::ZERO), None);
        assert_eq!(p.corner, Corner::TopLeft);
        drop_at(&mut p, Pos2::new(300.0, 700.0), Vec2::ZERO);
        assert_eq!(p.corner, Corner::BottomRight);
    }

    /// Just right of centre, flicked up and left: lands top-left.
    #[test]
    fn a_flick_lands_where_it_was_heading() {
        let mut p = Pip::default();
        drop_at(&mut p, Pos2::new(220.0, 450.0), Vec2::new(-900.0, -1200.0));
        assert_eq!(p.corner, Corner::TopLeft);
        assert_eq!(p.hidden, None, "900 pt/s sideways is a move, not a hide");
    }

    #[test]
    fn pushed_or_flung_off_the_side_it_hides_and_leaves_a_tab() {
        let mut p = Pip::default();
        assert_eq!(drop_at(&mut p, Pos2::new(-10.0, 600.0), Vec2::ZERO), Some(Edge::Left));
        let tab = p.tab_rect(phone(), 16.0 / 9.0).unwrap();
        assert!((tab.width() - 28.0).abs() < 0.01 && tab.left() == 0.0, "{tab:?}");
        assert!(tab.center().y > 400.0, "it stays at the height it left");
        p.unhide();
        assert!(phone().contains_rect(p.rect(phone(), 16.0 / 9.0)), "comes back on screen");
        assert_eq!(p.corner, Corner::BottomLeft);

        let mut q = Pip::default();
        assert_eq!(drop_at(&mut q, Pos2::new(250.0, 200.0), Vec2::new(2400.0, 300.0)), Some(Edge::Right));
        let t = q.tab_rect(phone(), 16.0 / 9.0).unwrap();
        assert!((t.right() - 400.0).abs() < 0.01);
    }

    /// A fast VERTICAL fling is a move to a corner, never a hide, even with
    /// some sideways drift.
    #[test]
    fn a_vertical_fling_does_not_hide() {
        let mut p = Pip::default();
        assert_eq!(drop_at(&mut p, Pos2::new(300.0, 600.0), Vec2::new(1700.0, -3000.0)), None);
        assert_eq!(p.corner, Corner::TopRight);
    }

    #[test]
    fn nonsense_velocity_is_ignored() {
        let mut p = Pip::default();
        assert_eq!(drop_at(&mut p, Pos2::new(100.0, 100.0), Vec2::new(f32::NAN, f32::INFINITY)), None);
        assert_eq!(p.corner, Corner::TopLeft);
    }

    /// Painting and input need a live context.
    #[test]
    #[allow(deprecated)]
    fn it_paints_visible_and_hidden_without_panicking() {
        let ctx = egui::Context::default();
        let mut p = Pip::default();
        for hidden in [None, Some(Edge::Right)] {
            p.hidden = hidden;
            let _ = ctx.run(Default::default(), |ctx| {
                let mut painted = None;
                let (r, a) = p.show(ctx, phone(), 16.0 / 9.0, |_, r| painted = Some(r));
                // egui rounds an Area to whole pixels; within one is exact.
                let got = painted.expect("paint called");
                assert!((got.min - r.min).length() < 1.0 && (got.size() - r.size()).length() < 1.0, "{got:?} vs {r:?}");
                assert!(r.width() > 0.0);
                assert!(a.is_none());
            });
        }
    }

    fn frame(ctx: &egui::Context, p: &mut Pip, events: Vec<egui::Event>) -> Option<PipAction> {
        let mut input = egui::RawInput { screen_rect: Some(phone()), ..Default::default() };
        input.events = events;
        let mut out = None;
        #[allow(deprecated)]
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| ui.label("page"));
            out = p.show(ctx, phone(), 16.0 / 9.0, |_, _| {}).1;
        });
        out
    }
    fn press(at: Pos2, down: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: down, modifiers: Default::default() }
    }

    /// Real input through a live context, over a page: the window must get
    /// the tap. (It did not, in the app, when this test did not exist.)
    #[test]
    fn a_tap_on_the_window_expands_it() {
        let ctx = egui::Context::default();
        let mut p = Pip::default();
        let c = p.rect(phone(), 16.0 / 9.0).center();
        frame(&ctx, &mut p, vec![]);
        frame(&ctx, &mut p, vec![egui::Event::PointerMoved(c)]);
        frame(&ctx, &mut p, vec![press(c, true)]);
        let a = frame(&ctx, &mut p, vec![press(c, false)]);
        assert_eq!(a, Some(PipAction::Expand));
    }

    #[test]
    fn a_drag_off_the_side_hides_it() {
        let ctx = egui::Context::default();
        let mut p = Pip::default();
        let c = p.rect(phone(), 16.0 / 9.0).center();
        frame(&ctx, &mut p, vec![]);
        frame(&ctx, &mut p, vec![egui::Event::PointerMoved(c)]);
        frame(&ctx, &mut p, vec![press(c, true)]);
        let mut x = c.x;
        let mut last = None;
        for _ in 0..12 {
            x += 40.0;
            last = frame(&ctx, &mut p, vec![egui::Event::PointerMoved(Pos2::new(x, c.y))]);
        }
        let a = frame(&ctx, &mut p, vec![press(Pos2::new(x, c.y), false)]).or(last);
        assert_eq!(a, Some(PipAction::Hid(Edge::Right)), "dragged to x={x}");
        assert_eq!(p.hidden, Some(Edge::Right));
    }

    /// Where the video is ACTUALLY painted when hidden: off screen but for
    /// the tab. (The model said so all along; egui's Area constraint put it
    /// back on screen, and only a screenshot showed that.)
    #[test]
    fn a_hidden_window_is_really_painted_off_screen() {
        let ctx = egui::Context::default();
        let mut p = Pip { hidden: Some(Edge::Right), ..Default::default() };
        let mut painted = None;
        for _ in 0..3 {
            let mut input = egui::RawInput { screen_rect: Some(phone()), ..Default::default() };
            input.events = vec![];
            #[allow(deprecated)]
            let _ = ctx.run(input, |ctx| {
                p.show(ctx, phone(), 16.0 / 9.0, |_, r| painted = Some(r));
            });
        }
        let r = painted.unwrap();
        let visible = r.intersect(phone());
        assert!(visible.width() <= p.style.tab + 1.0, "{} pt of a hidden window on screen: {r:?}", visible.width());
    }
}
