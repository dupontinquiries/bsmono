//! chat_panel — the in-game build chat, as a body function.
//!
//! Follows the `live_menu` pattern: the BODY is library code so it can
//! be drawn identically by the real renderer and by a headless
//! snapshot, and unit-tested without a window. The renderer supplies
//! only the chrome (glass window, anchor) and maps the returned
//! actions onto the game.
//!
//! Layout is a bottom-anchored bar, the way a game chat looks:
//!
//! ```text
//!   +--------------------------------------------------+
//!   | built 220 blocks in 4 batches                     |  <- log, last N
//!   | 70 blocks planned -- /commit to build             |
//!   +--------------------------------------------------+
//!   | > /build a small watchtower_                      |  <- input
//!   +--------------------------------------------------+
//!   | /build  <what you want>   Describe a structure... |  <- card
//!   +--------------------------------------------------+
//! ```
//!
//! The card under the input is the discoverability layer: typing `/`
//! shows every command without a manual.

/// What the panel needs to draw, and what the game owns.
pub struct ChatPanelState<'a> {
    /// Current input buffer (owned by `voxel_tools::chatbox::ChatBox`).
    pub buffer: &'a str,
    /// Recent log lines, oldest first. The panel shows the tail.
    pub log: &'a [String],
    /// Card for the verb being typed: (command, signature, summary).
    pub card: Option<(&'a str, &'a str, &'a str)>,
    /// Other commands matching the buffer, for the dropdown.
    pub matches: &'a [(&'a str, &'a str)],
    /// Focus line: tier and anchor, so the player can see where a
    /// build would land before committing to typing one.
    pub focus_hint: Option<&'a str>,
    /// True while a plan is pending, so the bar can say so.
    pub pending_blocks: Option<usize>,
    /// Loaded model name, or None.
    pub model: Option<&'a str>,
}

/// What the player did. The renderer turns these into engine calls.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ChatPanelOut {
    /// Clicked the commit affordance.
    pub commit: bool,
    /// Clicked cancel / the X.
    pub cancel: bool,
    /// Clicked a suggestion; the string is the full replacement line.
    pub pick: Option<String>,
}

/// Number of log lines shown. Enough to see a preview echo's tail
/// without covering the world.
pub const LOG_LINES: usize = 8;

