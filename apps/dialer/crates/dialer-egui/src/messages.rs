//! The messaging surface: a thread list, a conversation view, a composer,
//! and the two **handoff cards** (command suggestion, file offer/view) that
//! render a received [`Payload`] with the [`GuardPolicy`] deciding what the
//! user is even offered.
//!
//! The command card is the whole reason the guard exists: it never shows a
//! bare "Run" that fires on tap. What it shows depends on
//! [`GuardPolicy::for_command`] — copy-only when running is disabled, a
//! confirm-gated run when enabled, and a reject notice when the command text
//! can't be safely tokenised. The actual spawn is the host's job (returned as
//! [`MsgAction::RunConfirmed`]) so this module stays free of `std::process`.

use dialer_core::command_guard::{Decision, GuardPolicy};
use dialer_core::conversation::{Conversations, Dir, Thread};
use dialer_core::message::Payload;
use uikit_egui::{fmt, PaletteColors};

/// What the shell must do as a result of interacting with the messages view.
#[derive(Debug, Clone)]
pub enum MsgAction {
    /// Send this text/handoff to the open peer.
    Send { peer: String, payload: Payload },
    /// User asked to copy text to the clipboard.
    Copy(String),
    /// A command cleared the guard and (opt-in on) was confirmed: the host
    /// tokenises and spawns it. `tokens` is already guard-approved.
    RunConfirmed { command: String },
    /// User asked to open a file offer's uri or a confined view path.
    OpenUri(String),
    OpenPath(std::path::PathBuf),
    /// A view path was denied by the guard — the shell toasts this.
    ViewDenied(String),
}

/// Which pane the messages page is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagesView {
    /// The thread list.
    List,
    /// A single conversation with `peer`.
    Thread(String),
}

impl Default for MessagesView {
    fn default() -> Self {
        MessagesView::List
    }
}

/// Per-page UI state the shell owns.
pub struct MessagesState {
    pub view: MessagesView,
    pub compose: String,
    /// A command awaiting confirmation (command text), shown as a modal by the
    /// shell via the kit's ConfirmDialog.
    pub pending_run: Option<String>,
}

impl Default for MessagesState {
    fn default() -> Self {
        MessagesState { view: MessagesView::List, compose: String::new(), pending_run: None }
    }
}

/// The thread list. Returns a peer to open, if tapped.
pub fn thread_list(
    ui: &mut egui::Ui,
    convs: &Conversations,
    colors: &PaletteColors,
    epoch: u64,
) -> Option<String> {
    let mut open = None;
    let threads = convs.threads_by_recency();
    if threads.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No messages yet").color(colors.text_weak));
        });
        return None;
    }
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for t in threads {
            let resp = ui
                .horizontal(|ui| {
                    let unread = t.unread();
                    let name = peer_display(&t.peer);
                    let title = if unread > 0 {
                        egui::RichText::new(name).strong().color(colors.text)
                    } else {
                        egui::RichText::new(name).color(colors.text)
                    };
                    ui.label(title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(m) = t.last() {
                            let ago = fmt::relative_time(
                                epoch.saturating_sub(m.ts) as i64,
                                fmt::Style::Bare,
                            );
                            ui.label(egui::RichText::new(ago).small().color(colors.text_weak));
                        }
                        if unread > 0 {
                            ui.label(
                                egui::RichText::new(format!("● {unread}"))
                                    .small()
                                    .color(colors.accent),
                            );
                        }
                    });
                })
                .response;
            if let Some(m) = t.last() {
                ui.label(egui::RichText::new(m.payload.preview()).small().color(colors.text_weak));
            }
            if resp.interact(egui::Sense::click()).clicked() {
                open = Some(t.peer.clone());
            }
            ui.separator();
        }
    });
    open
}

/// A single conversation. Emits actions (send / handoff interactions).
pub fn thread_view(
    ui: &mut egui::Ui,
    thread: Option<&Thread>,
    peer: &str,
    state: &mut MessagesState,
    guard: &GuardPolicy,
    colors: &PaletteColors,
    out: &mut Vec<MsgAction>,
) {
    ui.horizontal(|ui| {
        if ui.button("‹ Messages").clicked() {
            state.view = MessagesView::List;
        }
        ui.heading(peer_display(peer));
    });
    ui.separator();

    egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
        let Some(thread) = thread else { return };
        for m in &thread.messages {
            let mine = m.dir == Dir::Out;
            let layout = if mine {
                egui::Layout::right_to_left(egui::Align::Min)
            } else {
                egui::Layout::left_to_right(egui::Align::Min)
            };
            ui.with_layout(layout, |ui| {
                bubble(ui, &m.payload, mine, peer, guard, colors, out);
            });
            ui.add_space(4.0);
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        let te = egui::TextEdit::singleline(&mut state.compose)
            .hint_text("Message…")
            .desired_width(ui.available_width() - 70.0);
        let resp = ui.add(te);
        let send = ui.button("Send").clicked()
            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if send && !state.compose.trim().is_empty() {
            // A composed message may itself be a `run:`/`file:` handoff — parse
            // so the sender's intent is structured on the wire.
            let payload = dialer_core::message::parse_sms(state.compose.trim());
            out.push(MsgAction::Send { peer: peer.to_string(), payload });
            state.compose.clear();
        }
    });
}

