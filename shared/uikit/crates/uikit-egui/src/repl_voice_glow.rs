//! repl_voice_glow — egui port of `qml/ReplVoiceGlow.qml`.
//!
//! A quiet companion card that appears when you drop into a REPL/debugger (gdb, python, sqlite, …),
//! inviting you to just say what you want. It fades in, breathes a soft halo, shows a context-specific
//! invite, and dismisses on Esc or click. The invite text is chosen purely (tested); the fade + halo
//! are driven by the egui clock.

use egui::{Align2, Color32, Id, Order, RichText};

/// The invite line for a REPL context (falls back to a generic prompt).
pub fn invite_for(context: &str) -> &'static str {
    match context {
        "gdb" | "lldb" => "Chasing a bug? Talk me through it — just say what you want to do.",
        "python" => "REPL's open. Say it out loud and I'll type it.",
        "sqlite" => "Poking at the database? Ask me in plain words.",
        "redis" => "Talk to Redis out loud — I'll translate.",
        _ => "Voice is on — say a command and I'll run it.",
    }
}

/// The glow companion.
#[derive(Default)]
pub struct ReplVoiceGlow {
    /// The host sets this true while a REPL is focused and voice is on.
    pub active: bool,
    /// REPL kind (drives the invite line).
    pub context: String,
    dismissed: bool,
}

impl ReplVoiceGlow {
    /// Call when the REPL context changes so a fresh glow can appear.
    pub fn reset(&mut self, context: impl Into<String>) {
        self.context = context.into();
        self.dismissed = false;
    }

    /// Render the glow if active and not dismissed. Fades with `animate_bool`; the halo breathes on a
    /// slow sine. Esc or a click on the card dismisses it. Returns true on the frame it's dismissed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let visible = self.active && !self.dismissed;
        // 320ms OutCubic fade — matches the QML `Behavior on opacity` instead of a linear snap.
        let fade = crate::motion::animate_eased(
            ctx,
            Id::new("repl_glow_fade"),
            visible,
            0.32,
            crate::motion::ease::out_cubic,
        );
        if fade <= 0.01 {
            return false;
        }
        ctx.request_repaint(); // keep the halo breathing

        let t = ctx.input(|i| i.time) as f32;
        // Gentle eased breathing (SequentialAnimation feel), not a bare abs-sine.
        let breath = 0.5 + 0.5 * (t * 1.4).sin(); // 0..1
        let halo = 0.16 + 0.16 * crate::motion::ease::out_sine(breath);
        // Rise softly into the corner as it fades in (QML "glows into the corner").
        let rise = (1.0 - fade) * 12.0;
        let mut just_dismissed = false;

        egui::Area::new(Id::new("repl_voice_glow"))
            .order(Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, [-16.0, -16.0 + rise])
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                let accent = ui.visuals().selection.bg_fill;
                let halo_col = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), (halo * 255.0) as u8);
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgb(18, 23, 31))
                    .stroke(egui::Stroke::new(1.5, halo_col))
                    .corner_radius(14)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.label(RichText::new("🎙 voice").small().color(accent));
                        ui.add_space(4.0);
                        ui.label(invite_for(&self.context));
                        ui.add_space(6.0);
                        if ui.small_button("dismiss").clicked() {
                            just_dismissed = true;
                        }
                    });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            just_dismissed = true;
        }
        if just_dismissed {
            self.dismissed = true;
        }
        just_dismissed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invites_are_context_specific_with_a_fallback() {
        assert!(invite_for("gdb").contains("bug"));
        assert_eq!(invite_for("gdb"), invite_for("lldb")); // debuggers share the line
        assert!(invite_for("python").contains("REPL"));
        assert!(invite_for("sqlite").contains("database"));
        assert!(invite_for("unknown-repl").contains("Voice is on")); // fallback
    }

    #[test]
    fn reset_clears_dismissal() {
        let mut g = ReplVoiceGlow::default();
        g.dismissed = true;
        g.reset("python");
        assert!(!g.dismissed);
        assert_eq!(g.context, "python");
    }
}
