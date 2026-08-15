//! dialer-egui — the phone app's surface.
//!
//! One frame is: drain backend [`Event`]s into the [`Session`], persist any
//! finished calls to the history store, then draw exactly one of two worlds:
//!
//! * **in-call** (something live in the session) — full-screen call view.
//!   A ringing incoming call takes the foreground over an active one because
//!   it is the one that expires; accept/decline are the only two large
//!   targets on screen at that moment.
//! * **idle** — two [`Nav`] pages, Keypad and Recents, with a bottom tab bar
//!   sized by [`Adaptive`] (thumb-reach on the FP5/PinePhone, compact strip
//!   on desktop).
//!
//! The dialpad reuses `uikit_egui::glass_key` — the same key surface as
//! rusty-board — so pressing a phone digit feels like pressing a keyboard
//! key everywhere in the fleet. During a call the identical pad becomes the
//! DTMF pad; the only thing that changes is where the digit goes
//! ([`Command::Dtmf`] instead of the entry buffer), which is the actual
//! semantics of a phone keypad and cheaper than a second widget.

pub mod messages;
pub mod wizard;

use std::time::{SystemTime, UNIX_EPOCH};

use dialer_core::command_guard::GuardPolicy;
use dialer_core::conversation::{Conversations, Dir, Message};
use dialer_core::history::HistoryStore;
use dialer_core::link::{Command, Event, Link};
use dialer_core::message::Payload;
use dialer_core::number;
use dialer_core::session::{ModemStatus, Outcome, Session};
use dialer_core::types::{CallDirection, CallInfo};
use uikit_egui::confirm_dialog::{ConfirmDialog, Outcome as ConfirmOutcome};
use uikit_egui::{Adaptive, GestureLayer, Nav, PaletteColors};
use messages::{MessagesState, MsgAction};

/// Must match packaging exactly or the icon/dock grouping silently breaks
/// (see krapper's note on the same constant).
pub const APP_ID: &str = "org.ecosystem.Dialer";

const PAGE_KEYPAD: usize = 0;
const PAGE_RECENTS: usize = 1;
const PAGE_MESSAGES: usize = 2;
const PAGE_COUNT: usize = 3;

/// The twelve keys, row-major. One source for both dial and DTMF pads.
pub const KEYPAD: [[char; 3]; 4] = [
    ['1', '2', '3'],
    ['4', '5', '6'],
    ['7', '8', '9'],
    ['*', '0', '#'],
];

