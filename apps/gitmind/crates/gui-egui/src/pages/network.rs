//! Network page — the fleet's daemons + a target device, calling cockpit directly (no JSON marshal
//! layer the Qt frontend needed). Per-daemon lifecycle control, one-shot logs, and a *live* log
//! stream (via the Task's run_logged + cancel). Remote actions run on the background Task so the
//! frame never freezes.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub user: String,
    pub host: String,
    pub note: String,
    /// Which daemon is currently streaming ("" = none), so the toggle knows its label.
    pub streaming_name: String,
    /// A session roster of devices (name, user, host) to roll daemon health up across.
    pub health_targets: Vec<(String, String, String)>,
    /// Third-party services discovered on a device (no fleet marker): (name, description).
    pub other_services: Vec<(String, String)>,
    /// Per-daemon up/down from the last `status` read, and the host it was read from.
    ///
    /// `daemons::parse_status` has been pure and tested since the module was written and
    /// nothing on this page called it: the ● beside each daemon was a decoration, the same
    /// colour whether the service was running or not, and the only way to learn the truth
    /// was to press `status` and read a block of raw text at the bottom of the page. A
    /// control panel whose lights do not mean anything is worse than one with no lights.
    pub daemon_up: std::collections::HashMap<String, bool>,
    /// Which host `daemon_up` describes. Cleared when the target changes, because showing
    /// one device's daemons as the state of another is the failure this guards against.
    pub status_host: String,
    /// Set when an action ran that changes what the lights should say, so the page re-reads
    /// rather than leaving a stale dot next to a service you just started.
    pub status_stale: bool,
    /// A status read is in flight, so its result is a status block rather than the output
    /// of whatever else the shared Task last ran. Without this the page would try to read
    /// daemon states out of a build log.
    pub status_pending: bool,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Network");
    ui.separator();

    // WRAPS, and both fields are sized. `ui.horizontal()` does not wrap, so on a phone the
    // `host` field was drawn past the right edge — the photo shows the label with no box
    // after it. See `narrow`.
    crate::narrow::field_pair(
        ui,
        ("user", "network-app_net_user", &mut app.net.user),
        ("host", "network-app_net_host", &mut app.net.host),
    );
    let idle = !app.task.is_running();
    let can = idle && !app.net.host.trim().is_empty();

    ui.add_space(6.0);
    ui.label(egui::RichText::new("fleet daemons · by the developer").strong());
    crate::narrow::desc(ui, "first-party services with core-kit integrations — auto-discovered via the X-Fleet-Integration unit marker");
    for d in cockpit::daemons::all() {
        // The description is the part that ran off the screen — 447 points of it on a
        // 360-point phone. On a narrow screen it drops to its own wrapped line under the
        // name; on a laptop it stays inline. The chip is unchanged.
        // THE LIGHT MEANS SOMETHING NOW. Green when the last status read said the service
        // is up, red when it said down, hollow when nothing has been read for this host.
        // "Not asked" is drawn differently from "asked and it is down" on purpose: a grey
        // dot that could mean either is what made the old one useless.
        let (glyph, tint) = match app.net.daemon_up.get(d.name) {
            _ if app.net.status_host != app.net.host || app.net.host.trim().is_empty() => {
                ("○", ui.visuals().weak_text_color())
            }
            Some(true) => ("●", egui::Color32::from_rgb(0x6d, 0xc2, 0x7a)),
            Some(false) => ("●", egui::Color32::from_rgb(0xd0, 0x6b, 0x5e)),
            None => ("○", ui.visuals().weak_text_color()),
        };
        let _ = tint;
        crate::narrow::titled_row(
            ui,
            &format!("{glyph} {}", d.name),
            Some((
                d.integration,
                egui::Color32::from_rgb(0x2a, 0x3a, 0x5a),
                egui::Color32::from_rgb(0x9a, 0xc2, 0xff),
            )),
            d.description,
            |_| {},
        );
        ui.horizontal(|ui| {
            for action in ["start", "stop", "restart"] {
                if ui.add_enabled(can, egui::Button::new(action).small()).clicked() {
                    let (u, h, n, a) =
                        (app.net.user.clone(), app.net.host.clone(), d.name.to_string(), action.to_string());
                    app.net.status_stale = true;
                    app.task.run(move || match cockpit::flow::daemon_control_remote(&u, &h, &n, &a) {
                        Ok(o) => format!("{n} {a}: {o}"),
                        Err(e) => format!("error: {e}"),
                    });
                }
            }
            // ENABLE AT BOOT. Start/stop/restart all last until the phone is rebooted, and
            // nothing on this page said so — someone who fixed a dead daemon and pocketed the
            // phone found it dead again. `daemons::enable_cmd` has existed since the module
            // was written with no caller anywhere.
            if ui
                .add_enabled(can, egui::Button::new("enable").small())
                .on_hover_text("start now AND at boot")
                .clicked()
            {
                let (u, h, n) = (app.net.user.clone(), app.net.host.clone(), d.name.to_string());
                app.net.status_stale = true;
                app.task.run(move || match cockpit::flow::daemon_enable_remote(&u, &h, &n) {
                    Ok(o) => format!("{n} enabled at boot: {o}"),
                    Err(e) => format!("error: {e}"),
                });
            }
            if ui.add_enabled(can, egui::Button::new("logs").small()).clicked() {
                let (u, h, n) = (app.net.user.clone(), app.net.host.clone(), d.name.to_string());
                app.task.run(move || match cockpit::flow::daemon_logs_remote(&u, &h, &n, 200) {
                    Ok(o) => o,
                    Err(e) => format!("error: {e}"),
                });
            }
            let streaming_this = app.net.streaming_name == d.name;
            let label = if streaming_this { "◼ stop" } else { "▶ stream" };
            let stream_enabled = !app.net.host.trim().is_empty() && (idle || streaming_this);
            if ui.add_enabled(stream_enabled, egui::Button::new(label).small()).clicked() {
                if streaming_this {
                    app.task.cancel();
                    app.net.streaming_name.clear();
                } else if idle {
                    let (u, h, n) = (app.net.user.clone(), app.net.host.clone(), d.name.to_string());
                    let cancel = app.task.cancel_flag();
                    app.net.streaming_name = d.name.to_string();
                    app.task.run_logged(move |sink| {
                        let mut on_line = |l: &str| sink.push(l);
                        match cockpit::flow::stream_daemon_logs(&u, &h, &n, cancel, &mut on_line) {
                            Ok(()) => "— stream ended —".to_string(),
                            Err(e) => format!("error: {e}"),
                        }
                    });
                }
            }
        });
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(can, egui::Button::new("status")).clicked() {
            let (u, h) = (app.net.user.clone(), app.net.host.clone());
            app.net.status_pending = true;
            app.net.status_stale = false;
            app.task.run(move || match cockpit::flow::daemons_status_remote(&u, &h) {
                Ok(o) => o,
                Err(e) => format!("error: {e}"),
            });
        }
        if ui.add_enabled(can, egui::Button::new("install + enable")).clicked() {
            let (u, h) = (app.net.user.clone(), app.net.host.clone());
            let (bin, home) = (format!("/home/{u}/.local/bin"), format!("/home/{u}"));
            app.task.run(move || match cockpit::flow::install_daemons_remote(&u, &h, &bin, &home) {
                Ok(o) => o,
                Err(e) => format!("error: {e}"),
            });
        }
    });

    // Third-party services: anything on the system WITHOUT the X-Fleet-Integration marker. These
    // aren't by the developer and carry no core-kit integration, so they're listed apart from the
    // fleet daemons above. Populated by a scan on the device (units whose body fails
    // cockpit::daemons::is_fleet_daemon); gitmind manages the fleet set, not these.
    ui.add_space(8.0);
    ui.label(egui::RichText::new("other services · third-party").strong());
    if app.net.other_services.is_empty() {
        crate::narrow::desc(ui, "none discovered — run a scan on a device; services without the fleet marker appear here");
    } else {
        for (name, description) in &app.net.other_services {
            crate::narrow::titled_row(ui, &format!("○ {name}"), None, description, |_| {});
        }
    }

    // fleet health: roll daemon status up across a roster of devices
    ui.add_space(6.0);
    ui.label(egui::RichText::new("fleet health").strong());
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!app.net.host.trim().is_empty(), egui::Button::new("add this device").small())
            .clicked()
        {
            let (n, u, h) = (app.net.host.clone(), app.net.user.clone(), app.net.host.clone());
            if !app.net.health_targets.iter().any(|(_, _, hh)| *hh == h) {
                app.net.health_targets.push((n, u, h));
            }
        }
        if ui
            .add_enabled(idle && !app.net.health_targets.is_empty(), egui::Button::new("refresh").small())
            .clicked()
        {
            let targets = app.net.health_targets.clone();
            app.task.run(move || {
                let mut out = String::new();
                for d in cockpit::flow::fleet_health_remote(&targets) {
                    if d.reachable {
                        let up = d.daemons.iter().filter(|(_, u)| *u).count();
                        out.push_str(&format!("[up] {} — {}/{} up\n", d.name, up, d.daemons.len()));
                    } else {
                        out.push_str(&format!("[!!] {} — unreachable\n", d.name));
                    }
                }
                if out.is_empty() {
                    out.push_str("(no devices in roster)");
                }
                out
            });
        }
    });
    if !app.net.health_targets.is_empty() {
        let roster: Vec<&str> = app.net.health_targets.iter().map(|(n, _, _)| n.as_str()).collect();
        ui.label(egui::RichText::new(format!("roster: {}", roster.join(", "))).weak().small());
    }
    let lines = app.task.log();
    if !lines.is_empty() || !app.net.streaming_name.is_empty() {
        ui.add_space(6.0);
        crate::show::task_log(ui, "network", &lines, 180.0);
    }

    if let Some(r) = app.task.take_result() {
        // Only a STATUS result feeds the lights. The Task is shared by every button on
        // this page, so parsing whatever finished would let a log dump or an install
        // transcript set the dots — `parse_status` skips lines it does not recognise, so
        // the damage would be silent and partial rather than obvious.
        if std::mem::take(&mut app.net.status_pending) && !r.starts_with("error:") {
            app.net.daemon_up = cockpit::daemons::parse_status(&r).into_iter().collect();
            app.net.status_host = app.net.host.clone();
        }
        app.net.note = r;
        app.net.streaming_name.clear(); // whatever ran is finished
    }
    // A start/stop/restart changed what the lights should say. Re-read once, automatically:
    // pressing "start" and then having to press "status" to find out whether it worked is
    // the page asking the operator to do its job.
    if app.net.status_stale && !app.task.is_running() && !app.net.host.trim().is_empty() {
        app.net.status_stale = false;
        app.net.status_pending = true;
        let (u, h) = (app.net.user.clone(), app.net.host.clone());
        app.task.run(move || match cockpit::flow::daemons_status_remote(&u, &h) {
            Ok(o) => o,
            Err(e) => format!("error: {e}"),
        });
    }
    if app.task.is_running() {
        ui.ctx().request_repaint();
    }
    if !app.net.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.net.note).monospace().small());
    }

    // Bluetooth OBEX push lives on the Network page, as it does in the Qt GUI:
    // it is the answer to "these two machines share no LAN", which is the
    // question this whole page is about.
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    super::bluetooth::section(app, ui);

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    super::phone_setup::section(app, ui);

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Tab};

    /// Drive the page for one frame at phone size and return the frame's text.
    fn frame(app: &mut App) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 760.0)));
        let out = ctx.run(input, |c| app.ui(c));
        uikit_egui::layout_probe::texts(&out.shapes).into_iter().map(|t| t.text).collect()
    }

    fn dot_for(texts: &[String], name: &str) -> Option<char> {
        texts.iter().find(|t| t.contains(name)).and_then(|t| t.chars().next())
    }

    /// The light beside a daemon says what the last status read said — and says "not
    /// asked" differently from "asked, and it is down".
    ///
    /// `daemons::parse_status` was pure and tested from the day the module was written, and
    /// this page never called it: every ● was the same colour whether the service was
    /// running or not.
    #[test]
    fn a_daemons_light_reflects_the_last_status_read() {
        let mut app = App::default();
        app.tab = Tab::Network;
        app.net.host = "phone".into();

        // nothing read yet: hollow
        let t = frame(&mut app);
        assert_eq!(dot_for(&t, "voiced"), Some('○'), "an unread daemon must not look up OR down");

        // a status block arrives
        app.net.daemon_up = cockpit::daemons::parse_status("voiced: active\nsharemd: inactive\n")
            .into_iter()
            .collect();
        app.net.status_host = "phone".into();
        let t = frame(&mut app);
        assert_eq!(dot_for(&t, "voiced"), Some('●'), "a running daemon is filled");
        assert_eq!(dot_for(&t, "sharemd"), Some('●'), "a stopped daemon is filled too — the COLOUR differs");
        // a daemon the block did not mention is unread, not down
        assert_eq!(dot_for(&t, "settingsd"), Some('○'));

        // POINTING AT ANOTHER DEVICE INVALIDATES THE READING. Showing one phone's daemons
        // as another's is the mistake this guards: the lights would be confidently wrong.
        app.net.host = "laptop".into();
        let t = frame(&mut app);
        assert_eq!(dot_for(&t, "voiced"), Some('○'), "a stale host's reading must not be shown");
    }

    /// A control action marks the reading stale so the page re-reads on its own. Pressing
    /// "start" and then having to press "status" to find out whether it worked is the page
    /// asking the operator to do its job.
    #[test]
    fn starting_a_daemon_schedules_a_fresh_read() {
        let mut app = App::default();
        app.tab = Tab::Network;
        app.net.host = "phone".into();
        app.net.status_stale = true;
        assert!(!app.task.is_running());
        let _ = frame(&mut app);
        // the page kicked the read itself; `status_pending` marks the result as a status
        // block rather than whatever else the shared Task might have run
        assert!(app.net.status_pending, "no re-read was scheduled");
        assert!(!app.net.status_stale, "the flag must be consumed, or it re-reads every frame");
    }

    /// The shared Task runs logs, installs and builds too. Only a status result may set the
    /// lights — `parse_status` skips lines it does not recognise, so feeding it a log would
    /// corrupt the dots silently and partially.
    #[test]
    fn a_log_dump_does_not_set_the_lights() {
        let mut app = App::default();
        app.tab = Tab::Network;
        app.net.host = "phone".into();
        app.net.daemon_up.insert("voiced".into(), true);
        app.net.status_host = "phone".into();
        app.net.status_pending = false;
        // a plausible log line that parse_status would happily read as `name: state`
        app.task.run(|| "voiced: stopped by the operator at 14:02\n".to_string());
        while app.task.is_running() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let _ = frame(&mut app);
        assert_eq!(app.net.daemon_up.get("voiced"), Some(&true), "a log line set a daemon light");
    }
}