fn bubble(
    ui: &mut egui::Ui,
    payload: &Payload,
    mine: bool,
    peer: &str,
    guard: &GuardPolicy,
    colors: &PaletteColors,
    out: &mut Vec<MsgAction>,
) {
    let fill = if mine { colors.accent_soft } else { colors.bg_card };
    egui::Frame::new().fill(fill).inner_margin(8.0).corner_radius(8.0).show(ui, |ui| {
        ui.set_max_width(280.0);
        match payload {
            Payload::Text { body } => {
                ui.label(egui::RichText::new(body).color(colors.text));
            }
            Payload::CommandSuggestion { command, cwd, note } => {
                command_card(ui, command, cwd.as_deref(), note.as_deref(), mine, peer, guard, colors, out);
            }
            Payload::FileOffer { name, mime, size, uri } => {
                ui.label(egui::RichText::new(format!("📎 {name}")).strong().color(colors.text));
                let meta = match (mime, size) {
                    (Some(m), Some(s)) => format!("{m} · {} B", s),
                    (Some(m), None) => m.clone(),
                    (None, Some(s)) => format!("{s} B"),
                    _ => String::new(),
                };
                if !meta.is_empty() {
                    ui.label(egui::RichText::new(meta).small().color(colors.text_weak));
                }
                if ui.button("Open").clicked() {
                    out.push(MsgAction::OpenUri(uri.clone()));
                }
            }
            Payload::FileView { path } => {
                ui.label(egui::RichText::new(format!("👁 {path}")).color(colors.text));
                if ui.button("Open here").clicked() {
                    match guard.resolve_view(path) {
                        Some(p) => out.push(MsgAction::OpenPath(p)),
                        None => out.push(MsgAction::ViewDenied(path.clone())),
                    }
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn command_card(
    ui: &mut egui::Ui,
    command: &str,
    cwd: Option<&str>,
    note: Option<&str>,
    mine: bool,
    peer: &str,
    guard: &GuardPolicy,
    colors: &PaletteColors,
    out: &mut Vec<MsgAction>,
) {
    ui.label(egui::RichText::new("Suggested command").small().color(colors.text_weak));
    ui.label(egui::RichText::new(command).monospace().color(colors.text));
    if let Some(c) = cwd {
        ui.label(egui::RichText::new(format!("in {c}")).small().color(colors.text_weak));
    }
    if let Some(n) = note {
        ui.label(egui::RichText::new(n).small().italics().color(colors.text_weak));
    }
    ui.horizontal(|ui| {
        if ui.button("Copy").clicked() {
            out.push(MsgAction::Copy(command.to_string()));
        }
        // Only inbound commands are runnable; running your own suggestion back
        // makes no sense and would be a footgun.
        if mine {
            return;
        }
        let sender = format!("matrix:{}", peer.trim_start_matches("matrix:")); // display id
        let sender = if peer.starts_with("tel:") { peer.to_string() } else { sender };
        match guard.for_command(&sender, command) {
            Decision::CopyOnly => {
                ui.label(
                    egui::RichText::new("running disabled (Setup ▸ enable)")
                        .small()
                        .color(colors.text_weak),
                );
            }
            Decision::Reject(why) => {
                ui.label(egui::RichText::new(format!("can't run: {why}")).small().color(colors.danger));
            }
            Decision::RunWithConfirm => {
                if ui
                    .add(egui::Button::new(egui::RichText::new("Run…").color(colors.warning)))
                    .clicked()
                {
                    // Defer to the shell's ConfirmDialog; store the command.
                    out.push(MsgAction::RunConfirmed { command: command.to_string() });
                }
            }
            Decision::RunAllowed => {
                if ui.button("Run").clicked() {
                    out.push(MsgAction::RunConfirmed { command: command.to_string() });
                }
            }
        }
    });
}

/// "tel:+15551234" → "+15551234"; "matrix:@a:x.org" → "@a:x.org".
pub fn peer_display(peer: &str) -> String {
    peer.split_once(':').map(|(_, rest)| rest.to_string()).unwrap_or_else(|| peer.to_string())
}

/// The whole messages page: routes between list and thread. Returns actions.
pub fn messages_page(
    ui: &mut egui::Ui,
    convs: &Conversations,
    state: &mut MessagesState,
    guard: &GuardPolicy,
    colors: &PaletteColors,
    epoch: u64,
) -> Vec<MsgAction> {
    let mut out = Vec::new();
    match state.view.clone() {
        MessagesView::List => {
            if let Some(peer) = thread_list(ui, convs, colors, epoch) {
                state.view = MessagesView::Thread(peer);
            }
        }
        MessagesView::Thread(peer) => {
            let thread = convs.thread(&peer);
            thread_view(ui, thread, &peer, state, guard, colors, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_display_strips_transport_prefix() {
        assert_eq!(peer_display("tel:+15551234"), "+15551234");
        assert_eq!(peer_display("matrix:@a:x.org"), "@a:x.org");
        assert_eq!(peer_display("weird"), "weird");
    }
}
