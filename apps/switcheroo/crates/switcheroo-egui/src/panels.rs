//! panels — the parts of the shell that arrived with switcheroo-core's
//! schedule/intent/filter/media modules. Each is one function over `App`;
//! `ui.rs` places them.
//!
//! * [`filter_chips`] — the menu's device filter, as toggles and a search.
//! * [`media_strip`] — transport for the player, through `MediaBackend`.
//! * [`command_box`] — "say or type": a sentence → `intent::parse` →
//!   `App::run_intent`. Typing is the fast path's floor; the mic is the
//!   host's to wire, and the box is what it types into when it does.
//! * [`schedules_pane`] — the rules, a form to add one, the home network,
//!   and what the wifi probe currently reads.
//! * [`pairing_dialog`] — the setup code, behind a confirmation that says
//!   what pairing costs. HOMEKIT.md asked for "a very loud confirmation
//!   flow, not a button"; this is the flow: the warning is the first
//!   thing in the window, the checkbox is its own sentence, and Pair is
//!   disabled until the code is eight digits AND the box is ticked.

use crate::{App, PairingDialog, Tab};
use egui::RichText;
use switcheroo_core::intent::{self, MediaCommand, Target};
use switcheroo_core::schedule::{self, Action, Trigger};
use switcheroo_core::Kind;

const TAP: f32 = 44.0;

pub fn filter_chips(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.filter.query).hint_text("find…").desired_width(120.0));
        ui.toggle_value(&mut app.filter.controllable_only, "switchable");
        ui.toggle_value(&mut app.filter.favourites_only, "★");
        let mut hk = app.filter.kinds.contains(&Kind::HomeKit);
        if ui.toggle_value(&mut hk, "HomeKit").changed() { set_kind(app, Kind::HomeKit, hk); }
        let mut gatt = app.filter.kinds.contains(&Kind::GenericGatt);
        if ui.toggle_value(&mut gatt, "GATT").changed() { set_kind(app, Kind::GenericGatt, gatt); }
        let mut near = app.filter.min_rssi.is_some();
        if ui.toggle_value(&mut near, "near").on_hover_text("≥ −80 dBm").changed() {
            app.filter.min_rssi = if near { Some(-80) } else { None };
        }
        if !app.filter.is_default() && ui.small_button("clear").clicked() {
            app.filter = Default::default();
        }
    });
}

fn set_kind(app: &mut App, k: Kind, on: bool) {
    app.filter.kinds.retain(|x| *x != k);
    if on { app.filter.kinds.push(k); }
}

pub fn media_strip(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("music").weak());
        for (label, cmd, tip) in [
            ("⏮", MediaCommand::Previous, "previous"),
            ("⏯", MediaCommand::PlayPause, "play / pause"),
            ("⏭", MediaCommand::Next, "next"),
            ("🔉", MediaCommand::VolumeDown, "quieter"),
            ("🔊", MediaCommand::VolumeUp, "louder"),
        ] {
            if ui.add_sized([TAP, TAP], egui::Button::new(label)).on_hover_text(tip).clicked() {
                app.media.submit(cmd);
            }
        }
        ui.label(RichText::new(app.media.describe()).small().weak());
    });
}

pub fn command_box(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("say or type:");
        let edit = ui.add(egui::TextEdit::singleline(&mut app.command)
            .hint_text("lights off · turn the lamp on by 9pm · next song")
            .desired_width(f32::INFINITY));
        let go = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if go && !app.command.trim().is_empty() {
            let text = std::mem::take(&mut app.command);
            app.last_result = app.run_intent(intent::parse(&text));
            edit.request_focus();
        }
    });
    if !app.last_result.is_empty() {
        ui.label(RichText::new(&app.last_result).small());
    }
}

/// A rule's form, as the pane keeps it between frames.
#[derive(Debug, Clone)]
pub struct RuleForm {
    pub target: String,
    pub on: bool,
    pub trigger: u8, // 0 by, 1 at, 2 arrive, 3 leave, 4 wifi weak
    pub time: String,
    pub weak_dbm: i16,
}

