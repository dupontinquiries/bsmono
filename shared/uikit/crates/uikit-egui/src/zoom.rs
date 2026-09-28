//! zoom — digital zoom for a live picture.
//!
//! A [`Zoom`] is a window into a frame: a centre and a magnification, in
//! frame fractions (0..1 on each axis), so it means the same thing on the
//! screen, in the texture and in the saved file. The viewfinder draws the
//! window instead of the whole frame; the shutter crops the frame to the
//! same window. Nothing is upscaled on the way to disk: a photo taken at
//! 2× is the middle half of the sensor at its native pixels, which is what
//! "digital zoom" honestly is.
//!
//! Input comes from the kit's touch bridge ([`crate::touch`]): a two-finger
//! pinch on a screen, a pinch on a trackpad or ctrl+wheel on a mouse all
//! arrive as [`Gesture::Pinch`], and a one-finger drag while zoomed arrives
//! as [`Gesture::Pan`]. [`Zoom::apply`] folds a frame's gestures in; the
//! caller says which screen rect is the picture and how a screen point maps
//! onto it, because that mapping (mirror, quarter turns) is the
//! viewfinder's business and differs between apps.
//!
//! The model is pure and the tests below are the contract: pinching keeps
//! the point under the fingers still, the window never leaves the frame,
//! and 1× is exactly the whole frame.

use egui::{Pos2, Rect, Ui};

pub use crate::touch::Gesture;

/// The magnification a pinch may reach. Four is where a 1280-wide frame
/// becomes a 320-wide photo; past that the crop is a thumbnail.
pub const MAX: f32 = 4.0;
/// One keyboard or button step.
pub const STEP: f32 = 1.25;
/// Below this the factor is treated as 1×, so a pinch that drifts back to
/// 1.0004 does not leave a "1.0×" chip on the picture.
const ONE: f32 = 1.001;

/// A window into a frame, in frame fractions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    /// Magnification; 1.0 is the whole frame.
    pub factor: f32,
    /// The most `factor` may reach.
    pub max: f32,
    /// The window's centre, 0..1 on each axis.
    pub centre: [f32; 2],
    /// A one-finger drag that began on the picture while zoomed is panning
    /// the window; one that began elsewhere (a list under the picture) is
    /// not, however far it wanders.
    panning: bool,
}

impl Default for Zoom {
    fn default() -> Self { Zoom::new(MAX) }
}

impl Zoom {
    pub fn new(max: f32) -> Zoom {
        Zoom { factor: 1.0, max: max.max(1.0), centre: [0.5, 0.5], panning: false }
    }

    pub fn is_zoomed(&self) -> bool { self.factor > ONE }

    /// Half the window's side, in frame fractions.
    fn half(&self) -> f32 { 0.5 / self.factor }

    /// The window as `[u0, v0, u1, v1]`.
    pub fn window(&self) -> [f32; 4] {
        let h = self.half();
        [self.centre[0] - h, self.centre[1] - h, self.centre[0] + h, self.centre[1] + h]
    }

    /// The window as a UV rect, top-left to bottom-right.
    pub fn rect(&self) -> Rect {
        let w = self.window();
        Rect::from_min_max(Pos2::new(w[0], w[1]), Pos2::new(w[2], w[3]))
    }

    /// The texture rect for `Painter::image`: the window, with u running
    /// right to left when the preview is a mirror.
    pub fn uv_rect(&self, mirrored: bool) -> Rect {
        let w = self.window();
        if mirrored {
            Rect::from_min_max(Pos2::new(w[2], w[1]), Pos2::new(w[0], w[3]))
        } else {
            Rect::from_min_max(Pos2::new(w[0], w[1]), Pos2::new(w[2], w[3]))
        }
    }

    /// A fraction of the displayed picture (after the caller has undone any
    /// mirror or turn) to a fraction of the frame.
    pub fn frac_to_frame(&self, frac: [f32; 2]) -> [f32; 2] {
        let w = self.window();
        [w[0] + frac[0] * (w[2] - w[0]), w[1] + frac[1] * (w[3] - w[1])]
    }

    /// The inverse: where a frame point sits in the displayed picture.
    /// Outside 0..1 when the point is outside the window.
    pub fn frame_to_frac(&self, frame: [f32; 2]) -> [f32; 2] {
        let w = self.window();
        [(frame[0] - w[0]) * self.factor, (frame[1] - w[1]) * self.factor]
    }

