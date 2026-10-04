//! mic_indicator — a small badge signalling "about to start listening"
//! as a dwell fills, "listening now" once live, with a manual override.
//!
//! ## Deliberately decoupled from `palette_flow`
//!
//! This takes plain data (`warming: f32`, `listening: bool`), not a
//! `PaletteFlow` reference. `palette_flow.rs`'s idle-dwell design is one
//! specific way to decide WHEN to warm up voice; a future consumer with
//! a different trigger (a settings toggle, a different key, a totally
//! different app) should be able to reuse the same drawing and the same
//! hold-to-cancel gesture without adopting that state machine too. The
//! widget only needs to know "how close" and "is it live", which is the
//! smallest interface that serves both.
//!
//! ## The hold-to-cancel gesture reuses `cmdstrip::Hold` again
//!
//! Third real consumer of that primitive in this crate now (touch
//! buttons, the keyboard adapter in `ctrl_p_gesture.rs`, and this) --
//! each time confirming it is worth having ONE shared timing rather than
//! a new guess per feature. Right-click is an instant `ForceOff`, no
//! dwell needed (a deliberate click is already unambiguous); a plain
//! press-and-hold on the icon ALSO forces off once past the same shared
//! `cmdstrip::HOLD_SECONDS` threshold, for input methods without a
//! right-click at all (touch).
//!
//! ## The fill visual is a first pass, not a finished art piece
//!
//! `warming` drives a color lerp from `text_weak` toward `accent` plus a
//! full-circle outline ring -- a literal "getting there" cue, but a true
//! partial arc (filling clockwise as `warming` climbs, rather than a
//! binary ring that is either drawn or not) would read better and was
//! deferred rather than guessed at without seeing either version
//! rendered. `egui::Shape::PathStroke`/`PathFill` can build one; left as
//! the next iteration once this first version is visible in a real app.

/// What the person asked the indicator to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicAction {
    None,
    /// A plain left click -- "start listening," for a caller with no
    /// separate arming/dwell step (aperture's Mode::VoiceCommand: the
    /// mode is already entered explicitly, so the mic is a click-to-talk
    /// button, not an idle-triggered one the way voiceterm's Ctrl+P flow
    /// is). Not returned while `listening` is already true -- clicking a
    /// mic that is already live is not a second "start."
    Start,
    /// Right-click, or a press-and-hold past the shared threshold --
    /// either way, "stop this, right now." The caller's own state
    /// machine decides what "stop" means (closing entirely, cancelling
    /// an in-flight capture, or both).
    ForceOff,
}

/// Owns the press-and-hold timer for the cancel gesture. One instance
/// per icon on screen.
#[derive(Default)]
pub struct MicIndicator {
    cancel_hold: crate::cmdstrip::Hold,
    /// Tracks the rising edge of the press, the same shape
    /// `ctrl_p_gesture::GestureTracker` uses to adapt a level-based query
    /// (`is_pointer_button_down_on()`/`key_down`, true every frame while
    /// held) onto `Hold`'s discrete `press()`/`release()` calls.
    was_pressed: bool,
}

impl MicIndicator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw the icon and report any cancel gesture. `warming` is 0.0
    /// (not dwelling toward voice at all -- caller may choose not to
    /// call this) to 1.0 (about to switch on); `listening` overrides the
    /// warming visual once true. `dt` is this frame's duration in
    /// seconds, needed to advance the hold timer.
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        colors: &crate::PaletteColors,
        warming: f32,
        listening: bool,
        dt: f32,
    ) -> MicAction {
        let size = egui::vec2(28.0, 28.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());

        let warming = warming.clamp(0.0, 1.0);
        let bg = if listening {
            colors.danger
        } else {
            lerp_color(colors.text_weak, colors.accent, warming)
        };
        ui.painter().circle_filled(rect.center(), 14.0, bg);
        if warming > 0.0 && !listening {
            // A full ring at ANY nonzero warming, not a partial arc yet
            // -- see the module doc for why that is a known, deliberate
            // gap rather than an oversight.
            ui.painter().circle_stroke(
                rect.center(),
                14.0,
                egui::Stroke::new(2.0_f32, colors.accent),
            );
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "\u{1F3A4}",
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );

        if resp.secondary_clicked() {
            self.cancel_hold.abort();
            self.was_pressed = false;
            return MicAction::ForceOff;
        }

        // A plain click starts listening -- checked via `clicked()`
        // (egui's own "full press+release, no drag") rather than folded
        // into the hold-cancel timer below, since that timer answers a
        // different question (was THIS press long enough to cancel) and
        // a quick click already completes before `cancel_hold` would
        // ever cross its threshold. Not fired while already `listening`
        // -- see MicAction::Start's own doc for why.
        if resp.clicked() && !listening {
            return MicAction::Start;
        }

        let pressed_now = resp.is_pointer_button_down_on();
        if pressed_now && !self.was_pressed {
            self.cancel_hold.press();
        }
        self.was_pressed = pressed_now;

        if pressed_now {
            if self.cancel_hold.tick(dt) {
                return MicAction::ForceOff;
            }
        } else {
            let _ = self.cancel_hold.release();
        }

        MicAction::None
    }
}

/// Linear interpolation between two colors in gamma space -- matching
/// `Color32::lerp_to_gamma`'s own semantics if a version of egui here
/// has it; spelled out by hand since that method's presence varies by
/// egui version and this crate should not gate a small widget behind a
/// version check for one convenience function.
fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    egui::Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_color_returns_the_start_at_t_zero_and_end_at_t_one() {
        let a = egui::Color32::from_rgb(0, 0, 0);
        let b = egui::Color32::from_rgb(255, 255, 255);
        assert_eq!(lerp_color(a, b, 0.0), a);
        assert_eq!(lerp_color(a, b, 1.0), b);
    }

    #[test]
    fn lerp_color_is_the_midpoint_at_t_half() {
        let a = egui::Color32::from_rgb(0, 0, 0);
        let b = egui::Color32::from_rgb(200, 100, 50);
        let mid = lerp_color(a, b, 0.5);
        // Rounding, not exact halves -- checked with a tolerance rather
        // than exact equality.
        assert!((mid.r() as i32 - 100).abs() <= 1);
        assert!((mid.g() as i32 - 50).abs() <= 1);
        assert!((mid.b() as i32 - 25).abs() <= 1);
    }

    #[test]
    fn lerp_color_clamps_out_of_range_t() {
        let a = egui::Color32::from_rgb(0, 0, 0);
        let b = egui::Color32::from_rgb(255, 255, 255);
        assert_eq!(lerp_color(a, b, -1.0), a, "t below 0 must clamp to the start color");
        assert_eq!(lerp_color(a, b, 2.0), b, "t above 1 must clamp to the end color");
    }
}
