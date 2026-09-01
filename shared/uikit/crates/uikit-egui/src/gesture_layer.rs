//! GestureLayer — the adapter that makes `uikit-gesture::TwoTierGesture` work
//! from egui input with zero per-app gesture code.
//!
//! The app owns one `GestureLayer` per swipe surface. Each frame it calls
//! [`GestureLayer::update`] with the surface rect and the surface's at-limit
//! flag; it gets back a [`GestureFrame`] describing what the model decided
//! (detent fired / page jump) plus the live barrier fraction to paint.
//!
//! Mapping (the snapclone convention):
//!   * horizontal pointer drag / two-finger horizontal scroll → the model
//!     (detent steps a sub-page; release decides a page jump),
//!   * "Next" = content moves left (finger left), so we feed `-dx`.
//!
//! This is the seam where the FP5-tuned feel reaches egui: the app never sees
//! raw deltas, only `DetentFired`/`PageJump`/progress.

use egui::{Rect, Ui, Vec2};
use uikit_gesture::{Direction, GestureConfig, Outcome, ReleaseOutcome, TwoTierGesture};

/// Axis the layer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// What happened on the surface this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct GestureFrame {
    /// A sub-page detent fired this frame (step one sub-page).
    pub detent: Option<Direction>,
    /// A whole-page jump committed this frame (at release).
    pub page_jump: Option<Direction>,
    /// Live barrier progress 0..1 (paint the "almost there" indicator).
    pub barrier_fraction: f32,
    /// Which edge the barrier is on.
    pub barrier_edge: Option<Direction>,
    /// True while the post-fire takeover lock is holding (for a HUD).
    pub locked: bool,
    /// Signed accumulator, for diagnostics/HUD.
    pub raw: f32,
}

/// One gesture surface.
pub struct GestureLayer {
    gesture: TwoTierGesture,
    axis: Axis,
    dragging: bool,
    /// The current press belongs to a widget — a text selection, a slider,
    /// a scroll area's own drag — and is not a page gesture. Set at drag
    /// start or the moment a widget claims the pointer mid-drag; cleared
    /// on release. See `pointer_belongs_to_a_widget`.
    suppressed: bool,
    drag_total: f32,
    // px/ms, signed along the chosen axis.
    last_velocity: f32,
    // snapshot for change detection in `update()` — see `config_mut`'s docs
    // for why the reset can't just live in `config_mut()` itself.
    last_config: GestureConfig,
}

impl GestureLayer {
    pub fn new(axis: Axis, cfg: GestureConfig) -> Self {
        Self {
            gesture: TwoTierGesture::new(cfg),
            axis,
            dragging: false,
            suppressed: false,
            drag_total: 0.0,
            last_velocity: 0.0,
            last_config: cfg,
        }
    }

    /// A horizontal surface sized to `page_px` (the common snapclone case).
    pub fn horizontal_page(page_px: f32) -> Self {
        Self::new(Axis::Horizontal, GestureConfig::for_page_size(page_px as f64))
    }

    /// Live-retune (Settings ▸ Gestures). Returns the mutable config.
    ///
    /// Note: does NOT reset the accumulated streak by itself — some callers
    /// (e.g. `Pager::ui`) call this every frame to retune `page_size` to the
    /// current layout width, which is normal operation, not a "settings
    /// changed" event, and must not interrupt an in-progress drag. Streak
    /// reset happens in `update()` instead, and only when the config's
    /// *value* actually differs from last frame's — see `last_config`.
    pub fn config_mut(&mut self) -> &mut GestureConfig {
        self.gesture.config_mut()
    }

    fn axis_of(&self, v: Vec2) -> f32 {
        match self.axis {
            Axis::Horizontal => v.x,
            Axis::Vertical => v.y,
        }
    }