    /// Keep the window inside the frame.
    fn clamp_centre(&mut self) {
        let h = self.half();
        for c in &mut self.centre {
            *c = if h >= 0.5 { 0.5 } else { c.clamp(h, 1.0 - h) };
        }
    }

    /// Set the magnification, clamped to `1..=max`; the centre is kept
    /// where it is (and pulled back inside the frame). Returns whether
    /// anything changed.
    pub fn set_factor(&mut self, f: f32) -> bool {
        let before = *self;
        self.factor = if f.is_finite() { f.clamp(1.0, self.max) } else { 1.0 };
        if !self.is_zoomed() { self.factor = 1.0; }
        self.clamp_centre();
        *self != before
    }

    /// Set the magnification so that the frame point `about` stays under
    /// the same spot on the screen — what a pinch means: the thing between
    /// the fingers grows, it does not slide away.
    pub fn set_factor_about(&mut self, f: f32, about: [f32; 2]) -> bool {
        let before = *self;
        let old = self.factor;
        let new = if f.is_finite() { f.clamp(1.0, self.max) } else { 1.0 };
        let rel = new / old;
        // A point at window fraction t stays at t: c' = about + (c − about) / rel.
        self.centre = [
            about[0] + (self.centre[0] - about[0]) / rel,
            about[1] + (self.centre[1] - about[1]) / rel,
        ];
        self.factor = new;
        if !self.is_zoomed() { self.factor = 1.0; }
        self.clamp_centre();
        *self != before
    }

    /// A pinch: `rel` is the multiplier since the last pinch event.
    pub fn pinch(&mut self, rel: f32, about: [f32; 2]) -> bool {
        if !rel.is_finite() || rel <= 0.0 { return false; }
        self.set_factor_about(self.factor * rel, about)
    }

    /// Move the window by `d`, given in fractions of the displayed picture
    /// (a drag across the whole picture is 1.0). Nothing happens at 1×,
    /// where there is nowhere to go.
    pub fn pan_frac(&mut self, d: [f32; 2]) -> bool {
        if !self.is_zoomed() { return false; }
        let before = *self;
        let size = 1.0 / self.factor;
        self.centre[0] += d[0] * size;
        self.centre[1] += d[1] * size;
        self.clamp_centre();
        *self != before
    }

    pub fn step_in(&mut self) -> bool { self.set_factor(self.factor * STEP) }
    pub fn step_out(&mut self) -> bool { self.set_factor(self.factor / STEP) }

    pub fn reset(&mut self) -> bool {
        let before = *self;
        self.factor = 1.0;
        self.centre = [0.5, 0.5];
        self.panning = false;
        *self != before
    }

    /// "2.0×", for the chip.
    pub fn label(&self) -> String { format!("{:.1}×", self.factor) }

    /// Fold a frame's gestures in. `rect` is where the picture is on
    /// screen; `to_frac` maps a screen point to a fraction of the displayed
    /// picture — the viewfinder's own mapping, mirror and turns included,
    /// unbounded (points outside the picture map outside 0..1).
    ///
    /// A pinch zooms about the point between the fingers when that point is
    /// on the picture; one elsewhere (over a list, over the controls) is
    /// left alone. A one-finger drag pans, but only one that began on the
    /// picture while zoomed — a list under the picture scrolls, it does not
    /// drag the window. Returns whether the window changed.
    pub fn apply(&mut self, gestures: &[Gesture], rect: Rect, to_frac: impl Fn(Pos2) -> [f32; 2]) -> bool {
        let mut changed = false;
        for g in gestures {
            match *g {
                Gesture::Pinch { rel, center, .. } => {
                    let c = Pos2::new(center.x, center.y);
                    if rect.contains(c) {
                        let about = self.frac_to_frame(to_frac(c));
                        changed |= self.pinch(rel, about);
                    }
                }
                Gesture::PanStart(p) => {
                    self.panning = self.is_zoomed() && rect.contains(Pos2::new(p.x, p.y));
                }
                Gesture::Pan { delta, pos } => {
                    if self.panning {
                        let now = to_frac(Pos2::new(pos.x, pos.y));
                        let was = to_frac(Pos2::new(pos.x - delta.x, pos.y - delta.y));
                        // The content follows the finger: the window goes the other way.
                        changed |= self.pan_frac([was[0] - now[0], was[1] - now[1]]);
                    }
                }
                Gesture::PanEnd(_) => self.panning = false,
                _ => {}
            }
        }
        changed
    }
}