/// mm:ss (or h:mm:ss past an hour) for the ticking call timer.
pub fn talk_clock(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// Glyph + tint key for a history row. Pure so it is testable: the glyph is
/// how a glance distinguishes "they never reached me" from "I didn't pick up".
pub fn history_glyph(direction: CallDirection, outcome: Outcome) -> (&'static str, Tint) {
    match (outcome, direction) {
        (Outcome::Missed, _) => ("↓✕", Tint::Danger),
        (Outcome::Rejected, _) => ("↓⊘", Tint::Weak),
        (Outcome::NoAnswer, _) => ("↑✕", Tint::Weak),
        (Outcome::Completed, CallDirection::Incoming) => ("↓", Tint::Success),
        (Outcome::Completed, _) => ("↑", Tint::Success),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    Success,
    Danger,
    Weak,
}

impl Tint {
    fn color(self, colors: &PaletteColors) -> egui::Color32 {
        match self {
            Tint::Success => colors.success,
            Tint::Danger => colors.danger,
            Tint::Weak => colors.text_weak,
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The whole app state. Public so a future tablet/desktop shell can embed it.
pub struct Shell {
    link: Link,
    /// The Matrix signalling link, when signed in. Its calls carry
    /// "matrix/"-prefixed ids; the shell routes commands on that prefix.
    matrix_link: Option<Link>,
    /// Matrix connection status, kept out of `session.modem` so the two
    /// transports don't clobber each other's status line.
    matrix_status: Option<String>,
    wizard: Option<wizard::Wizard>,
    /// Why we fell back to the mock, if we did — shown once as a toast.
    fallback_reason: Option<String>,
    session: Session,
    history: HistoryStore,
    nav: Nav,
    gesture: GestureLayer,
    entry: String,
    convs: Conversations,
    guard: GuardPolicy,
    msg: MessagesState,
    run_confirm: Option<(String, ConfirmDialog)>,
    /// DTMF pad open on the in-call screen; tones sent so far (readout).
    dtmf_open: bool,
    dtmf_sent: String,
    toasts: uikit_egui::toast::Toasts,
    colors: PaletteColors,
}

impl Shell {
    pub fn new(link: Link, fallback_reason: Option<String>, colors: PaletteColors) -> Shell {
        let (history, warn) = HistoryStore::open(&HistoryStore::default_path());
        let (convs, cwarn) = Conversations::open(&Conversations::default_path());
        let mut toasts = uikit_egui::toast::Toasts::new();
        if let Some(w) = warn {
            toasts.warning(w, 0.0);
        }
        if let Some(w) = cwarn {
            toasts.warning(w, 0.0);
        }
        let guard = load_guard_policy();
        // A saved Matrix session comes up on launch; failure is a toast,
        // not a launch blocker.
        #[cfg(feature = "matrix")]
        let (matrix_link, matrix_err) = match dialer_core::matrix::Creds::load() {
            Some(creds) => match dialer_core::matrix::spawn(creds) {
                Ok(l) => (Some(l), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        #[cfg(not(feature = "matrix"))]
        let (matrix_link, matrix_err): (Option<Link>, Option<String>) = (None, None);
        if let Some(e) = matrix_err {
            toasts.warning(e, 0.0);
        }

        Shell {
            link,
            matrix_link,
            matrix_status: None,
            wizard: if wizard::setup_was_completed() { None } else { Some(wizard::Wizard::new(None, guard.clone())) },
            fallback_reason,
            session: Session::default(),
            history,
            nav: Nav::new(PAGE_COUNT),
            gesture: GestureLayer::horizontal_page(360.0),
            entry: String::new(),
            convs,
            guard,
            msg: MessagesState::default(),
            run_confirm: None,
            dtmf_open: false,
            dtmf_sent: String::new(),
            toasts,
            colors,
        }
    }

    fn send(&mut self, cmd: Command, now: f64) {
        // Route by call id: "matrix/…" belongs to the matrix link, everything
        // else (including id-less commands like Dial/ApplyApn) to the modem.
        let is_matrix = matches!(&cmd,
            Command::Accept(id) | Command::Hangup(id) | Command::Dtmf(id, _)
                if id.starts_with("matrix/"));
        let tx = if is_matrix {
            match &self.matrix_link {
                Some(l) => &l.tx,
                None => {
                    self.toasts.error("matrix backend not running", now);
                    return;
                }
            }
        } else {
            &self.link.tx
        };
        // A dead backend thread must be loud, not silent: every press after
        // it dies would otherwise just do nothing.
        if tx.send(cmd).is_err() {
            self.toasts.error("backend stopped — restart the app", now);
        }
    }

    fn on_message_in(&mut self, peer: String, payload: Payload, ts: u64, now: f64) {
        let preview = payload.preview();
        // If the thread is open, mark read immediately; else it counts unread.
        let read = matches!(&self.msg.view, messages::MessagesView::Thread(p) if *p == peer)
            && self.nav.page() == PAGE_MESSAGES
            && self.wizard.is_none();
        let m = Message { peer: peer.clone(), dir: Dir::In, payload, ts, read, reaction: None };
        if let Err(e) = self.convs.append(m) {
            self.toasts.error(e, now);
        }
        if !read {
            self.toasts.info(format!("{}: {}", messages::peer_display(&peer), preview), now);
        }
    }

    fn record_outgoing(&mut self, peer: &str, payload: Payload, now: f64) {
        let m = Message {
            peer: peer.to_string(),
            dir: Dir::Out,
            payload,
            ts: now_unix(),
            read: true,
            reaction: None,
        };
        if let Err(e) = self.convs.append(m) {
            self.toasts.error(e, now);
        }
    }

    /// Route a SendMessage to the transport implied by the peer prefix.
    fn send_message(&mut self, peer: String, payload: Payload, now: f64) {
        self.record_outgoing(&peer, payload.clone(), now);
        let cmd = Command::SendMessage { peer: peer.clone(), payload };
        let tx = if peer.starts_with("matrix") {
            self.matrix_link.as_ref().map(|l| &l.tx)
        } else {
            Some(&self.link.tx)
        };
        match tx {
            Some(tx) if tx.send(cmd).is_ok() => {}
            Some(_) => self.toasts.error("backend stopped — restart the app", now),
            None => self.toasts.error("no backend for that peer", now),
        }
    }

    fn handle_msg_actions(&mut self, actions: Vec<MsgAction>, ctx: &egui::Context, now: f64) {
        for a in actions {
            match a {
                MsgAction::Send { peer, payload } => self.send_message(peer, payload, now),
                MsgAction::Copy(text) => {
                    ctx.copy_text(text);
                    self.toasts.success("copied", now);
                }
                MsgAction::OpenUri(uri) => {
                    ctx.open_url(egui::OpenUrl::new_tab(uri));
                }
                MsgAction::OpenPath(path) => {
                    // A confined view: hand it to the desktop opener.
                    ctx.open_url(egui::OpenUrl::new_tab(format!("file://{}", path.display())));
                }
                MsgAction::ViewDenied(path) => {
                    self.toasts.error(format!("view denied (outside allowed roots): {path}"), now);
                }
                MsgAction::React { peer, dir, ts, tapback } => {
                    if let Err(e) = self.convs.set_reaction(&peer, dir, ts, tapback) {
                        self.toasts.error(e, now);
                    }
                }
                MsgAction::PopToList => {
                    self.msg.view = messages::MessagesView::List;
                    self.msg.pop = None;
                    self.msg.menu_for = None;
                    self.msg.peek = 0.0;
                }
                MsgAction::Reply(quote) => {
                    // iMessage-style quote prefix; the composer keeps focus.
                    if !self.msg.compose.is_empty() && !self.msg.compose.ends_with(' ') {
                        self.msg.compose.push(' ');
                    }
                    self.msg.compose.push_str(&format!("> {quote}\n"));
                }
                MsgAction::RunConfirmed { command } => {
                    // Never run straight from a card: always stage a modal that
                    // re-shows the exact command. The guard already said it MAY
                    // run; the modal is the human's last look.
                    let dialog = ConfirmDialog::new(
                        "Run this command?",
                        format!("{command}

This executes on your device."),
                        "Run",
                    )
                    .with_timeout(20.0);
                    self.run_confirm = Some((command, dialog));
                }
            }
        }
    }

    /// Actually spawn a guard- and user-approved command. No shell: the
    /// tokenised argv is executed directly, so payload metacharacters are
    /// inert (see command_guard). Detached; output is not captured here.
    fn spawn_command(&mut self, command: &str, now: f64) {
        match dialer_core::command_guard::tokenize(command) {
            Ok(tokens) => {
                let (prog, args) = tokens.split_first().expect("tokenize rejects empty");
                match std::process::Command::new(prog).args(args).spawn() {
                    Ok(_) => self.toasts.success(format!("ran: {command}"), now),
                    Err(e) => self.toasts.error(format!("run failed: {e}"), now),
                }
            }
            Err(e) => self.toasts.error(format!("won't run: {e}"), now),
        }
    }

    fn drain_events(&mut self, epoch: u64, now: f64) {
        loop {
            match self.link.rx.try_recv() {
                Ok(Event::Error(e)) => self.toasts.error(e, now),
                Ok(Event::Info(msg)) => {
                    // APN feedback belongs next to the Apply button too.
                    if let Some(w) = &mut self.wizard {
                        w.note_apply_result(&msg);
                    }
                    self.toasts.success(msg, now);
                }
                Ok(Event::MessageIn { peer, payload, ts }) => self.on_message_in(peer, payload, ts, now),
                Ok(Event::MessageSent { .. }) => {}
                Ok(ev) => {
                    if let Event::ModemDown { detail } = &ev {
                        self.toasts.warning(format!("modem lost: {detail}"), now);
                    }
                    self.session.apply(ev, epoch);
                }
                Err(_) => break,
            }
        }
        // Drain the matrix link the same way, keeping matrix status separate.
        let mx_events: Vec<Event> = match &self.matrix_link {
            Some(mx) => std::iter::from_fn(|| mx.rx.try_recv().ok()).collect(),
            None => Vec::new(),
        };
        for ev in mx_events {
            match ev {
                Event::ModemUp { label } => self.matrix_status = Some(label),
                Event::ModemDown { detail } => self.matrix_status = Some(format!("offline: {detail}")),
                Event::Error(e) => self.toasts.error(e, now),
                Event::Info(msg) => self.toasts.success(msg, now),
                Event::MessageIn { peer, payload, ts } => self.on_message_in(peer, payload, ts, now),
                Event::MessageSent { .. } => {}
                ev => self.session.apply(ev, epoch),
            }
        }
        for entry in self.session.take_history() {
            if let Err(e) = self.history.append(entry) {
                self.toasts.error(e, now);
            }
        }
    }

    /// The shared 4×3 pad. Calls `on_key` per press; sized for touch.
    fn keypad(&self, ui: &mut egui::Ui, adaptive: &Adaptive, mut on_key: impl FnMut(char)) {
        let gap = 8.0;
        let cols = 3.0;
        let avail = ui.available_width().min(adaptive.pick(360.0, 420.0, 380.0));
        let key_w = ((avail - gap * (cols - 1.0)) / cols).max(adaptive.min_touch_target);
        let key_h = adaptive.min_touch_target.max(56.0);
        let pad_w = key_w * cols + gap * (cols - 1.0);
        let left = ui.available_width() / 2.0 - pad_w / 2.0; // center the pad

        for row in KEYPAD {
            ui.horizontal(|ui| {
                ui.add_space(left.max(0.0));
                for key in row {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(key_w, key_h), egui::Sense::hover());
                    let resp = uikit_egui::glass_key(
                        ui,
                        rect,
                        &key.to_string(),
                        self.colors.bg_card,
                        self.colors.border,
                        self.colors.accent,
                        self.colors.text,
                        !key.is_ascii_digit(), // * and # read as modifiers
                    );
                    if resp.clicked() {
                        on_key(key);
                    }
                }
            });
            ui.add_space(gap);
        }
    }

    fn keypad_page(&mut self, ui: &mut egui::Ui, adaptive: &Adaptive, now: f64) {
        ui.add_space(adaptive.pick(24.0, 32.0, 16.0));

        // Readout: what will be dialed, grouped for scanning.
        let shown = if self.entry.is_empty() {
            "Enter number".to_string()
        } else {
            number::display_group(&self.entry)
        };
        let color = if self.entry.is_empty() { self.colors.text_weak } else { self.colors.text };
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(shown).size(28.0).monospace().color(color));
        });
        ui.add_space(12.0);

        let mut pressed: Option<char> = None;
        self.keypad(ui, adaptive, |k| pressed = Some(k));
        if let Some(k) = pressed {
            self.entry.push(k);
        }

        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                let dialable = number::is_dialworthy(&number::normalize(&self.entry));
                ui.add_space(ui.available_width() / 2.0 - 110.0);
                let call = egui::Button::new(
                    egui::RichText::new("📞  Call").size(18.0).color(egui::Color32::WHITE),
                )
                .fill(self.colors.success)
                .min_size(egui::vec2(150.0, adaptive.min_touch_target.max(52.0)));
                if ui.add_enabled(dialable, call).clicked() {
                    let number = number::normalize(&self.entry);
                    self.send(Command::Dial(number), now);
                    self.entry.clear();
                }
                let back = egui::Button::new(egui::RichText::new("⌫").size(18.0))
                    .min_size(egui::vec2(60.0, adaptive.min_touch_target.max(52.0)));
                let r = ui.add_enabled(!self.entry.is_empty(), back);
                if r.clicked() {
                    self.entry.pop();
                }
                // Long-press-to-clear is the phone idiom; secondary click is
                // the desktop stand-in for it.
                if r.long_touched() || r.secondary_clicked() {
                    self.entry.clear();
                }
            });
        });
    }

    fn recents_page(&mut self, ui: &mut egui::Ui, epoch: u64, now: f64) {
        if self.history.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("No calls yet").color(self.colors.text_weak));
            });
            return;
        }
        let mut redial: Option<String> = None;
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            for e in self.history.recent().take(100) {
                let (glyph, tint) = history_glyph(e.direction, e.outcome);
                let row = ui
                    .horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(glyph).size(16.0).color(tint.color(&self.colors)),
                        );
                        ui.label(
                            egui::RichText::new(number::display_group(&e.number))
                                .monospace()
                                .color(self.colors.text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ago = uikit_egui::fmt::relative_time(
                                epoch.saturating_sub(e.started) as i64,
                                uikit_egui::fmt::Style::Bare,
                            );
                            ui.label(egui::RichText::new(ago).small().color(self.colors.text_weak));
                            if e.duration_secs > 0 {
                                ui.label(
                                    egui::RichText::new(talk_clock(e.duration_secs))
                                        .small()
                                        .color(self.colors.text_weak),
                                );
                            }
                        });
                    })
                    .response;
                // The whole row redials — a phone list, not a hyperlink.
                if row.interact(egui::Sense::click()).clicked() {
                    redial = Some(e.number.clone());
                }
                ui.separator();
            }
        });
        if let Some(n) = redial {
            self.entry = n;
            self.nav.go_page(PAGE_KEYPAD);
            let _ = now; // dialing is a deliberate second tap on the Call key
        }
    }

    fn in_call(&mut self, ui: &mut egui::Ui, call: CallInfo, adaptive: &Adaptive, epoch: u64, now: f64) {
        ui.add_space(adaptive.pick(40.0, 60.0, 30.0));
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(number::display_group(&call.number))
                    .size(30.0)
                    .monospace()
                    .color(self.colors.text),
            );
            ui.add_space(6.0);
            let status = if call.state.is_ringing_in() {
                call.state.label().to_string()
            } else if let Some(t) = self.session.talk_secs(&call.id, epoch) {
                talk_clock(t)
            } else {
                call.state.label().to_string()
            };
            ui.label(egui::RichText::new(status).size(18.0).color(self.colors.text_weak));
        });
        ui.add_space(20.0);

        if call.state.is_ringing_in() {
            let is_matrix = call.id.starts_with("matrix/");
            if is_matrix {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Matrix call — answering needs a media engine (not in yet)")
                            .small()
                            .color(self.colors.text_weak),
                    );
                });
                ui.add_space(6.0);
            }
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - if is_matrix { 75.0 } else { 165.0 });
                    if !is_matrix {
                        let accept = egui::Button::new(
                            egui::RichText::new("Accept").size(18.0).color(egui::Color32::WHITE),
                        )
                        .fill(self.colors.success)
                        .min_size(egui::vec2(150.0, 56.0));
                        if ui.add(accept).clicked() {
                            self.send(Command::Accept(call.id.clone()), now);
                        }
                        ui.add_space(12.0);
                    }
                    let decline = egui::Button::new(
                        egui::RichText::new("Decline").size(18.0).color(egui::Color32::WHITE),
                    )
                    .fill(self.colors.danger)
                    .min_size(egui::vec2(150.0, 56.0));
                    if ui.add(decline).clicked() {
                        self.session.note_user_hangup(&call.id);
                        self.send(Command::Hangup(call.id.clone()), now);
                    }
                });
            });
            return;
        }

        // Active / dialing / held.
        if self.dtmf_open {
            if !self.dtmf_sent.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(&self.dtmf_sent)
                            .monospace()
                            .color(self.colors.text_weak),
                    );
                });
            }
            let mut tone: Option<char> = None;
            self.keypad(ui, adaptive, |k| tone = Some(k));
            if let Some(k) = tone {
                self.dtmf_sent.push(k);
                self.send(Command::Dtmf(call.id.clone(), k.to_string()), now);
            }
            ui.add_space(8.0);
        }

        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 2.0 - 150.0);
                let hang = egui::Button::new(
                    egui::RichText::new("Hang up").size(18.0).color(egui::Color32::WHITE),
                )
                .fill(self.colors.danger)
                .min_size(egui::vec2(150.0, 56.0));
                if ui.add(hang).clicked() {
                    self.send(Command::Hangup(call.id.clone()), now);
                }
                ui.add_space(12.0);
                let label = if self.dtmf_open { "Hide keys" } else { "Keypad" };
                if ui
                    .add(egui::Button::new(label).min_size(egui::vec2(110.0, 56.0)))
                    .clicked()
                {
                    self.dtmf_open = !self.dtmf_open;
                }
            });
        });
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui, adaptive: &Adaptive) {
        let unread = self.convs.total_unread();
        let names = ["Keypad", "Recents", "Messages"];
        ui.horizontal(|ui| {
            let w = ui.available_width() / names.len() as f32;
            for (i, name) in names.iter().enumerate() {
                let selected = self.nav.page() == i;
                let label = if i == PAGE_MESSAGES && unread > 0 {
                    format!("{name} ●{unread}")
                } else {
                    name.to_string()
                };
                let text = egui::RichText::new(label).size(15.0).color(if selected {
                    self.colors.accent
                } else if i == PAGE_MESSAGES && unread > 0 {
                    self.colors.accent
                } else {
                    self.colors.text_weak
                });
                let btn = egui::Button::new(text)
                    .frame(false)
                    .min_size(egui::vec2(w, adaptive.min_touch_target.max(44.0)));
                if ui.add(btn).clicked() {
                    self.nav.go_page(i);
                }
            }
        });
    }

    fn status_line(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal(|ui| {
            let (txt, col) = match &self.session.modem {
                ModemStatus::Probing => ("modem: probing…".to_string(), self.colors.text_weak),
                ModemStatus::Up(l) => (format!("{l} · {}", self.link.kind), self.colors.text_weak),
                ModemStatus::Down(d) => (format!("no modem: {d}"), self.colors.warning),
            };
            ui.label(egui::RichText::new(txt).small().color(col));
            if let Some(mx) = &self.matrix_status {
                ui.label(
                    egui::RichText::new(format!("· matrix {mx}"))
                        .small()
                        .color(self.colors.text_weak),
                );
            }
            if self.wizard.is_none() && ui.small_button("setup").clicked() {
                let user = self.matrix_status.clone().filter(|s| !s.starts_with("offline:"));
                self.wizard = Some(wizard::Wizard::new(user, self.guard.clone()));
            }
            if self.link.kind == "mock" {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The demo affordance, only where the modem is simulated.
                    if ui.small_button("simulate incoming").clicked() {
                        self.send(Command::SimulateIncoming("+15550100".into()), now);
                    }
                });
            }
        });
    }

    fn handle_wizard(&mut self, outs: Vec<wizard::WizardOut>, now: f64) {
        for out in outs {
            match out {
                wizard::WizardOut::ApplyApn(s) => self.send(Command::ApplyApn(s), now),
                wizard::WizardOut::MatrixSignedIn(creds) => {
                    if let Err(e) = creds.save() {
                        self.toasts.error(e, now);
                    }
                    #[cfg(feature = "matrix")]
                    match dialer_core::matrix::spawn(creds) {
                        Ok(l) => {
                            self.matrix_link = Some(l);
                            self.toasts.success("Matrix signed in", now);
                        }
                        Err(e) => self.toasts.error(e, now),
                    }
                    #[cfg(not(feature = "matrix"))]
                    let _ = creds; // sign-in UI is gated; arm kept total
                }
                wizard::WizardOut::MatrixForgotten => {
                    dialer_core::matrix::Creds::forget();
                    if let Some(l) = self.matrix_link.take() {
                        let _ = l.tx.send(Command::Quit);
                    }
                    self.matrix_status = None;
                    self.toasts.success("Matrix session forgotten", now);
                }
                wizard::WizardOut::SetGuardPolicy(p) => {
                    self.guard = p.clone();
                    match save_guard_policy(&p) {
                        Ok(()) => self.toasts.success("command policy saved", now),
                        Err(e) => self.toasts.error(e, now),
                    }
                }
                wizard::WizardOut::Close => self.wizard = None,
            }
        }
    }

    /// One frame. Split from `eframe::App` so a host can drive it in tests
    /// or embed it in another surface.
    pub fn frame(&mut self, ctx: &egui::Context) {
        let epoch = now_unix();
        let now = ctx.input(|i| i.time);
        if let Some(reason) = self.fallback_reason.take() {
            self.toasts.warning(format!("using simulated modem ({reason})"), now);
        }
        self.drain_events(epoch, now);

        let adaptive = Adaptive::detect(ctx);
        let foreground = self.session.foreground().cloned();
        if foreground.is_none() {
            // Leaving a call resets per-call UI state.
            self.dtmf_open = false;
            self.dtmf_sent.clear();
        }

        egui::TopBottomPanel::bottom("dialer-bottom")
            .frame(egui::Frame::new().fill(self.colors.bg_base).inner_margin(6.0))
            .show(ctx, |ui| {
                self.status_line(ui, now);
                if foreground.is_none() {
                    self.tab_bar(ui, &adaptive);
                }
            });

        let mut msg_actions: Vec<MsgAction> = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.colors.bg_base).inner_margin(10.0))
            .show(ctx, |ui| {
                match &foreground {
                    // A ringing/active call outranks the wizard: the phone
                    // part of the phone is never behind setup.
                    Some(call) => {
                        self.in_call(ui, call.clone(), &adaptive, epoch, now);
                        return;
                    }
                    None if self.wizard.is_some() => {
                        let mut w = self.wizard.take().expect("checked is_some");
                        let operator = self.session.operator.clone();
                        let mut outs = Vec::new();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            outs = w.ui(ui, &self.colors, operator.as_deref());
                        });
                        self.wizard = Some(w);
                        self.handle_wizard(outs, now);
                        return;
                    }
                    None => {}
                }

                // Swipe paging across the three idle pages. Only arms when we
                // aren't inside an open thread (the thread's own vertical
                // scroll + its back button own that surface).
                let surface = ui.available_rect_before_wrap();
                let in_thread = self.nav.page() == PAGE_MESSAGES
                    && matches!(self.msg.view, messages::MessagesView::Thread(_));
                if !in_thread {
                    let frame = self.gesture.update(ui, surface, true);
                    if let Some(dir) = frame.page_jump {
                        let next = (self.nav.page() as i32 + dir.sign())
                            .clamp(0, PAGE_COUNT as i32 - 1) as usize;
                        self.nav.go_page(next);
                    }
                }

                match self.nav.page() {
                    PAGE_RECENTS => self.recents_page(ui, epoch, now),
                    PAGE_MESSAGES => {
                        // Opening a thread clears its unread.
                        if let messages::MessagesView::Thread(peer) = self.msg.view.clone() {
                            self.convs.mark_read(&peer);
                        } else {
                            // On the list: no pop in flight.
                            self.msg.pop = None;
                        }
                        msg_actions = messages::messages_page(
                            ui, &self.convs, &mut self.msg, &self.guard, &self.colors, epoch,
                        );
                    }
                    _ => self.keypad_page(ui, &adaptive, now),
                }
            });
        if !msg_actions.is_empty() {
            self.handle_msg_actions(msg_actions, ctx, now);
        }

        // The run-confirm modal, if a command is staged.
        if let Some((command, dialog)) = &mut self.run_confirm {
            match dialog.show(ctx) {
                ConfirmOutcome::Confirmed => {
                    let cmd = command.clone();
                    self.run_confirm = None;
                    self.spawn_command(&cmd, now);
                }
                // Cancelled AND Discarded both refuse the command. This
                // fleet's confirm_dialog has a Discarded variant the sibling's
                // did not — a timeout-driven dismissal rather than a deliberate
                // no. They are different reasons and the same answer, and for a
                // guard on running a command from a remote handoff, the safe
                // reading of "the dialog went away on its own" is DO NOT RUN.
                //
                // Left as separate arms rather than a wildcard: a future
                // variant should fail this match and be decided, not silently
                // inherit whatever the last arm does.
                ConfirmOutcome::Cancelled => self.run_confirm = None,
                ConfirmOutcome::Discarded => self.run_confirm = None,
                ConfirmOutcome::Pending => {}
            }
        }

        self.toasts.ui(ctx, 64.0);

        // Backend events arrive between frames; while anything is live (or
        // could ring) poll at 4 Hz rather than only on input. Idle with a
        // mock and no calls still ticks so "simulate incoming" lands.
        if !self.session.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