    /// Drive the model from this frame's input.
    ///
    /// `surface` bounds the area that should respond. `at_limit` is the app's
    /// answer to "is the inner content scrolled to its edge on this axis?" —
    /// when false, the detent streak is reset so a fresh pull starts clean
    /// (the model never invents a sub-step in the middle of a scroll).
    pub fn update(&mut self, ui: &Ui, surface: Rect, at_limit: bool) -> GestureFrame {
        // Change-detection, not reset-on-every-call: `config_mut()` itself must stay reset-free
        // (Pager retunes page_size to the current layout width via config_mut() every frame,
        // which is normal operation and must not interrupt an in-progress drag). Only an actual
        // *value* change — e.g. the user editing gesture sensitivity in Settings while mid-swipe
        // — should discard accumulated state, so state computed against stale thresholds is
        // never compared against a config that has since moved on.
        let current_config = *self.gesture.config();
        if current_config != self.last_config {
            self.gesture.reset_streak();
            self.last_config = current_config;
        }

        let mut frame = GestureFrame::default();
        let now_ms = ui.input(|i| i.time) * 1000.0;

        let (pointer_down, hovered, delta, velocity_pps) = ui.input(|i| {
            let p = &i.pointer;
            let hovered = p.hover_pos().map_or(false, |pos| surface.contains(pos));
            (
                p.primary_down(),
                hovered,
                p.delta(),
                p.velocity(),
            )
        });

        // ----- page tier: pointer drag within the surface -----
        //
        // THE MONOREPO-WIDE BUG lived on this branch: any primary-down
        // pointer over the surface was a page drag, so selecting text in a
        // TextEdit, dragging a slider, or scrolling a list by touch all
        // swiped the page out from under the widget being used. Every app
        // feeds this one adapter, which is why it was every app's bug and
        // is one fix.
        if pointer_down && (self.dragging || hovered) {
            if !self.dragging {
                self.dragging = true;
                self.drag_total = 0.0;
                self.gesture.reset_streak();
                self.suppressed = pointer_belongs_to_a_widget(ui.ctx());
            } else if !self.suppressed && pointer_belongs_to_a_widget(ui.ctx()) {
                // A widget claimed the pointer after the press — a text
                // selection only becomes a drag once the finger has moved.
                // Whatever this drag accumulated was the widget's motion.
                self.suppressed = true;
                self.gesture.reset_streak();
            }
            if self.suppressed {
                return frame;
            }
            let d = self.axis_of(delta);
            self.drag_total += d;
            self.last_velocity = self.axis_of(velocity_pps) / 1000.0; // px/ms

            // Treat a page surface as "at its horizontal limit" (no inner
            // horizontal scroll): accumulate drag toward a sub-page detent.
            // Next = finger toward -axis (content moves left/up) → feed -d.
            let o = self.gesture.on_scroll(-(d as f64), now_ms);
            apply_outcome(o, &mut frame);
        } else if self.dragging {
            // release
            self.dragging = false;
            if std::mem::take(&mut self.suppressed) {
                return frame;
            }
            let rel = self
                .gesture
                .on_release(self.last_velocity as f64 * -1.0, self.drag_total as f64 * -1.0);
            if let ReleaseOutcome::PageJump { dir } = rel {
                frame.page_jump = Some(dir);
            }
        }

        // ----- trackpad: two-finger scroll along the axis when hovered -----
        if hovered && !pointer_down {
            let scroll = ui.input(|i| self.axis_of(i.smooth_scroll_delta));
            if scroll.abs() > 0.01 {
                if at_limit {
                    let o = self.gesture.on_scroll(scroll as f64, now_ms);
                    apply_outcome(o, &mut frame);
                } else {
                    self.gesture.reset_streak();
                }
            }
        }

        frame.locked = self.gesture.locked(now_ms);
        frame.raw = self.gesture.raw() as f32;
        if frame.barrier_edge.is_none() {
            // keep showing residual progress between samples
            let f = (self.gesture.raw().abs() / self.gesture.config().detent_threshold) as f32;
            frame.barrier_fraction = f.clamp(0.0, 1.0);
            if frame.barrier_fraction > 0.0 {
                frame.barrier_edge =
                    Some(if self.gesture.raw() >= 0.0 { Direction::Next } else { Direction::Prev });
            }
        }
        frame
    }
}

/// Does the current press belong to a widget rather than to the page?
///
/// Two signals, both from egui's own bookkeeping rather than from any
/// list of widget types this crate would have to keep current:
///
/// * `dragged_id()` — a widget with `Sense::drag` is being dragged. egui
///   sets it in the start-of-frame interaction snapshot, so it reads the
///   same whether this layer updates before or after the widget draws.
///   A TextEdit selecting text, a selectable label, a slider, a
///   ScrollArea being dragged by touch all set it.
/// * the focused widget's rect contains the press origin — the press
///   landed in the text field the user is editing. Caught at press time,
///   before any movement, so the page never moves even for the first few
///   pixels of a selection drag.
///
/// `egui_wants_pointer_input` is deliberately NOT used: it is true
/// whenever the pointer merely hovers a widget, which is always.
pub fn pointer_belongs_to_a_widget(ctx: &egui::Context) -> bool {
    if ctx.dragged_id().is_some() {
        return true;
    }
    let Some(origin) = ctx.input(|i| i.pointer.press_origin()) else { return false };
    match ctx.memory(|m| m.focused()) {
        Some(id) => ctx.read_response(id).is_some_and(|r| r.rect.contains(origin)),
        None => false,
    }
}