/// The magnification chip over the top-right corner of the picture, only
/// while zoomed. A tap on it goes back to 1× — the way out that does not
/// need a second finger. Returns whether it was tapped.
///
/// Drawn in its own dark pill rather than in palette colours: it sits on a
/// photograph, and a photograph is any colour.
pub fn chip_ui(ui: &mut Ui, zoom: &mut Zoom, picture: Rect, id: egui::Id) -> bool {
    if !zoom.is_zoomed() { return false; }
    let font = egui::FontId::proportional(13.0);
    let text = zoom.label();
    let galley = ui.painter().layout_no_wrap(text, font, egui::Color32::WHITE);
    let pad = egui::vec2(10.0, 5.0);
    let size = galley.size() + pad * 2.0;
    let pill = Rect::from_min_size(Pos2::new(picture.right() - size.x - 8.0, picture.top() + 8.0), size);
    let resp = ui.interact(pill, id, egui::Sense::click());
    let p = ui.painter_at(picture);
    let alpha = if resp.hovered() { 200 } else { 150 };
    p.rect_filled(pill, size.y * 0.5, egui::Color32::from_black_alpha(alpha));
    p.galley(pill.min + pad, galley, egui::Color32::WHITE);
    let tapped = resp.clicked();
    if tapped { zoom.reset(); }
    resp.on_hover_text("tap for 1×");
    tapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use touchkit::Vec2 as TVec2;

    fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-4 }

    #[test]
    fn one_x_is_exactly_the_whole_frame() {
        let z = Zoom::default();
        assert!(!z.is_zoomed());
        assert_eq!(z.window(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(z.uv_rect(false), Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)));
        // The mirror runs u backwards and leaves v alone.
        assert_eq!(z.uv_rect(true), Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0)));
    }

    #[test]
    fn the_window_never_leaves_the_frame() {
        let mut z = Zoom::default();
        z.set_factor(2.0);
        assert_eq!(z.window(), [0.25, 0.25, 0.75, 0.75]);
        // Pan hard into a corner: the window stops at the edge.
        z.pan_frac([-10.0, -10.0]);
        assert_eq!(z.window(), [0.0, 0.0, 0.5, 0.5]);
        z.pan_frac([10.0, 0.0]);
        assert_eq!(z.window(), [0.5, 0.0, 1.0, 0.5]);
        // Zooming out from a corner pulls the window back inside.
        z.set_factor(1.0);
        assert_eq!(z.window(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(z.centre, [0.5, 0.5]);
    }

    #[test]
    fn the_factor_is_clamped_and_snaps_to_one() {
        let mut z = Zoom::new(3.0);
        assert!(z.set_factor(10.0));
        assert_eq!(z.factor, 3.0);
        assert!(z.set_factor(0.2));
        assert_eq!(z.factor, 1.0);
        z.set_factor(f32::NAN);
        assert_eq!(z.factor, 1.0, "a NaN factor is 1×, not a NaN window");
        z.set_factor(1.0004);
        assert!(!z.is_zoomed(), "a hair over 1× is 1×");
        assert_eq!(z.factor, 1.0);
    }

    #[test]
    fn pinching_keeps_the_point_between_the_fingers_still() {
        // Fingers on the frame point (0.8, 0.3). Wherever it was on screen
        // before the pinch, it is there after — the thing between the
        // fingers grows, it does not slide away.
        let mut z = Zoom::default();
        let about = [0.8, 0.3];
        let before = z.frame_to_frac(about);
        assert!(z.pinch(1.5, about));
        let after = z.frame_to_frac(about);
        assert!(close(before[0], after[0]) && close(before[1], after[1]), "{before:?} -> {after:?}");
        assert!(close(z.factor, 1.5));
        // Again, from a zoomed state and about a different point.
        let about = [0.55, 0.6];
        let before = z.frame_to_frac(about);
        z.pinch(1.3, about);
        let after = z.frame_to_frac(about);
        assert!(close(before[0], after[0]) && close(before[1], after[1]), "{before:?} -> {after:?}");
        // Pinching in reverses it exactly.
        z.pinch(1.0 / 1.3, about);
        z.pinch(1.0 / 1.5, [0.8, 0.3]);
        assert!(!z.is_zoomed());
    }

    #[test]
    fn a_pinch_at_the_edge_is_clamped_not_ignored() {
        // Zooming about a corner would put the window half outside; the
        // window is pulled in and the corner is what fills it.
        let mut z = Zoom::default();
        z.pinch(2.0, [0.0, 0.0]);
        assert_eq!(z.window(), [0.0, 0.0, 0.5, 0.5]);
    }

    #[test]
    fn frac_and_frame_round_trip() {
        let mut z = Zoom::default();
        z.set_factor(2.5);
        z.pan_frac([0.3, -0.2]);
        for f in [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0], [0.2, 0.9]] {
            let back = z.frame_to_frac(z.frac_to_frame(f));
            assert!(close(back[0], f[0]) && close(back[1], f[1]), "{f:?} -> {back:?}");
        }
    }

    #[test]
    fn steps_and_reset() {
        let mut z = Zoom::default();
        assert!(z.step_in());
        assert!(close(z.factor, STEP));
        assert!(z.step_out());
        assert!(!z.is_zoomed());
        assert!(!z.step_out(), "nothing below 1×");
        for _ in 0..20 { z.step_in(); }
        assert!(close(z.factor, MAX));
        assert_eq!(z.label(), "4.0×");
        assert!(z.reset());
        assert_eq!(z, Zoom::default());
        assert!(!z.reset(), "resetting 1× is not a change");
    }

    fn rect() -> Rect { Rect::from_min_size(Pos2::new(0.0, 100.0), egui::vec2(400.0, 300.0)) }

    /// Plain (unmirrored) picture mapping.
    fn plain(p: Pos2) -> [f32; 2] {
        let r = rect();
        [(p.x - r.left()) / r.width(), (p.y - r.top()) / r.height()]
    }

    /// The mirror a front camera has: screen-left is picture-right.
    fn mirrored(p: Pos2) -> [f32; 2] {
        let f = plain(p);
        [1.0 - f[0], f[1]]
    }

    #[test]
    fn a_pinch_on_the_picture_zooms_about_the_fingers_and_one_elsewhere_does_nothing() {
        let mut z = Zoom::default();
        // Fingers at the picture's right third, upper part.
        let at = TVec2::new(300.0, 175.0);
        let g = [Gesture::Pinch { scale: 2.0, rel: 2.0, center: at }];
        assert!(z.apply(&g, rect(), plain));
        assert!(close(z.factor, 2.0));
        // That frame point is still under the fingers.
        let want = plain(Pos2::new(300.0, 175.0));
        let got = z.frame_to_frac([0.75, 0.25]);
        assert!(close(got[0], want[0]) && close(got[1], want[1]), "{got:?} vs {want:?}");
        // A pinch over the list below the picture is not the picture's.
        let below = [Gesture::Pinch { scale: 2.0, rel: 2.0, center: TVec2::new(200.0, 700.0) }];
        assert!(!z.apply(&below, rect(), plain));
        assert!(close(z.factor, 2.0));
    }

    #[test]
    fn the_mirror_is_honoured_when_zooming_about_the_fingers() {
        // Fingers on the screen's left edge of a mirrored preview are on the
        // frame's RIGHT edge; the window must end up there.
        let mut z = Zoom::default();
        let g = [Gesture::Pinch { scale: 2.0, rel: 2.0, center: TVec2::new(1.0, 250.0) }];
        z.apply(&g, rect(), mirrored);
        let w = z.window();
        assert!(w[2] > 0.99, "window should hug the frame's right edge: {w:?}");
    }

    #[test]
    fn a_drag_that_began_on_the_zoomed_picture_pans_and_one_that_did_not_does_not() {
        let mut z = Zoom::default();
        z.set_factor(2.0);
        let start = z.window();
        // Began on the picture, moved left by 40 px (a tenth of the width):
        // the content follows the finger, so the window moves RIGHT by a
        // tenth of its own width.
        let g = [
            Gesture::PanStart(TVec2::new(200.0, 250.0)),
            Gesture::Pan { delta: TVec2::new(-40.0, 0.0), pos: TVec2::new(160.0, 250.0) },
            Gesture::PanEnd(TVec2::new(160.0, 250.0)),
        ];
        assert!(z.apply(&g, rect(), plain));
        let w = z.window();
        assert!(close(w[0], start[0] + 0.05), "{w:?} from {start:?}");
        // Began below the picture (the roll list), wandered onto it: not a pan.
        let before = z.window();
        let g = [
            Gesture::PanStart(TVec2::new(200.0, 600.0)),
            Gesture::Pan { delta: TVec2::new(0.0, -300.0), pos: TVec2::new(200.0, 300.0) },
            Gesture::PanEnd(TVec2::new(200.0, 300.0)),
        ];
        assert!(!z.apply(&g, rect(), plain));
        assert_eq!(z.window(), before);
        // On a mirrored preview the content still follows the finger on
        // SCREEN: a leftward drag reveals more of the screen's right, which
        // is the frame's left, so the window moves LEFT in frame space.
        let mut m = Zoom::default();
        m.set_factor(2.0);
        let g = [
            Gesture::PanStart(TVec2::new(200.0, 250.0)),
            Gesture::Pan { delta: TVec2::new(-40.0, 0.0), pos: TVec2::new(160.0, 250.0) },
        ];
        m.apply(&g, rect(), mirrored);
        assert!(close(m.window()[0], 0.25 - 0.05), "{:?}", m.window());
    }

    #[test]
    fn at_one_x_a_drag_is_not_a_pan() {
        // Nowhere to go, and the drag is probably the page's.
        let mut z = Zoom::default();
        let g = [
            Gesture::PanStart(TVec2::new(200.0, 250.0)),
            Gesture::Pan { delta: TVec2::new(-40.0, 0.0), pos: TVec2::new(160.0, 250.0) },
        ];
        assert!(!z.apply(&g, rect(), plain));
        assert_eq!(z, Zoom::default());
    }

    #[test]
    fn a_trackpad_pinch_or_ctrl_wheel_reaches_the_model_through_the_touch_bridge() {
        // egui folds a trackpad pinch and ctrl+wheel into `zoom_delta()`;
        // the bridge turns that into a Pinch whose `rel` IS the factor, at
        // the pointer. The apps rely on this being the same path a
        // two-finger pinch takes.
        let ctx = egui::Context::default();
        let mut src = crate::touch::TouchSource::new();
        let mut rec = crate::touch::GestureRecognizer::default();
        let mut z = Zoom::default();
        let screen = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 600.0)));
        let inside = Pos2::new(300.0, 175.0);
        let frames = vec![
            (0.0, vec![egui::Event::PointerMoved(inside)]),
            (0.016, vec![egui::Event::Zoom(1.5)]),
            (0.032, vec![]),
        ];
        for (t, events) in frames {
            let _ = ctx.run(egui::RawInput { screen_rect: screen, time: Some(t), events, ..Default::default() }, |ctx| {
                let (g, _) = src.poll(ctx, &mut rec);
                z.apply(&g, rect(), plain);
            });
        }
        assert!(close(z.factor, 1.5), "factor {}", z.factor);
        // And the frame point under the pointer stayed put.
        let want = plain(inside);
        let got = z.frame_to_frac([0.75, 0.25]);
        assert!(close(got[0], want[0]) && close(got[1], want[1]), "{got:?} vs {want:?}");
    }

    #[test]
    fn the_chip_shows_only_while_zoomed_and_a_tap_resets() {
        let ctx = egui::Context::default();
        let mut z = Zoom::default();
        let picture = rect();
        let mut drawn = false;
        let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 600.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                drawn = chip_ui(ui, &mut z, picture, egui::Id::new("chip"));
            });
        });
        assert!(!drawn);
        z.set_factor(2.0);
        // Click the pill: it sits at the picture's top-right corner.
        let click_at = Pos2::new(picture.right() - 24.0, picture.top() + 20.0);
        let press = |pressed: bool| egui::Event::PointerButton {
            pos: click_at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let screen = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 600.0)));
        let mut frames = vec![
            egui::RawInput { screen_rect: screen, events: vec![egui::Event::PointerMoved(click_at)], ..Default::default() },
            egui::RawInput { screen_rect: screen, events: vec![press(true)], ..Default::default() },
            egui::RawInput { screen_rect: screen, events: vec![press(false)], ..Default::default() },
        ];
        let mut tapped = false;
        for input in frames.drain(..) {
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    tapped |= chip_ui(ui, &mut z, picture, egui::Id::new("chip"));
                });
            });
        }
        assert!(tapped, "the tap on the chip did not register");
        assert!(!z.is_zoomed(), "the chip resets to 1×");
    }
}