/// Draw the panel body. Pure: no engine access, no side effects.
pub fn chat_panel_body(ui: &mut egui::Ui, s: &ChatPanelState<'_>) -> ChatPanelOut {
    let mut out = ChatPanelOut::default();
    ui.set_min_width(560.0);

    // ── status line ──────────────────────────────────────────────
    ui.horizontal(|ui| {
        match s.model {
            Some(m) => ui.label(egui::RichText::new(m).small().weak()),
            None => ui.label(egui::RichText::new("no model — /model").small()
                .color(egui::Color32::from_rgb(200, 150, 90))),
        };
        if let Some(f) = s.focus_hint {
            ui.separator();
            ui.label(egui::RichText::new(f).small().weak());
        }
        if let Some(n) = s.pending_blocks {
            ui.separator();
            ui.label(egui::RichText::new(format!("{n} blocks pending")).small()
                .color(egui::Color32::from_rgb(140, 200, 255)));
            if ui.small_button("build it").clicked() { out.commit = true; }
            if ui.small_button("cancel").clicked() { out.cancel = true; }
        }
    });
    ui.separator();

    // ── log tail ─────────────────────────────────────────────────
    let start = s.log.len().saturating_sub(LOG_LINES);
    egui::ScrollArea::vertical()
        .max_height(150.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &s.log[start..] {
                // A preview echo is multi-line; show it as one block so
                // the arithmetic stays aligned.
                for l in line.lines() {
                    ui.label(egui::RichText::new(l).monospace().small());
                }
            }
        });

    // ── input line ───────────────────────────────────────────────
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(">").monospace().strong());
        // The buffer is owned by ChatBox, not by egui: the game already
        // routes keys through a state machine, and two sources of truth
        // for one string is how a cursor ends up in the wrong place.
        ui.label(egui::RichText::new(s.buffer).monospace());
        // a blinking-ish caret without needing animation state
        ui.label(egui::RichText::new("_").monospace().weak());
    });

    // ── card / completions ───────────────────────────────────────
    if let Some((cmd, sig, summary)) = s.card {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(cmd).monospace().strong());
            ui.label(egui::RichText::new(sig).monospace().small().weak());
        });
        ui.label(egui::RichText::new(summary).small());
    }
    if s.matches.len() > 1 {
        ui.separator();
        egui::ScrollArea::vertical().max_height(90.0).id_salt("chat_matches").show(ui, |ui| {
            for (cmd, summary) in s.matches {
                let r = ui.selectable_label(false,
                    egui::RichText::new(format!("{cmd:9} {summary}")).small());
                if r.clicked() { out.pick = Some(format!("{cmd} ")); }
            }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state<'a>(buffer: &'a str, log: &'a [String]) -> ChatPanelState<'a> {
        ChatPanelState {
            buffer, log, card: None, matches: &[], focus_hint: None,
            pending_blocks: None, model: None,
        }
    }

    /// egui's test harness: build a context, run the body, inspect the
    /// output. Same approach `live_menu` uses.
    fn run(s: &ChatPanelState<'_>) -> (ChatPanelOut, egui::FullOutput) {
        let ctx = egui::Context::default();
        let mut out = ChatPanelOut::default();
        let full = ctx.run(egui::RawInput::default(), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                out = chat_panel_body(ui, s);
            });
        });
        (out, full)
    }

    #[test]
    fn draws_without_panicking_when_empty() {
        let log: Vec<String> = Vec::new();
        let (out, _) = run(&state("", &log));
        assert_eq!(out, ChatPanelOut::default());
    }

    #[test]
    fn shows_only_the_log_tail() {
        let log: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let (_, full) = run(&state("", &log));
        let text = collect_text(&full);
        assert!(text.contains("line 39"), "newest must be visible");
        assert!(!text.contains("line 0"), "oldest must be trimmed: shows {LOG_LINES}");
    }

    #[test]
    fn the_buffer_is_rendered_verbatim() {
        let log: Vec<String> = Vec::new();
        let (_, full) = run(&state("/build a Small tower", &log));
        assert!(collect_text(&full).contains("/build a Small tower"), "case preserved");
    }

    #[test]
    fn a_pending_plan_offers_build_it() {
        let log: Vec<String> = Vec::new();
        let mut s = state("", &log);
        s.pending_blocks = Some(220);
        let (_, full) = run(&s);
        let text = collect_text(&full);
        assert!(text.contains("220 blocks pending"));
        assert!(text.contains("build it") && text.contains("cancel"));
    }

    #[test]
    fn no_model_is_called_out_where_the_player_will_see_it() {
        let log: Vec<String> = Vec::new();
        let (_, full) = run(&state("", &log));
        assert!(collect_text(&full).contains("no model"));
    }

    #[test]
    fn the_card_explains_the_verb_being_typed() {
        let log: Vec<String> = Vec::new();
        let mut s = state("/anim ph", &log);
        s.card = Some(("/anim", "/anim <ground-up|physics|random|instant>",
                       "How builds reveal."));
        let (_, full) = run(&s);
        let text = collect_text(&full);
        assert!(text.contains("/anim <ground-up"));
        assert!(text.contains("How builds reveal."));
    }

    fn collect_text(full: &egui::FullOutput) -> String {
        let mut s = String::new();
        for p in &full.shapes {
            if let egui::epaint::Shape::Text(t) = &p.shape {
                s.push_str(t.galley.text());
                s.push('\n');
            }
        }
        s
    }
}