#[cfg(feature = "app")]
impl eframe::App for Shell {
    // This fleet's eframe requires `ui` alongside `update`; the app is driven
    // from update() below, exactly as every other egui frontend here does it.
    // The sibling that wrote this crate was building against a version without
    // it, which is why the trait was satisfied there and not here.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

/// Backend choice for [`run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// ModemManager, falling back to the mock with a toast saying why.
    Auto,
    Mock,
    #[cfg(feature = "mm")]
    Mm,
}

#[cfg(feature = "app")]
pub fn run(choice: BackendChoice) -> eframe::Result<()> {
    let (link, fallback): (Link, Option<String>) = match choice {
        BackendChoice::Mock => (dialer_core::mock::spawn(), None),
        #[cfg(feature = "mm")]
        BackendChoice::Mm => match dialer_core::mm::spawn() {
            Ok(l) => (l, None),
            Err(e) => {
                eprintln!("dialer: --mm requested but unavailable: {e}");
                std::process::exit(2);
            }
        },
        BackendChoice::Auto => {
            #[cfg(feature = "mm")]
            {
                dialer_core::mm::spawn_or(dialer_core::mock::spawn)
            }
            #[cfg(not(feature = "mm"))]
            {
                (dialer_core::mock::spawn(), Some("built without mm".into()))
            }
        }
    };

    let opts = eframe::NativeOptions {
        viewport: uikit_egui::desktop::viewport(APP_ID, "Dialer", [400.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Dialer",
        opts,
        Box::new(move |cc| {
            let colors = uikit_egui::apply_palette(
                &cc.egui_ctx,
                snapclone_palette(),
            );
            Ok(Box::new(Shell::new(link, fallback, colors)))
        }),
    )
}

/// Guard policy location, next to the other prefs.
fn guard_policy_path() -> std::path::PathBuf {
    HistoryStore::default_path()
        .parent()
        .map(|d| d.join("command-policy.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("command-policy.json"))
}

/// Load the command guard policy; conservative default (running off) if there
/// is no file or it's unreadable — a corrupt policy must never fail *open*.
fn load_guard_policy() -> GuardPolicy {
    std::fs::read_to_string(guard_policy_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_guard_policy(p: &GuardPolicy) -> Result<(), String> {
    let path = guard_policy_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(p).unwrap()).map_err(|e| e.to_string())
}

/// The shared default palette — this app is not a Qt/GTK port, so per the
/// snapclone-egui convention it takes the fleet default rather than a
/// bespoke one.
#[cfg(feature = "app")]
fn snapclone_palette() -> &'static snapclone_theme::Palette {
    snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talk_clock_rolls_into_hours() {
        assert_eq!(talk_clock(0), "0:00");
        assert_eq!(talk_clock(65), "1:05");
        assert_eq!(talk_clock(3599), "59:59");
        assert_eq!(talk_clock(3661), "1:01:01");
    }

    #[test]
    fn glyphs_distinguish_missed_from_rejected_from_no_answer() {
        let (m, mt) = history_glyph(CallDirection::Incoming, Outcome::Missed);
        let (r, _) = history_glyph(CallDirection::Incoming, Outcome::Rejected);
        let (n, _) = history_glyph(CallDirection::Outgoing, Outcome::NoAnswer);
        assert_ne!(m, r);
        assert_ne!(m, n);
        assert_eq!(mt, Tint::Danger, "a missed call is the one that must pop in the list");
    }

    #[test]
    fn keypad_covers_all_dtmf_symbols_once() {
        let flat: Vec<char> = KEYPAD.iter().flatten().copied().collect();
        assert_eq!(flat.len(), 12);
        for c in "0123456789*#".chars() {
            assert_eq!(flat.iter().filter(|k| **k == c).count(), 1, "{c} exactly once");
        }
    }
}
