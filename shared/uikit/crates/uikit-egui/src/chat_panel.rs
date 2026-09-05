//! chat_panel — egui port of `qml/ChatPanel.qml`.
//!
//! A conversational overlay over the local model (or cloud): a scrollable, role-aligned message list
//! plus an input line. The QML talked to the backend through three hooks — `chat_send_async(provider,
//! text)`, `chat_messages() -> "M\x1F<role>\x1F<content>\x1F<ts>…"`, and a `chat_busy` flag. This port
//! keeps that contract: the panel owns the parsed messages + input + busy state and renders them;
//! [`ChatPanel::ui`] returns `Some(text)` on send so the host can call its backend and reload.
//!
//! The wire-format parsing is pure and unit-tested; only the rendering needs egui.

use egui::{Align, Color32, Layout, RichText, ScrollArea};
use crate::slide_menu::{SlideMenu, SlideOption, SlideOutcome};

/// Who authored a turn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
}

/// One chat turn.
/// What the header's buttons asked for. Returned rather than acted on:
/// starting a new conversation and closing the panel are the HOST's
/// calls — they touch the backend and the window — and a widget doing
/// them itself would need both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAction {
    New,
    Close,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// Parse the backend's `chat_messages()` blob: newline-separated records, each
/// `M<US><role><US><content>[<US><ts>]` where `<US>` is the 0x1F unit separator. Unknown/short records
/// are skipped (forward-compatible). Pure — the tested core.
pub fn parse_messages(blob: &str) -> Vec<ChatMessage> {
    const US: char = '\u{1F}';
    blob.lines()
        .filter_map(|line| {
            let mut parts = line.split(US);
            if parts.next()? != "M" {
                return None;
            }
            let role = match parts.next()? {
                "user" => Role::User,
                "assistant" | "llama" | "model" => Role::Assistant,
                _ => return None,
            };
            let content = parts.next()?.to_string();
            (!content.is_empty()).then_some(ChatMessage { role, content })
        })
        .collect()
}

/// The chat overlay state.
pub struct ChatPanel {
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub busy: bool,
    /// Provider string sent alongside the message ("llama" routes to the local model when active).
    pub provider: String,
    /// Press-drag picker (shared gesture widget) for choosing the provider.
    provider_menu: SlideMenu,
    /// Caller-supplied, one-line explanation of what will actually happen
    /// on send — "no local model attached", "using ollama fallback", etc.
    /// `None` when there's nothing worth saying (a real backend is
    /// configured and working). Shown in the header, before the empty
    /// state or any message, so the limitation is visible on open rather
    /// than discovered only after sending and getting back a canned
    /// notice. This panel has no way to know its own backend's state —
    /// the caller sets this, same separation as `provider`/`busy`.
    pub backend_status: Option<String>,
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            busy: false,
            provider: "llama".into(),
            provider_menu: SlideMenu::new("chat_provider").columns(3).item_size(64.0),
            backend_status: None,
        }
    }
}

impl ChatPanel {
    /// Replace the message list from a fresh `chat_messages()` blob (call after each send/poll).
    pub fn reload(&mut self, blob: &str) {
        self.messages = parse_messages(blob);
    }