impl Default for RuleForm {
    fn default() -> Self {
        RuleForm { target: String::new(), on: false, trigger: 0, time: "9pm".into(), weak_dbm: schedule::DEFAULT_WEAK_DBM }
    }
}

impl RuleForm {
    /// Build the rule, or say what is missing.
    pub fn build(&self) -> Result<(Trigger, Action), String> {
        let words: Vec<String> = self.target.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
        let target = match self.target.trim().to_ascii_lowercase().as_str() {
            "" => return Err("which device? (a name, or lights / music / everything)".into()),
            "lights" | "all lights" => Target::Lights,
            "music" => Target::Music,
            "everything" | "all" => Target::All,
            _ => Target::Named(words.join(" ")),
        };
        let trigger = match self.trigger {
            0 | 1 => {
                let w: Vec<String> = self.time.split_whitespace().map(|s| s.to_string()).collect();
                let (minute, _) = intent::parse_time(&w, 0).ok_or_else(|| format!("time {:?} not understood (9pm, 21:30, noon)", self.time))?;
                if self.trigger == 0 { Trigger::By { minute_of_day: minute } } else { Trigger::At { minute_of_day: minute } }
            }
            2 => Trigger::ArriveHome,
            3 => Trigger::LeaveHome,
            _ => Trigger::WifiWeak { below_dbm: self.weak_dbm },
        };
        Ok((trigger, Action { target, on: self.on }))
    }
}

pub fn schedules_pane(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // The world the rules watch.
        ui.horizontal(|ui| {
            ui.label("home network:");
            if ui.add(egui::TextEdit::singleline(&mut app.home_ssid).hint_text("SSID").desired_width(140.0)).changed() {
                app.save_settings();
            }
            let w = app.wifi_now();
            ui.label(RichText::new(match (w.connected, w.at_home, w.rssi_dbm) {
                (false, _, _) => "wifi: not connected".to_string(),
                (true, true, Some(d)) => format!("wifi: home, {d} dBm"),
                (true, false, Some(d)) => format!("wifi: elsewhere, {d} dBm"),
                (true, h, None) => format!("wifi: {}", if h { "home" } else { "elsewhere" }),
            }).small().weak());
        });
        ui.separator();

        // The rules.
        let mut remove: Option<u32> = None;
        let mut changed = false;
        if app.scheduler.rules.is_empty() {
            ui.label(RichText::new("no schedules yet").weak());
        }
        for r in app.scheduler.rules.iter_mut() {
            ui.horizontal(|ui| {
                if ui.checkbox(&mut r.enabled, "").changed() { changed = true; }
                ui.label(r.describe());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() { remove = Some(r.id); }
                });
            });
        }
        if let Some(id) = remove { app.scheduler.remove(id); changed = true; }
        if changed { app.save_schedules(); }
        ui.separator();

        // The form.
        ui.label(RichText::new("new schedule").strong());
        let f = &mut app.rule_form;
        ui.horizontal(|ui| {
            ui.label("turn");
            ui.add(egui::TextEdit::singleline(&mut f.target).hint_text("kitchen light / lights / music / everything").desired_width(220.0));
            ui.selectable_value(&mut f.on, true, "on");
            ui.selectable_value(&mut f.on, false, "off");
        });
        ui.horizontal_wrapped(|ui| {
            for (i, label) in [(0, "by a time"), (1, "at a time"), (2, "when I get home"), (3, "when I leave"), (4, "when wifi gets weak")] {
                ui.selectable_value(&mut f.trigger, i, label);
            }
        });
        match f.trigger {
            0 | 1 => { ui.horizontal(|ui| { ui.label("time:"); ui.add(egui::TextEdit::singleline(&mut f.time).desired_width(90.0)); }); }
            4 => { ui.add(egui::Slider::new(&mut f.weak_dbm, -90..=-50).text("below dBm")); }
            _ => {}
        }
        let built = f.build();
        ui.horizontal(|ui| {
            let ok = built.is_ok();
            if ui.add_enabled(ok, egui::Button::new("add").min_size(egui::vec2(72.0, 36.0))).clicked() {
                if let Ok((t, a)) = built {
                    app.scheduler.add(t, a);
                    app.save_schedules();
                    app.rule_form = RuleForm::default();
                }
            } else if let Err(e) = &built {
                ui.label(RichText::new(e).small().weak());
            } else if let Ok((t, a)) = &built {
                ui.label(RichText::new(schedule::Rule { id: 0, trigger: t.clone(), action: a.clone(), enabled: true }.describe()).small());
            }
        });
    });
}

