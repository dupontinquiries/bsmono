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
use dialer_core::conversation::{Conversations, Dir, Message, Thread};
use dialer_core::gesture::{EdgePop, PopConfig, PopResult};
use dialer_core::message::{Payload, Tapback};
use uikit_egui::{fmt, PaletteColors};

/// What the shell must do as a result of interacting with the messages view.
#[derive(Debug, Clone)]
pub enum MsgAction {
    /// Long-press Tapback: toggle a reaction on a message (peer, dir, ts).
    React { peer: String, dir: Dir, ts: u64, tapback: Tapback },
    /// Interactive edge-pop committed: leave the thread.
    PopToList,
    /// Quote-reply: prefill the composer.
    Reply(String),
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
    /// In-progress interactive edge-pop for the open thread.
    pub pop: Option<EdgePop>,
    /// The message (dir, ts) whose long-press menu + Tapback bar is open.
    pub menu_for: Option<(Dir, u64)>,
    /// Live "drag conversation left to peek timestamps" offset, px.
    pub peek: f32,
}

impl Default for MessagesState {
    fn default() -> Self {
        MessagesState {
            view: MessagesView::List,
            compose: String::new(),
            pending_run: None,
            pop: None,
            menu_for: None,
            peek: 0.0,
        }
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

/// A single conversation with iOS interactions:
/// * **interactive edge-pop** — drag from the left edge and the whole thread
///   tracks the finger, revealing the list behind; release past the threshold
///   (or a flick) leaves.
/// * **swipe conversation left** — reveal per-message timestamps (iMessage).
/// * per-bubble **long-press** menu + Tapback bar (in [`bubble`]).
#[allow(clippy::too_many_arguments)]
pub fn thread_view(
    ui: &mut egui::Ui,
    thread: Option<&Thread>,
    peer: &str,
    state: &mut MessagesState,
    guard: &GuardPolicy,
    colors: &PaletteColors,
    out: &mut Vec<MsgAction>,
) {
    let surface = ui.available_rect_before_wrap();
    let width = surface.width().max(1.0);

    // --- interactive edge-pop -------------------------------------------
    // A full-surface sense picks up the edge drag; the message list lives
    // inside the translated child so its own scroll still works when the drag
    // didn't start at the edge.
    let pop = state.pop.get_or_insert_with(|| EdgePop::new(width, PopConfig::default()));
    let bg = ui.interact(surface, ui.id().with("thread-pop"), egui::Sense::drag());
    if bg.drag_started() {
        if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
            pop.begin(p.x - surface.left());
        }
    }
    if bg.dragged() && pop.is_active() {
        pop.drag(bg.drag_delta().x);
    }
    if bg.drag_stopped() && pop.is_active() {
        let vx = ui.input(|i| i.pointer.velocity().x) / 1000.0; // px/s → px/ms
        if pop.finish(vx) == PopResult::Commit {
            out.push(MsgAction::PopToList);
        }
    }
    let pop_offset = pop.offset();
    let progress = pop.progress();

    // The list behind, dimmed, so the pop reveals "the previous screen".
    if pop_offset > 0.5 {
        ui.painter().rect_filled(
            surface,
            0.0,
            colors.bg_base.gamma_multiply(1.0),
        );
        ui.painter().text(
            surface.left_top() + egui::vec2(16.0, 16.0),
            egui::Align2::LEFT_TOP,
            "‹ Messages",
            egui::FontId::proportional(16.0),
            colors.text_weak.gamma_multiply(progress),
        );
    }

    // Everything below is drawn into a child shifted right by the pop offset.
    let content_rect = surface.translate(egui::vec2(pop_offset, 0.0));
    let builder = egui::UiBuilder::new().max_rect(content_rect);
    ui.scope_builder(builder, |ui| {
        ui.set_clip_rect(content_rect);
        egui::Frame::new().fill(colors.bg_base).show(ui, |ui| {
            ui.set_min_size(content_rect.size());
            ui.horizontal(|ui| {
                if ui.button("‹ Messages").clicked() {
                    out.push(MsgAction::PopToList);
                }
                ui.heading(peer_display(peer));
            });
            ui.separator();

            // --- swipe-left-to-peek timestamps -------------------------
            let list_rect = ui.available_rect_before_wrap();
            let peek_sense = ui.interact(list_rect, ui.id().with("peek"), egui::Sense::drag());
            if peek_sense.dragged() {
                let d = peek_sense.drag_delta();
                // Horizontal, leftward drags drive the peek; vertical scroll is
                // left to the ScrollArea (we only react when |dx|>|dy|).
                if d.x.abs() > d.y.abs() {
                    state.peek = dialer_core::gesture::peek_fraction(state.peek - d.x, 84.0);
                }
            }
            if peek_sense.drag_stopped() {
                state.peek = 0.0;
            }
            let peek = state.peek;
            // Pull the open-menu id into a local for the loop so we don't hold
            // a borrow of all of `state` across the ScrollArea closure.
            let mut menu_for = state.menu_for;
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let Some(thread) = thread else { return };
                    for m in &thread.messages {
                        bubble_row(ui, m, peer, &mut menu_for, guard, colors, peek, out);
                        ui.add_space(4.0);
                    }
                });
            state.menu_for = menu_for;

            ui.separator();
            ui.horizontal(|ui| {
                let te = egui::TextEdit::singleline(&mut state.compose).id_salt("messages-state_compose")
                    .hint_text("Message…")
                    .desired_width(ui.available_width() - 70.0);
                let resp = ui.add(te);
                let send = ui.button("Send").clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if send && !state.compose.trim().is_empty() {
                    let payload = dialer_core::message::parse_sms(state.compose.trim());
                    out.push(MsgAction::Send { peer: peer.to_string(), payload });
                    state.compose.clear();
                }
            });
        });
    });
}