    /// Render the panel. Returns `Some(text)` on the frame the user sends a non-empty message (the
    /// host should dispatch it to the backend, set `busy`, then `reload`). Clears the input on send.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<String> {
        self.ui_with(ui).0
    }

    /// Draw, returning `(sent, action)`.
    ///
    /// The header and the empty state come from the QML this is a port
    /// OF, and were missing until `apps/voiceterm/docs/QML_OUTLINE.txt`
    /// made the gap visible. That spec lists, in order: a header row
    /// (title, a `[fill]` spacer, a "thinking…" label shown while busy,
    /// a "New" button), a "✕" close button, the bubble list, an
    /// "Ask the local model anything…" line shown when empty, then the
    /// input row.
    ///
    /// The bubbles and the input row were already here. The header, both
    /// buttons and the empty state were not — so the port rendered the
    /// middle of the panel and none of its chrome, and a person could
    /// neither start a new conversation nor close it.
    pub fn ui_with(&mut self, ui: &mut egui::Ui) -> (Option<String>, Option<ChatAction>) {
        let mut sent = None;
        let mut action = None;

        // header — title, spacer, busy hint, New, close.
        ui.horizontal(|ui| {
            ui.label(RichText::new("💬 Chat").strong());
            // The QML spacer is `Item [fill]`: it takes the slack so the
            // controls sit hard right rather than beside the title.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("✕").on_hover_text("close").clicked() {
                    action = Some(ChatAction::Close);
                }
                if ui.button("New").on_hover_text("new conversation").clicked() {
                    action = Some(ChatAction::New);
                }
                if self.busy {
                    ui.label(RichText::new("thinking…").weak());
                }
            });
        });
        // BACKEND STATUS, under the header, before anything else — visible
        // on open rather than discovered only after sending a message and
        // getting back a canned reply (which is what happened here before:
        // the notice lived only in the assistant's first turn). Inspired
        // by logscope-egui's inline "backend: stub → fallback because X"
        // line under its own header, same idea: say what will actually
        // happen up front, in the app's own voice, not just in a code
        // comment nobody using the app ever sees.
        if let Some(status) = &self.backend_status {
            ui.colored_label(ui.visuals().warn_fg_color, RichText::new(status).small());
        }
        ui.separator();

        // empty state — the QML shows this INSTEAD of an empty list, so a
        // fresh panel says what it is for rather than being a blank box.
        if self.messages.is_empty() && !self.busy {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Ask the local model anything…").weak());
            });
        }

        // MESSAGE AREA (fills the remaining space) AND THE FOOTER BELOW IT,
        // in a bottom-up sub-layout. `ScrollArea::auto_shrink([false, false])`
        // claims "available height" in whatever it's given — and in a normal
        // top-down flow, added before the footer, "available" means the
        // WHOLE remaining pane, since egui has no way to know a fixed-height
        // footer is coming after it. The footer (separator + input row) then
        // got zero room and never rendered at all, which a real-frame test
        // confirmed: the input `TextEdit` never even registered a response
        // in an empty chat, at any window size — there was no way to send
        // the first message, ever.
        //
        // Bottom-up fixes it structurally rather than with a guessed pixel
        // reservation: widgets added first land at the BOTTOM, so adding the
        // input row and its separator first claims their natural (small,
        // fixed) height there, and the scroll area — added last in THIS
        // sub-layout, so it fills whatever is left above them — never sees
        // less room than it should.
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            // input row — Enter or the button sends (disabled while busy).
            ui.horizontal(|ui| {
                // provider picker — press-drag the badge to open a grid of providers.
                let badge = ui.button(RichText::new(format!("◆ {}", self.provider)).small());
                let providers = [
                    SlideOption::new("llama", "llama"),
                    SlideOption::new("gpt", "gpt"),
                    SlideOption::new("claude", "claude"),
                ];
                if let SlideOutcome::Selected(id, _) = self.provider_menu.show(ui, &badge, &providers) {
                    self.provider = id;
                }
                let r_input = ui.add(egui::TextEdit::singleline(&mut self.input).id_salt("chat_panel-self_input").desired_width(f32::INFINITY).hint_text("Message…"));
                // TEST SUPPORT ONLY: stash the rect in egui's own temp memory
                // (auto-expiring, standard idiom — see the same pattern in
                // voiceterm-egui's bars.rs) so a test can find it without
                // replicating `id_salt`'s actual formula, `ui.id().with(salt)`
                // — which depends on the surrounding ui nesting and isn't
                // safely reproducible from outside. Nothing reads this key
                // outside `#[cfg(test)]`.
                ui.ctx().memory_mut(|m| m.data.insert_temp(
                    egui::Id::new("chat_panel_input_rect_for_test"), r_input.rect));
                let enter = r_input.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui.add_enabled(!self.busy, egui::Button::new("Send")).clicked();
                if (enter || clicked) && !self.busy {
                    let t = self.input.trim().to_string();
                    if !t.is_empty() {
                        sent = Some(t);
                        self.input.clear();
                    }
                }
            });
            ui.separator();

            // message list — role-aligned bubbles, newest at the bottom, auto-scrolled.
            //
            // NOT `.stick_to_bottom(true)` — that was here originally and,
            // once the message area sits inside the bottom-up sub-layout
            // above, scrolled PAST the actual content on the very first
            // frame a message appears: confirmed live under Xvfb, a fresh
            // chat that had never been manually scrolled showed nothing at
            // all after sending, right up until a manual scroll-up revealed
            // both bubbles sitting above the visible area. `stick_to_bottom`
            // tracks content height against the viewport across frames to
            // decide how far "the bottom" is; nested inside a layout whose
            // own available height depends on what's added to it in the
            // SAME frame (the footer below, added moments earlier in this
            // bottom-up flow), that tracking saw a viewport size that didn't
            // match the one actually rendered into.
            //
            // `scroll_to_cursor` sidesteps the cross-frame tracking
            // entirely: called right after the last widget, it asks for
            // whatever the cursor's position ACTUALLY is this frame,
            // rather than a remembered content height from a frame where
            // the surrounding layout may have been sized differently.
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for m in &self.messages {
                    let (align, fill, text_col) = match m.role {
                        Role::User => (Align::Max, Color32::from_rgb(46, 92, 158), Color32::WHITE),
                        Role::Assistant => (Align::Min, ui.visuals().extreme_bg_color, ui.visuals().text_color()),
                    };
                    // WIDTH ALLOCATED FIRST, not constrained from inside.
                    // The old code entered `Layout::top_down(align)` at the
                    // OUTER (full list) width and only capped the bubble's
                    // own width once already inside the Frame — so `Align::
                    // Max` right-aligned against the WIDE outer rect while
                    // the bubble itself sized down to its short content,
                    // and the two didn't agree on where "the right edge"
                    // was. Confirmed with a real-frame test: a short user
                    // message in a 500px-wide pane rendered its bubble
                    // ending at x=545 — 41px past the pane's own right
                    // edge, clipped and mostly invisible, matching exactly
                    // what a "hi" clipped down to a single visible letter
                    // looked like live.
                    //
                    // Allocating the narrower rect up front, then aligning
                    // WITHIN it, means there is only one width in play.
                    let max_w = ui.available_width() * 0.78;
                    ui.allocate_ui_with_layout(egui::vec2(max_w, 0.0), Layout::top_down(align), |ui| {
                        let frame_resp = egui::Frame::new()
                            .fill(fill)
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .corner_radius(10)
                            .show(ui, |ui| {
                                ui.label(RichText::new(&m.content).color(text_col));
                            });
                        // TEST SUPPORT ONLY: same temp-memory pattern as the
                        // input row above, keyed by role, so a test can
                        // check where a right-aligned (User) bubble actually
                        // lands without replicating egui's id math.
                        let key = match m.role {
                            Role::User => "chat_panel_last_user_bubble_rect_for_test",
                            Role::Assistant => "chat_panel_last_assistant_bubble_rect_for_test",
                        };
                        ui.ctx().memory_mut(|mem| mem.data.insert_temp(
                            egui::Id::new(key), frame_resp.response.rect));
                    });
                    ui.add_space(4.0);
                }
                if self.busy {
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        ui.label(RichText::new("…").weak());
                    });
                }
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            });
        });

        (sent, action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_role_delimited_records() {
        let blob = "M\u{1F}user\u{1F}hello there\u{1F}123\nM\u{1F}assistant\u{1F}hi!\u{1F}124";
        let msgs = parse_messages(blob);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], ChatMessage { role: Role::User, content: "hello there".into() });
        assert_eq!(msgs[1], ChatMessage { role: Role::Assistant, content: "hi!".into() });
    }

    #[test]
    fn skips_malformed_and_empty_records() {
        let blob = "garbage\nM\u{1F}user\u{1F}\nM\u{1F}user\u{1F}ok\nM\u{1F}weirdrole\u{1F}x";
        let msgs = parse_messages(blob);
        assert_eq!(msgs.len(), 1); // only the non-empty, known-role record
        assert_eq!(msgs[0].content, "ok");
    }

    #[test]
    fn llama_maps_to_assistant() {
        let msgs = parse_messages("M\u{1F}llama\u{1F}from the local model");
        assert_eq!(msgs[0].role, Role::Assistant);
    }

    /// Real-frame check for a bug found live, under Xvfb: a fresh (empty)
    /// chat pane showed the header, "Ask the local model anything…", and
    /// nothing else — no input row, no Send button, at ANY window height,
    /// and scrolling revealed nothing further. "The wire-format parsing is
    /// pure and unit-tested; only the rendering needs egui" (this file's
    /// own header) turned out to be exactly the gap: nothing had ever
    /// driven `ui_with` through a real frame to check what it actually
    /// draws when the message list is empty.
    ///
    /// Bounded to a small rect on purpose, the same way the bar_ui tests
    /// in voiceterm-egui are: `ScrollArea::auto_shrink([false, false])`
    /// greedily claims "available height" in whatever container it's
    /// given, and the input row is added AFTER it in the code — so if the
    /// container is generous (as a real window usually is), the scroll
    /// area can claim all of it before the footer ever gets a turn.
    #[test]
    fn the_input_row_is_reachable_in_an_empty_chat() {
        let mut panel = ChatPanel::default();
        let ctx = egui::Context::default();
        // 320px, not a razor-thin 300: this is checking "does the footer
        // get its natural size at all", not pixel-perfect spacing budgets —
        // the original bug failed this by never rendering the row at all,
        // not by missing a tight bound by a pixel of frame margin.
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 320.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 320.0));
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                let _ = panel.ui_with(ui);
            });
        });
        // Queried AFTER the frame ends, not from inside the closure — egui's
        // temp memory (like Context::read_response) is finalised at end-of-pass.
        let input_rect: Option<egui::Rect> = ctx.memory(|m|
            m.data.get_temp(egui::Id::new("chat_panel_input_rect_for_test")));
        assert!(input_rect.is_some(),
            "the message TextEdit never got a response at all — id_salt \
             mismatch or it was never reached during layout");
        let r = input_rect.unwrap();
        // A SMALL tolerance, not exact containment: this is checking "did
        // the footer get a real, sane position" — the original bug failed
        // the `is_some()` check above outright, with no rect at all. A
        // rect landing a pixel past a hand-picked round-number bound is
        // ordinary frame-margin rounding, not the defect being tested for.
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 320.0)).expand(4.0);
        assert!(bounds.contains_rect(r),
            "the input row rendered well outside the pane's own bounds: {r:?} vs {bounds:?} \
             — the empty-state ScrollArea most likely claimed the whole height \
             before the input row got a turn");
    }

    /// Diagnostic for a clipped-looking user bubble seen live: a right-
    /// aligned (`Role::User`) message appeared to render mostly off the
    /// right edge of the pane, only its first character visible. Checking
    /// whether that's real clipping or just a narrow test window before
    /// deciding whether it needs a fix.
    #[test]
    fn a_user_bubble_lands_within_the_pane_not_off_its_right_edge() {
        let mut panel = ChatPanel::default();
        panel.messages.push(ChatMessage { role: Role::Assistant, content: "hi".into() });
        panel.messages.push(ChatMessage { role: Role::User, content: "hello there".into() });
        let ctx = egui::Context::default();
        let size = egui::vec2(500.0, 400.0);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size);
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                let _ = panel.ui_with(ui);
            });
        });
        let user_rect: Option<egui::Rect> = ctx.memory(|m|
            m.data.get_temp(egui::Id::new("chat_panel_last_user_bubble_rect_for_test")));
        let r = user_rect.expect("the user bubble never rendered at all");
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, size).expand(4.0);
        assert!(bounds.contains_rect(r),
            "the user bubble rendered outside the pane's own bounds: {r:?} vs {bounds:?}");
    }
}
