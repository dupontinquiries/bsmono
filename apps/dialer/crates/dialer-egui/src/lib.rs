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

use std::time::{SystemTime, UNIX_EPOCH};

use dialer_core::history::HistoryStore;
use dialer_core::link::{Command, Event, Link};
use dialer_core::number;
use dialer_core::session::{ModemStatus, Outcome, Session};
use dialer_core::types::{CallDirection, CallInfo};
use uikit_egui::{Adaptive, Nav, PaletteColors};

/// Must match packaging exactly or the icon/dock grouping silently breaks
/// (see krapper's note on the same constant).
pub const APP_ID: &str = "org.ecosystem.Dialer";

const PAGE_KEYPAD: usize = 0;
const PAGE_RECENTS: usize = 1;

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
    /// Why we fell back to the mock, if we did — shown once as a toast.
    fallback_reason: Option<String>,
    session: Session,
    history: HistoryStore,
    nav: Nav,
    entry: String,
    /// DTMF pad open on the in-call screen; tones sent so far (readout).
    dtmf_open: bool,
    dtmf_sent: String,
    toasts: uikit_egui::toast::Toasts,
    colors: PaletteColors,
}

impl Shell {
    pub fn new(link: Link, fallback_reason: Option<String>, colors: PaletteColors) -> Shell {
        let (history, warn) = HistoryStore::open(&HistoryStore::default_path());
        let mut toasts = uikit_egui::toast::Toasts::new();
        if let Some(w) = warn {
            toasts.warning(w, 0.0);
        }
        Shell {
            link,
            fallback_reason,
            session: Session::default(),
            history,
            nav: Nav::new(2),
            entry: String::new(),
            dtmf_open: false,
            dtmf_sent: String::new(),
            toasts,
            colors,
        }
    }

    fn send(&mut self, cmd: Command, now: f64) {
        // A dead backend thread must be loud, not silent: every press after
        // it dies would otherwise just do nothing.
        if self.link.tx.send(cmd).is_err() {
            self.toasts.error("backend stopped — restart the app", now);
        }
    }

    fn drain_events(&mut self, epoch: u64, now: f64) {
        loop {
            match self.link.rx.try_recv() {
                Ok(Event::Error(e)) => self.toasts.error(e, now),
                Ok(ev) => {
                    if let Event::ModemDown { detail } = &ev {
                        self.toasts.warning(format!("modem lost: {detail}"), now);
                    }
                    self.session.apply(ev, epoch);
                }
                Err(_) => break,
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
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 165.0);
                    let accept = egui::Button::new(
                        egui::RichText::new("Accept").size(18.0).color(egui::Color32::WHITE),
                    )
                    .fill(self.colors.success)
                    .min_size(egui::vec2(150.0, 56.0));
                    if ui.add(accept).clicked() {
                        self.send(Command::Accept(call.id.clone()), now);
                    }
                    ui.add_space(12.0);
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
        let names = ["Keypad", "Recents"];
        ui.horizontal(|ui| {
            let w = ui.available_width() / names.len() as f32;
            for (i, name) in names.iter().enumerate() {
                let selected = self.nav.page() == i;
                let text = egui::RichText::new(*name).size(15.0).color(if selected {
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

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.colors.bg_base).inner_margin(10.0))
            .show(ctx, |ui| match &foreground {
                Some(call) => self.in_call(ui, call.clone(), &adaptive, epoch, now),
                None => match self.nav.page() {
                    PAGE_RECENTS => self.recents_page(ui, epoch, now),
                    _ => self.keypad_page(ui, &adaptive, now),
                },
            });

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