fn apply_outcome(o: Outcome, frame: &mut GestureFrame) {
    match o {
        Outcome::None => {}
        Outcome::DetentFired { dir } => {
            frame.detent = Some(dir);
            frame.barrier_fraction = 0.0;
            frame.barrier_edge = None;
        }
        Outcome::DetentProgress { fraction, edge } => {
            frame.barrier_fraction = fraction as f32;
            frame.barrier_edge = Some(edge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, PointerButton, Pos2, RawInput};

    /// Drive `GestureLayer::update` through a real egui frame, mirroring the
    /// harness `apps/snapclone-egui` uses for its own gesture tests — this
    /// crate had zero test coverage on the egui-input adapter itself
    /// (`uikit-gesture`'s pure model is tested separately, but nothing here
    /// exercised the seam between real `Ui`/`InputState` and it).
    fn frame(ctx: &Context, gesture: &mut GestureLayer, surface: Rect, events: Vec<Event>) -> GestureFrame {
        let mut out = GestureFrame::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 800.0));
        let _ = ctx.run(RawInput { events, screen_rect: Some(screen), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                out = gesture.update(ui, surface, true);
            });
        });
        out
    }

    /// Like `frame`, with a clock and content drawn inside the surface
    /// BEFORE the layer updates — the order every app uses, and the order
    /// in which the bug reproduced.
    fn frame_with(ctx: &Context, gesture: &mut GestureLayer, surface: Rect, t: f64,
                  events: Vec<Event>, mut draw: impl FnMut(&mut Ui)) -> GestureFrame {
        let mut out = GestureFrame::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 800.0));
        let _ = ctx.run(RawInput { events, screen_rect: Some(screen), time: Some(t), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(ui);
                out = gesture.update(ui, surface, true);
            });
        });
        out
    }

    /// Press at `from`, drag to `to` over several frames, release. Returns
    /// every frame's outcome; the last is the release.
    fn drag(ctx: &Context, g: &mut GestureLayer, surface: Rect, from: Pos2, to: Pos2,
            mut draw: impl FnMut(&mut Ui)) -> Vec<GestureFrame> {
        let press = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut t = 0.0;
        let mut out = Vec::new();
        // A warm-up frame so egui knows the widget rects before the press.
        frame_with(ctx, g, surface, t, vec![Event::PointerMoved(from)], &mut draw);
        t += 0.016;
        out.push(frame_with(ctx, g, surface, t, vec![press(from, true)], &mut draw));
        for i in 1..=8 {
            t += 0.016;
            let pos = from + (to - from) * (i as f32 / 8.0);
            out.push(frame_with(ctx, g, surface, t, vec![Event::PointerMoved(pos)], &mut draw));
        }
        t += 0.016;
        out.push(frame_with(ctx, g, surface, t, vec![press(to, false)], &mut draw));
        out
    }

    const FIELD: Rect = Rect { min: Pos2::new(40.0, 380.0), max: Pos2::new(360.0, 420.0) };

    fn draw_text_field(text: &mut String, focus: bool) -> impl FnMut(&mut Ui) + '_ {
        move |ui: &mut Ui| {
            let r = ui.put(FIELD, egui::TextEdit::singleline(text));
            if focus { r.request_focus(); }
        }
    }

    /// THE MONOREPO-WIDE BUG. Selecting text by dragging across a focused
    /// TextEdit swiped the page: the layer took any primary-down pointer
    /// over the surface as a page drag. The same drag, same distance, same
    /// velocity, outside the field is the control and must still swipe.
    #[test]
    fn a_drag_inside_a_focused_text_field_does_not_swipe_the_page() {
        let surface = Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        let mut text = String::from("select some of this text by dragging");

        let ctx = Context::default();
        let mut g = GestureLayer::horizontal_page(320.0);
        let frames = drag(&ctx, &mut g, surface, Pos2::new(300.0, 400.0), Pos2::new(60.0, 400.0),
                          draw_text_field(&mut text, true));
        assert!(frames.iter().all(|f| f.page_jump.is_none() && f.detent.is_none() && f.raw == 0.0),
                "a text-selection drag moved the page: {:?}",
                frames.iter().map(|f| (f.page_jump, f.raw)).collect::<Vec<_>>());

        // Control: the same drag below the field.
        let ctx = Context::default();
        let mut g = GestureLayer::horizontal_page(320.0);
        let frames = drag(&ctx, &mut g, surface, Pos2::new(300.0, 600.0), Pos2::new(60.0, 600.0),
                          draw_text_field(&mut text, true));
        assert!(frames.last().unwrap().page_jump.is_some(),
                "the control drag outside the field must still swipe, or the fix is a suppression of everything");
    }

    /// A widget that claims the pointer only once the finger has moved — a
    /// slider, a selectable label, a scroll area's touch drag — was not
    /// under the press as a focused field. The layer must let go the frame
    /// the widget takes hold, and not fire a jump on release for the
    /// distance it had already counted.
    #[test]
    fn a_widget_claiming_the_pointer_mid_drag_takes_it_from_the_page() {
        let surface = Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        let mut value = 0.5f32;
        let ctx = Context::default();
        let mut g = GestureLayer::horizontal_page(320.0);
        // The rail is `slider_width` wide regardless of the rect it is put
        // in; widen it so the press lands ON the slider, or this tests a
        // press on empty page, which must jump.
        let frames = drag(&ctx, &mut g, surface, Pos2::new(300.0, 400.0), Pos2::new(60.0, 400.0),
                          |ui: &mut Ui| {
                              ui.spacing_mut().slider_width = 300.0;
                              ui.put(FIELD, egui::Slider::new(&mut value, 0.0..=1.0).show_value(false));
                          });
        assert!(frames.last().unwrap().page_jump.is_none(), "the slider's drag became a page jump");
        assert!(value < 0.5, "the slider itself did not receive the drag: {value}");
    }

    /// The bug: nothing detected a config change, so a value change mid-drag
    /// (e.g. Settings ▸ Gestures edited while mid-swipe) left the accumulated
    /// detent state (`raw`, `armed`, etc.) computed against thresholds that
    /// no longer matched the config it would be compared against next frame.
    /// The fix detects an actual *value* change in `update()` (not merely a
    /// call to `config_mut()` — see that method's docs for why `Pager`
    /// retuning `page_size` every frame must NOT trip this) and resets the
    /// streak only then.
    #[test]
    fn a_real_config_value_change_mid_drag_resets_the_accumulated_streak() {
        let ctx = Context::default();
        let mut g = GestureLayer::horizontal_page(320.0);
        let surface = Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));

        let at = |x: f32| Event::PointerMoved(Pos2::new(x, 400.0));
        let press = |x: f32| Event::PointerButton {
            pos: Pos2::new(x, 400.0), button: PointerButton::Primary,
            pressed: true, modifiers: Default::default() };

        frame(&ctx, &mut g, surface, vec![at(320.0), press(320.0)]);
        let mid = frame(&ctx, &mut g, surface, vec![at(260.0)]); // a real drag, accumulating
        assert!(mid.raw != 0.0, "fixture must actually accumulate some raw progress to be meaningful");

        // Mid-drag config VALUE change (e.g. user opens Settings ▸ Gestures while mid-swipe) —
        // note this is a real value change, not merely a call to config_mut(), which by itself
        // must be side-effect-free (Pager calls it every frame with an unchanged value).
        g.config_mut().detent_threshold *= 2.0;

        // Same position again -> zero delta this frame, so `raw` reflects only what survived
        // the config change above, not any new accumulation.
        let after = frame(&ctx, &mut g, surface, vec![at(260.0)]);
        assert_eq!(after.raw, 0.0,
            "a config VALUE change mid-drag must reset the accumulated streak, not leave it \
             computed against the old config (got raw={})", after.raw);
    }

    /// The regression this guards against: `Pager::ui()` calls `config_mut()` every single
    /// frame to retune `page_size` to the current layout width (see pager.rs) — that is normal
    /// operation, not a settings change, and must never interrupt an in-progress drag. A naive
    /// fix that resets the streak inside `config_mut()` itself (rather than detecting an actual
    /// value change in `update()`) would break every app's swipe navigation, since the config
    /// gets "mutated" (to the same value) every frame regardless of whether anything changed.
    #[test]
    fn config_mut_called_every_frame_with_an_unchanged_value_does_not_break_a_drag() {
        let ctx = Context::default();
        let mut g = GestureLayer::horizontal_page(320.0);
        let surface = Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));

        let at = |x: f32| Event::PointerMoved(Pos2::new(x, 400.0));
        let press = |x: f32| Event::PointerButton {
            pos: Pos2::new(x, 400.0), button: PointerButton::Primary,
            pressed: true, modifiers: Default::default() };

        frame(&ctx, &mut g, surface, vec![at(320.0), press(320.0)]);
        let mid = frame(&ctx, &mut g, surface, vec![at(260.0)]);
        assert!(mid.raw != 0.0, "fixture must actually accumulate some raw progress to be meaningful");

        // Exactly what Pager::ui() does every frame: re-derive and re-assign the SAME value.
        *g.config_mut() = uikit_gesture::GestureConfig::for_page_size(320.0);

        // Continue the same drag — accumulation must carry over uninterrupted.
        let after = frame(&ctx, &mut g, surface, vec![at(200.0)]);
        assert!(after.raw.abs() > mid.raw.abs(),
            "an unchanged-value config_mut() call must not reset an in-progress drag \
             (mid={}, after={})", mid.raw, after.raw);
    }
}
