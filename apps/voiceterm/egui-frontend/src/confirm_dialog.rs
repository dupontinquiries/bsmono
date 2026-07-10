//! confirm_dialog — egui port of `qml/ConfirmDialog.qml`.
//!
//! A modal confirmation overlay for destructive actions (overwriting a saved graph, deleting a
//! workspace, …). Faithful to the QML: a dimmed backdrop that cancels on click-outside, three choices
//! (the primary/destructive action, "keep — don't apply yet", and "discard changes"), a countdown, and
//! a border that animates from full width to zero as the timeout runs out. The timeout defaults to
//! "discard" per the original spec ("do not apply changes").
//!
//! The timeout arithmetic is factored into pure helpers so the resolution logic is unit-tested without
//! a running egui context.

use egui::{Align2, Color32, Context, Id, Key, Order, Stroke};

/// What a timeout should do if the user doesn't choose.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TimeoutAction {
    #[default]
    Discard,
    Cancel,
    Confirm,
}

/// The dialog's result. `Pending` while it's still open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Pending,
    Confirmed,
    Cancelled,
    Discarded,
}

/// Map a fired timeout to the outcome its `TimeoutAction` implies. Pure — the tested core.
pub fn timeout_outcome(action: TimeoutAction) -> Outcome {
    match action {
        TimeoutAction::Discard => Outcome::Discarded,
        TimeoutAction::Cancel => Outcome::Cancelled,
        TimeoutAction::Confirm => Outcome::Confirmed,
    }
}

/// Fraction of the timeout still remaining, clamped to `[0, 1]` — drives the shrinking border. Pure.
pub fn remaining_fraction(elapsed_secs: f32, timeout_secs: f32) -> f32 {
    if timeout_secs <= 0.0 {
        return 0.0;
    }
    ((timeout_secs - elapsed_secs) / timeout_secs).clamp(0.0, 1.0)
}

/// A reusable confirmation modal. Construct once, then call [`ConfirmDialog::show`] every frame while
/// it's open; act on any non-`Pending` [`Outcome`] and stop showing it.
pub struct ConfirmDialog {
    pub title: String,
    pub body: String,
    pub confirm_label: String,
    pub timeout_secs: f32,
    pub timeout_action: TimeoutAction,
    opened_at: Option<f64>,
}

impl ConfirmDialog {
    pub fn new(title: impl Into<String>, body: impl Into<String>, confirm_label: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            confirm_label: confirm_label.into(),
            timeout_secs: 10.0,
            timeout_action: TimeoutAction::Discard,
            opened_at: None,
        }
    }
    pub fn with_timeout(mut self, secs: f32) -> Self {
        self.timeout_secs = secs;
        self
    }
    pub fn with_timeout_action(mut self, action: TimeoutAction) -> Self {
        self.timeout_action = action;
        self
    }

    /// Render the modal for one frame and return the current [`Outcome`]. Resets itself once resolved
    /// so the same instance can be reused for the next confirmation.
    pub fn show(&mut self, ctx: &Context) -> Outcome {
        let now = ctx.input(|i| i.time);
        let opened = *self.opened_at.get_or_insert(now);
        let elapsed = (now - opened) as f32;
        let frac = remaining_fraction(elapsed, self.timeout_secs);
        let remaining = (self.timeout_secs - elapsed).max(0.0);
        ctx.request_repaint(); // keep the countdown animating

        let mut outcome = Outcome::Pending;

        // Dimmed backdrop; a click anywhere on it cancels (QML "click outside cancels").
        let screen = ctx.screen_rect();
        egui::Area::new(Id::new("confirm_backdrop"))
            .order(Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let resp = ui.allocate_response(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(140));
                if resp.clicked() {
                    outcome = Outcome::Cancelled;
                }
            });

        // The dialog itself.
        egui::Area::new(Id::new("confirm_dialog"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(18)).show(ui, |ui| {
                    ui.set_max_width(380.0);
                    ui.heading(&self.title);
                    ui.add_space(6.0);
                    ui.label(&self.body);
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button(&self.confirm_label).clicked() {
                            outcome = Outcome::Confirmed;
                        }
                        if ui.button("Don't apply yet").clicked() {
                            outcome = Outcome::Cancelled;
                        }
                        if ui.button("Discard changes").clicked() {
                            outcome = Outcome::Discarded;
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(format!("{}s", remaining.ceil() as i32));

                    // Animated border: a top edge line that shrinks from full width to zero.
                    let r = ui.min_rect().expand(8.0);
                    let w = r.width() * frac;
                    let x0 = r.center().x - w / 2.0;
                    let accent = ui.visuals().selection.bg_fill;
                    ui.painter().line_segment(
                        [egui::pos2(x0, r.top()), egui::pos2(x0 + w, r.top())],
                        Stroke::new(2.0, accent),
                    );
                });
            });

        // Esc cancels.
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            outcome = Outcome::Cancelled;
        }
        // Timeout fires the configured action.
        if remaining <= 0.0 && outcome == Outcome::Pending {
            outcome = timeout_outcome(self.timeout_action);
        }
        if outcome != Outcome::Pending {
            self.opened_at = None; // ready for reuse
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_action_maps_to_outcome() {
        assert_eq!(timeout_outcome(TimeoutAction::Discard), Outcome::Discarded);
        assert_eq!(timeout_outcome(TimeoutAction::Cancel), Outcome::Cancelled);
        assert_eq!(timeout_outcome(TimeoutAction::Confirm), Outcome::Confirmed);
        assert_eq!(TimeoutAction::default(), TimeoutAction::Discard); // spec: default to discard
    }

    #[test]
    fn remaining_fraction_counts_down_and_clamps() {
        assert_eq!(remaining_fraction(0.0, 10.0), 1.0);
        assert_eq!(remaining_fraction(5.0, 10.0), 0.5);
        assert_eq!(remaining_fraction(10.0, 10.0), 0.0);
        assert_eq!(remaining_fraction(15.0, 10.0), 0.0); // clamped, never negative
        assert_eq!(remaining_fraction(1.0, 0.0), 0.0); // zero timeout is a no-op border
    }
}