/// The confirmation, the code, and the button that stays grey until both
/// are right. Returns nothing; it submits through `App::pair`.
pub fn pairing_dialog(ctx: &egui::Context, app: &mut App) {
    let Some(mut d) = app.pairing.take() else { return };
    let name = app.hub.get(&d.id).map(|x| x.name.clone()).unwrap_or_else(|| d.id.to_string());
    let mut open = true;
    let mut done = false;
    let width = uikit_egui::dialog::dialog_width(ctx.content_rect().width(), 420.0);
    egui::Window::new(format!("Pair with {name}")).open(&mut open).collapsible(false).resizable(false)
        .default_width(width).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(RichText::new("This takes exclusive control of the plug.").strong().size(16.0));
            ui.label("A HomeKit accessory holds ONE pairing. If this plug is set up in an Apple Home, \
                      pairing it here removes it from that Home, and getting it back means a factory \
                      reset of the plug and setting it up again there. If nothing else is paired with it, \
                      nothing is lost.");
            ui.label(RichText::new("The pairing record is stored on this device only. Back it up: losing it means resetting the plug.").small().weak());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("setup code");
                ui.add(egui::TextEdit::singleline(&mut d.code).hint_text("XXX-XX-XXX, printed on the plug").desired_width(160.0));
            });
            let code_ok = switcheroo_core::setup_code_ok(&d.code);
            if !d.code.is_empty() && !code_ok {
                ui.label(RichText::new("eight digits").small().color(egui::Color32::LIGHT_RED));
            }
            ui.checkbox(&mut d.confirmed, "I understand this replaces any pairing the plug already has.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can = code_ok && d.confirmed;
                if ui.add_enabled(can, egui::Button::new(RichText::new("Pair").strong()).min_size(egui::vec2(96.0, 40.0))).clicked() {
                    app.pair(&d.id, &d.code);
                    done = true;
                }
                if ui.button("cancel").clicked() { done = true; }
            });
        });
    if open && !done { app.pairing = Some(d); }
}

/// The tab bar entry the pane needs.
pub fn tab_label(t: Tab) -> &'static str {
    match t { Tab::Devices => "outlets", Tab::Schedules => "schedules", Tab::Activity => "activity", Tab::About => "about" }
}

/// The dialog's state. Public so `ui.rs` can open it from a row.
impl PairingDialog {
    pub fn for_device(id: switcheroo_core::DeviceId) -> Self {
        PairingDialog { id, code: String::new(), confirmed: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_form_builds_each_trigger_and_names_what_is_missing() {
        let mut f = RuleForm::default();
        assert!(f.build().unwrap_err().starts_with("which device"));
        f.target = "Kitchen Light".into();
        let (t, a) = f.build().unwrap();
        assert_eq!(t, Trigger::By { minute_of_day: 21 * 60 });
        assert_eq!(a, Action { target: Target::Named("kitchen light".into()), on: false });
        f.time = "7:30".into();
        f.trigger = 1;
        assert_eq!(f.build().unwrap().0, Trigger::At { minute_of_day: 19 * 60 + 30 }, "bare hours read as evening, like the sentence grammar");
        f.time = "half past".into();
        assert!(f.build().unwrap_err().contains("not understood"));
        f.trigger = 2; assert_eq!(f.build().unwrap().0, Trigger::ArriveHome);
        f.trigger = 3; assert_eq!(f.build().unwrap().0, Trigger::LeaveHome);
        f.trigger = 4; f.weak_dbm = -75;
        assert_eq!(f.build().unwrap().0, Trigger::WifiWeak { below_dbm: -75 });
        f.target = "everything".into(); f.on = true;
        assert_eq!(f.build().unwrap().1, Action { target: Target::All, on: true });
    }
}
