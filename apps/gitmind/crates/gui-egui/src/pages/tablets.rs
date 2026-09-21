//! Tablets page — e-ink tablets (reMarkable) from the phone.
//!
//! The flow the page is built around: plug the tablet into the Fairphone
//! (USB networking answers at 10.11.99.1) or type its Wi-Fi address, type
//! the SSH password shown in the tablet's Settings ONCE, and gitmind pins
//! the host key, installs its key, and reports what the tablet is and how
//! apps reach its screen. After that: pick a built binary, deploy.
//!
//! All logic is `cockpit::eink_fleet` (the same calls `gitmind eink` uses);
//! this page only collects input and shows progress. Blocking work runs on
//! the page's own Task so a slow SSH handshake never freezes the frame, and
//! so a tablet deploy does not queue behind an unrelated build on `app.task`.

use crate::app::App;
use crate::task::Task;
use cockpit::eink_fleet as ef;
use std::sync::{Arc, Mutex};

pub struct State {
    pub host: String,
    /// First contact only. Masked, cleared on success, never persisted.
    pub password: String,
    pub bin_path: String,
    pub app_name: String,
    /// None = the fastest the panel supports (colour panels: fast-color).
    pub profile: Option<ef::RefreshProfile>,
    pub task: Task,
    /// What the last enrol/probe found. Written by the worker thread.
    pub found: Arc<Mutex<Option<(ef::Probe, ef::Displays)>>>,
    pub last_result: String,
    /// Set when enrolment stopped on a changed host key: shows Re-trust.
    pub hostkey_changed: Arc<Mutex<bool>>,
    /// Two-step confirmation for actions that change the tablet itself.
    pub confirm: Option<Confirm>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confirm {
    Activate(ef::Activation),
    KeepHostKey,
}

impl Default for State {
    fn default() -> Self {
        State {
            host: "10.11.99.1".into(),
            password: String::new(),
            bin_path: String::new(),
            app_name: String::new(),
            profile: None,
            task: Task::default(),
            found: Arc::new(Mutex::new(None)),
            last_result: String::new(),
            hostkey_changed: Arc::new(Mutex::new(false)),
            confirm: None,
        }
    }
}

fn describe(p: &ef::Probe, d: &ef::Displays) -> Vec<String> {
    let mut v = vec![
        format!("model: {}", p.model.map(|m| m.label()).unwrap_or("unknown")),
        format!("  identified by: {}", p.how),
        format!("target: {}{}", p.triple.unwrap_or("unknown"), d.glibc.as_ref().map(|g| format!("  ·  glibc {g}")).unwrap_or_default()),
    ];
    if let Some(c) = &p.unknown_codename {
        v.push(format!("  motd names \"{c}\", which gitmind does not know yet"));
    }
    v.push(match d.readiness() {
        Ok((ef::Route::Blight, _)) => "route: Oxide / Blight — gitmind launches apps directly".into(),
        Ok((ef::Route::AppLoad { .. }, _)) => "route: AppLoad — gitmind installs, you tap the icon on the tablet".into(),
        Err(e) => format!("route: none — {e}"),
    });
    match d.readiness() {
        Ok((_, Some(a))) => v.push(format!("since its last boot it needs: {}", a.label())),
        Ok((_, None)) if !d.screen_ready() => v.push("no display server is running (and none gitmind can start)".into()),
        _ => {}
    }
    if d.etc_overlay {
        v.push("system files reset at boot; gitmind only writes under /home/root".into());
    }
    if d.hostkey_volatile {
        v.push("its SSH host key changes at every reboot until made permanent".into());
    }
    v
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let s = &mut app.tablets;
    ui.heading("Tablets");
    ui.label(egui::RichText::new("reMarkable and other e-ink tablets: enrol once, then deploy eink-kit apps").weak().small());
    ui.separator();

    let busy = s.task.is_running();
    // Hints sit on their own line: beside the field they wrap into it on a
    // 360-px phone (the phone_layout test caught exactly that).
    ui.horizontal(|ui| {
        ui.label("address");
        ui.add(egui::TextEdit::singleline(&mut s.host).id_salt("tablets-host").desired_width(150.0));
    });
    ui.label(egui::RichText::new("USB cable: 10.11.99.1 · or the tablet's Wi-Fi address").weak().small());
    ui.horizontal(|ui| {
        ui.label("SSH password");
        ui.add(egui::TextEdit::singleline(&mut s.password).id_salt("tablets-pw").password(true).desired_width(150.0));
    });
    ui.label(egui::RichText::new("first time only · shown on the tablet under Settings → Help → Copyrights and licenses").weak().small());
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && !s.host.trim().is_empty(), egui::Button::new("Enrol")).clicked() {
            let host = s.host.trim().to_string();
            let pw = std::mem::take(&mut s.password); // consumed, not kept
            let found = s.found.clone();
            let changed = s.hostkey_changed.clone();
            s.task.run_logged(move |log| {
                let mut e = ef::SshEnroller { host: host.clone() };
                *changed.lock().unwrap() = false;
                let pw = Some(pw).filter(|p| !p.is_empty());
                match ef::enroll(&mut e, pw.as_deref(), &mut |ev| log.push(&format!("{ev:?}"))) {
                    Ok(r) => {
                        let lines = describe(&r.probe, &r.displays).join("\n");
                        *found.lock().unwrap() = Some((r.probe, r.displays));
                        format!("enrolled root@{host}\n{lines}")
                    }
                    Err(e) => {
                        if e == ef::HOSTKEY_CHANGED {
                            *changed.lock().unwrap() = true;
                        }
                        format!("enrol failed: {e}")
                    }
                }
            });
        }
        if ui.add_enabled(!busy && !s.host.trim().is_empty(), egui::Button::new("Probe")).clicked() {
            let host = s.host.trim().to_string();
            let found = s.found.clone();
            s.task.run_logged(move |_| {
                let mut r = ef::SshRemote { user: "root".into(), host: host.clone() };
                let probe = ef::Remote::run(&mut r, ef::probe_cmd()).map(|o| ef::parse_probe(&o));
                let disp = ef::Remote::run(&mut r, ef::displays_probe_cmd()).map(|o| ef::parse_displays(&o));
                match (probe, disp) {
                    (Ok(p), Ok(d)) => {
                        let lines = describe(&p, &d).join("\n");
                        *found.lock().unwrap() = Some((p, d));
                        format!("{host}\n{lines}")
                    }
                    (Err(e), _) | (_, Err(e)) => format!("probe failed: {e} (enrolled yet?)"),
                }
            });
        }
    });

    if *s.hostkey_changed.lock().unwrap() {
        ui.label(egui::RichText::new("The tablet's host key changed. Re-trust only if you know why \
            (e.g. you just rebooted it on your own cable).").strong());
        if ui.add_enabled(!busy, egui::Button::new("Re-trust this tablet")).clicked() {
            let (host, changed) = (s.host.trim().to_string(), s.hostkey_changed.clone());
            s.task.run_logged(move |_| match cockpit::deploy::retrust_host(&host) {
                Ok(m) => {
                    *changed.lock().unwrap() = false;
                    format!("{m}\nnow Enrol again, then make the key permanent")
                }
                Err(e) => format!("re-trust failed: {e}"),
            });
        }
    }

    let found = s.found.lock().unwrap().clone();
    if let Some((p, d)) = &found {
        for line in describe(p, d) {
            ui.label(line);
        }
        // Per-boot activation and host-key persistence: both change the
        // tablet, so each takes a second, explicit tap.
        let mut actions: Vec<(Confirm, String)> = Vec::new();
        if let Ok((_, Some(a))) = d.readiness() {
            actions.push((Confirm::Activate(a), format!("Activate: {}", a.label())));
        }
        if d.hostkey_volatile {
            actions.push((Confirm::KeepHostKey, "Keep this host key across reboots".into()));
        }
        for (c, label) in actions {
            if s.confirm == Some(c) {
                let warn = match c {
                    Confirm::Activate(ef::Activation::StartXovi) => "The tablet's screen will restart for a few seconds.",
                    Confirm::Activate(_) => "The tablet switches to its launcher.",
                    Confirm::KeepHostKey => "This briefly makes the tablet's system writable and copies its SSH host key into /etc.",
                };
                ui.label(egui::RichText::new(warn).small());
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Confirm")).clicked() {
                        let host = s.host.trim().to_string();
                        s.task.run_logged(move |_| {
                            let mut r = ef::SshRemote { user: "root".into(), host };
                            let cmd = match c {
                                Confirm::Activate(a) => ef::activation_cmd(a),
                                Confirm::KeepHostKey => ef::persist_host_keys_cmd().to_string(),
                            };
                            match ef::Remote::run(&mut r, &cmd) {
                                Ok(out) => format!("{}\n(Probe again to refresh)", out.trim()),
                                Err(e) => format!("failed: {e}"),
                            }
                        });
                        s.confirm = None;
                    }
                    if ui.button("Cancel").clicked() {
                        s.confirm = None;
                    }
                });
            } else if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                s.confirm = Some(c);
            }
        }
    }

    ui.separator();
    ui.label(egui::RichText::new("deploy").strong());
    // Full width and wrapping: build paths are long, and a clipped path
    // hides exactly the part that says which build you are about to push.
    ui.label("binary");
    ui.add(egui::TextEdit::multiline(&mut s.bin_path).id_salt("tablets-bin").desired_rows(2)
        .desired_width(f32::INFINITY).hint_text("path to an aarch64 / armv7 build"));
    ui.horizontal_wrapped(|ui| {
        ui.label("name");
        ui.add(egui::TextEdit::singleline(&mut s.app_name).id_salt("tablets-name").desired_width(140.0)
            .hint_text("defaults to the file name"));
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("refresh");
        ui.radio_value(&mut s.profile, None, "fastest for the panel");
        ui.radio_value(&mut s.profile, Some(ef::RefreshProfile::FastColor), "fast colour");
        ui.radio_value(&mut s.profile, Some(ef::RefreshProfile::FastMono), "fast mono");
        ui.radio_value(&mut s.profile, Some(ef::RefreshProfile::Quality), "quality");
    });
    let name = if s.app_name.trim().is_empty() {
        std::path::Path::new(s.bin_path.trim()).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        s.app_name.trim().to_string()
    };
    let name_ok = ef::valid_app_name(&name);
    if !s.bin_path.trim().is_empty() && !name_ok {
        ui.label(egui::RichText::new(format!("\"{name}\" is not a valid app name: lowercase letters, digits, - and _ only")).small());
    }
    ui.horizontal(|ui| {
        let can = !busy && found.is_some() && !s.bin_path.trim().is_empty() && name_ok;
        if ui.add_enabled(can, egui::Button::new("Deploy")).clicked() {
            let (host, bin, name, choice) = (s.host.trim().to_string(), s.bin_path.trim().to_string(), name.clone(), s.profile);
            s.task.run_logged(move |log| {
                // Re-probe: the route may have changed since enrolment.
                let mut r = ef::SshRemote { user: "root".into(), host: host.clone() };
                let p = match ef::Remote::run(&mut r, ef::probe_cmd()) { Ok(o) => ef::parse_probe(&o), Err(e) => return format!("probe failed: {e}") };
                let d = match ef::Remote::run(&mut r, ef::displays_probe_cmd()) { Ok(o) => ef::parse_displays(&o), Err(e) => return format!("probe failed: {e}") };
                let (route, activation) = match d.readiness() { Ok(r) => r, Err(e) => return e };
                if let (ef::Route::Blight, Some(a)) = (&route, activation) {
                    return format!("the tablet needs \"{}\" since its last boot — use Activate above", a.label());
                }
                let profile = choice.unwrap_or_else(|| ef::RefreshProfile::fastest(p.model.map(|m| m.panel()).unwrap_or(ef::Panel::Mono)));
                let env = ef::LaunchEnv { profile, model: p.model, blight_lib: d.blight_lib.clone() };
                match ef::deliver(&mut r, &route, &name, &bin, &env, &[], &mut |ev| log.push(&format!("{ev:?}"))) {
                    Ok(ef::Delivered::Launched(ef::Outcome::Running { pid })) => format!("{name} running (pid {pid}, {})", profile.as_env()),
                    Ok(ef::Delivered::Launched(ef::Outcome::ExitedEarly { log_tail })) => format!("{name} exited right after launch:\n{log_tail}"),
                    Ok(ef::Delivered::Registered { .. }) if activation.is_some() =>
                        format!("{name} installed — activate xovi (above), then open AppLoad on the tablet and tap it"),
                    Ok(ef::Delivered::Registered { .. }) => format!("{name} installed — open AppLoad on the tablet and tap it"),
                    Err(e) => format!("deploy failed: {e}"),
                }
            });
        }
        for (label, stop) in [("Status", false), ("Stop", true)] {
            if ui.add_enabled(!busy && found.is_some() && name_ok, egui::Button::new(label)).clicked() {
                let (host, name) = (s.host.trim().to_string(), name.clone());
                s.task.run_logged(move |_| {
                    let mut r = ef::SshRemote { user: "root".into(), host };
                    let cmd = if stop { ef::stop_cmd(&name) } else { ef::status_cmd(&name, 0) };
                    match cmd.and_then(|c| ef::Remote::run(&mut r, &c)) {
                        Ok(_) if stop => format!("{name}: stopped"),
                        Ok(out) => {
                            let st = ef::parse_status(&out);
                            let head = match st.pid { Some(p) => format!("{name}: running (pid {p})"), None => format!("{name}: stopped") };
                            if st.log_tail.is_empty() { head } else { format!("{head}\n{}", st.log_tail) }
                        }
                        Err(e) => format!("{label} failed: {e}"),
                    }
                });
            }
        }
    });

    if let Some(r) = s.task.take_result() {
        s.last_result = r;
    }
    if busy {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        uikit_egui::scroll::log(ui, &s.task.log(), 140.0);
    }
    if !s.last_result.is_empty() {
        ui.separator();
        for line in s.last_result.lines() {
            ui.label(line);
        }
    }
}