/// One bubble plus its long-press menu, Tapback bar, reaction badge, and the
/// timestamp that slides in from the right during a peek.
#[allow(clippy::too_many_arguments)]
fn bubble_row(
    ui: &mut egui::Ui,
    m: &Message,
    peer: &str,
    menu_for: &mut Option<(Dir, u64)>,
    guard: &GuardPolicy,
    colors: &PaletteColors,
    peek: f32,
    out: &mut Vec<MsgAction>,
) {
    let mine = m.dir == Dir::Out;
    let this = (m.dir, m.ts);
    ui.horizontal(|ui| {
        let layout = if mine {
            egui::Layout::right_to_left(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Min)
        };
        // Reserve the peek gutter on the right; the row shifts left into it.
        ui.with_layout(layout, |ui| {
            if peek > 0.5 {
                ui.add_space(0.0);
            }
            let resp = ui
                .scope(|ui| bubble(ui, &m.payload, mine, peer, guard, colors, out))
                .response;

            // Long-press (touch) or right-click (desktop) opens the menu.
            if resp.long_touched() || resp.secondary_clicked() {
                *menu_for = Some(this);
            }

            // Reaction badge on the bubble corner.
            if let Some(tb) = m.reaction {
                ui.label(egui::RichText::new(tb.glyph()).size(14.0));
            }
        });
        // Timestamp peeking in from the right.
        if peek > 0.5 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space((84.0 - peek).max(0.0));
                ui.label(egui::RichText::new(clock(m.ts)).small().color(colors.text_weak));
            });
        }
    });

    // The long-press overlay for this bubble.
    if *menu_for == Some(this) {
        tapback_and_menu(ui, m, peer, menu_for, colors, out);
    }
}

/// hh:mm (local, from unix secs) — the peek label.
fn clock(ts: u64) -> String {
    let secs = ts % 86_400;
    format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

/// The iMessage long-press surface: the six-Tapback bar plus a small menu
/// (Copy / Reply, and Run for a runnable command). Rendered inline under the
/// bubble; a click elsewhere (the buttons) closes it.
fn tapback_and_menu(
    ui: &mut egui::Ui,
    m: &Message,
    peer: &str,
    menu_for: &mut Option<(Dir, u64)>,
    colors: &PaletteColors,
    out: &mut Vec<MsgAction>,
) {
    egui::Frame::new().fill(colors.bg_raised).inner_margin(6.0).corner_radius(20.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            for tb in Tapback::bar() {
                let selected = m.reaction == Some(tb);
                let mut txt = egui::RichText::new(tb.glyph()).size(20.0);
                if selected {
                    txt = txt.background_color(colors.accent_soft);
                }
                if ui.add(egui::Button::new(txt).frame(false)).clicked() {
                    out.push(MsgAction::React {
                        peer: peer.to_string(),
                        dir: m.dir,
                        ts: m.ts,
                        tapback: tb,
                    });
                    *menu_for = None;
                }
            }
        });
    });
    ui.horizontal(|ui| {
        if ui.small_button("Copy").clicked() {
            out.push(MsgAction::Copy(m.payload.to_sms()));
            *menu_for = None;
        }
        if ui.small_button("Reply").clicked() {
            out.push(MsgAction::Reply(m.payload.preview()));
            *menu_for = None;
        }
        if ui.small_button("Close").clicked() {
            *menu_for = None;
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
