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
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            busy: false,
            provider: "llama".into(),
            provider_menu: SlideMenu::new("chat_provider").columns(3).item_size(64.0),
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
        let mut sent = None;

        // message list — role-aligned bubbles, newest at the bottom, auto-scrolled.
        ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            for m in &self.messages {
                let (align, fill, text_col) = match m.role {
                    Role::User => (Align::Max, Color32::from_rgb(46, 92, 158), Color32::WHITE),
                    Role::Assistant => (Align::Min, ui.visuals().extreme_bg_color, ui.visuals().text_color()),
                };
                ui.with_layout(Layout::top_down(align), |ui| {
                    egui::Frame::none()
                        .fill(fill)
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .corner_radius(10)
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width() * 0.78);
                            ui.label(RichText::new(&m.content).color(text_col));
                        });
                });
                ui.add_space(4.0);
            }
            if self.busy {
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.label(RichText::new("…").weak());
                });
            }
        });

        // input row — Enter or the button sends (disabled while busy).
        ui.separator();
        ui.horizontal(|ui| {
            // provider picker — press-drag the badge to open a grid of providers.
            let badge = ui.button(RichText::new(format!("◆ {}", self.provider)).small());
            let providers = [
                SlideOption::new("llama", "llama"),
                SlideOption::new("gpt", "gpt"),
                SlideOption::new("claude", "claude"),
            ];
            if let SlideOutcome::Selected(id) = self.provider_menu.show(ui, &badge, &providers) {
                self.provider = id;
            }
            let enter =
                ui.add(egui::TextEdit::singleline(&mut self.input).desired_width(f32::INFINITY).hint_text("Message…"))
                    .lost_focus()
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

        sent
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
}
