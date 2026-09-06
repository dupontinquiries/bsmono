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
    /// Seconds since each character in `buffer` was typed, oldest
    /// first. Same length as `buffer.chars()` or empty (no animation).
    /// Drives the pop-in: a fresh letter lands large and bright and
    /// settles over ~120 ms, the way a block does when it is placed.
    pub char_ages: &'a [f32],
    /// Wall-clock seconds, for the cursor blink.
    pub now: f64,
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
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(egui::RichText::new("> ").monospace().strong());
        // The buffer is owned by ChatBox, not by egui: the game already
        // routes keys through a state machine, and two sources of truth
        // for one string is how a cursor ends up in the wrong place.
        //
        // Each character is its own label so it can carry its own age:
        // a fresh letter is drawn larger and brighter and eases to rest,
        // the way a placed block lands. In the voxel font, which is what
        // makes it read as this game rather than a terminal.
        let base = 15.0;
        for (i, ch) in s.buffer.chars().enumerate() {
            let age = s.char_ages.get(i).copied().unwrap_or(1.0);
            let t = (age / 0.12).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - t) * (1.0 - t);          // ease-out
            let size = base + (1.0 - ease) * 7.0;             // 22 -> 15
            let bright = (0.75 + 0.25 * ease) as f32;
            let col = egui::Color32::from_rgb(
                (255.0 * bright) as u8, (240.0 * bright) as u8, (210.0 * bright) as u8);
            let mut text = egui::RichText::new(ch.to_string()).size(size).color(col);
            #[cfg(feature = "unendlich-ui")]
            { text = text.font(unendlich_ui::menu::blocks_font(size)); }
            #[cfg(not(feature = "unendlich-ui"))]
            { text = text.monospace(); }
            ui.label(text);
        }
        // BLOCK cursor: a PAINTED rectangle, static.
        //
        // Not a glyph. U+2588 FULL BLOCK is not in the voxel font, so
        // it rendered as the font's missing-glyph "?" -- which is what
        // the screenshot showed. A filled rect needs no glyph and is a
        // block in every font. And not blinking: a cursor that blinks
        // is a cursor that is sometimes not there, and the flicker at
        // this frame rate read as a fault rather than a blink.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(base * 0.6, base * 1.1),
                                                egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::from_rgb(255, 240, 210));
        // Ask for a redraw only while letters are still settling; a
        // static cursor needs none, and a standing 33 ms repaint request
        // is what made the old blink stutter at the game's frame rate.
        if s.char_ages.iter().any(|a| *a < 0.15) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        }
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
            pending_blocks: None, model: None, char_ages: &[], now: 0.0,
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
        // One label per character now, so the text is gathered back
        // from all of them.
        let log: Vec<String> = Vec::new();
        let (_, full) = run(&state("/build a Small tower", &log));
        let text: String = collect_text(&full).chars().filter(|c| *c != '\n').collect();
        assert!(text.contains("/build a Small tower"), "case preserved: {text}");
    }

    #[test]
    fn a_fresh_letter_is_drawn_larger_than_a_settled_one() {
        // The pop-in: age 0 lands large, age 1s is at rest.
        let log: Vec<String> = Vec::new();
        let ages = [1.0f32, 0.0];
        let mut s = state("ab", &log);
        s.char_ages = &ages;
        let (_, full) = run(&s);
        let sizes: Vec<f32> = full.shapes.iter().filter_map(|p| match &p.shape {
            egui::epaint::Shape::Text(t) if t.galley.text().len() == 1
                => Some(t.galley.rect.height()),
            _ => None,
        }).collect();
        assert!(sizes.len() >= 2, "{sizes:?}");
        let (old, fresh) = (sizes[sizes.len() - 2], sizes[sizes.len() - 1]);
        assert!(fresh > old, "fresh {fresh} must be larger than settled {old}");
    }

    #[test]
    fn the_cursor_is_a_painted_block_not_a_glyph() {
        // U+2588 is not in the voxel font and rendered as "?"; the
        // cursor is a filled rect now, which needs no glyph.
        let log: Vec<String> = Vec::new();
        let (_, full) = run(&state("ab", &log));
        let text = collect_text(&full);
        assert!(!text.contains('\u{2588}') && !text.contains('?') && !text.contains('_'),
                "no cursor glyph of any kind: {text}");
        let rects = full.shapes.iter().filter(|p| matches!(&p.shape,
            egui::epaint::Shape::Rect(r) if r.fill == egui::Color32::from_rgb(255, 240, 210))).count();
        assert!(rects >= 1, "a filled rect in the cursor colour");
    }

    #[test]
    fn the_cursor_does_not_blink() {
        let log: Vec<String> = Vec::new();
        let count = |now: f64| {
            let mut st = state("", &log);
            st.now = now;
            let (_, full) = run(&st);
            full.shapes.iter().filter(|p| matches!(&p.shape,
                egui::epaint::Shape::Rect(r) if r.fill == egui::Color32::from_rgb(255, 240, 210))).count()
        };
        assert_eq!(count(0.0), count(0.3), "same cursor at every instant");
        assert!(count(0.0) >= 1);
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
