//! Tablets page — e-ink tablets (reMarkable) from the phone.
//!
//! The flow the page is built around: plug the tablet into the Fairphone
//! (USB networking answers at 10.11.99.1) or type its Wi-Fi address, type
//! the SSH password shown in the tablet's Settings ONCE, and gitmind pins
//! the host key, installs its key, and reports what the tablet is and how
//! apps reach its screen. After that: pick an eink-kit app from this
//! checkout, build it for that tablet (on a postmarketOS phone the AppLoad
//! route builds natively), and deploy. On the AppLoad route the app gets a
//! manifest, a sidebar title and a letter-tile icon, so it appears in
//! xochitl's sidebar through AppLoad's injected menu; gitmind can list and
//! remove the apps it put there, and never touches anyone else's.
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
    /// SSH user. Default `root` (every model today); a field because
    /// reMarkable documents it as "root at the time of writing".
    pub user: String,
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
    /// eink-kit apps in this checkout (discovered on first view).
    pub apps: Option<Vec<ef::TabletApp>>,
    pub selected: usize,
    /// (binary path, bin name) from the last successful build.
    pub built: Arc<Mutex<Option<(String, String)>>>,
    /// Host toolchain, container (podman/docker), or auto.
    pub engine: ef::Engine,
    /// AppLoad sidebar title; empty = the app name.
    pub display_name: String,
    /// Allow AppLoad's windowed mode (default off: fullscreen, crisp).
    pub windowed: bool,
    /// gitmind's apps in the tablet's AppLoad: (name, title).
    pub installed: Arc<Mutex<Vec<(String, String)>>>,
    pub remove_confirm: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confirm {
    Activate(ef::Activation),
    /// `rm-ssh-over-wlan on`.
    WifiSsh,
    /// Restart xochitl so AppLoad re-reads changed manifests.
    RestartUi,
}

impl Default for State {
    fn default() -> Self {
        State {
            host: "10.11.99.1".into(),
            user: cockpit::eink_fleet::DEFAULT_USER.into(),
            password: String::new(),
            bin_path: String::new(),
            app_name: String::new(),
            profile: None,
            task: Task::default(),
            found: Arc::new(Mutex::new(None)),
            last_result: String::new(),
            hostkey_changed: Arc::new(Mutex::new(false)),
            confirm: None,
            apps: None,
            selected: 0,
            built: Arc::new(Mutex::new(None)),
            engine: ef::Engine::Auto,
            display_name: String::new(),
            windowed: false,
            installed: Arc::new(Mutex::new(Vec::new())),
            remove_confirm: None,
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
    if let Some(a) = &d.wlan_addr {
        v.push(format!("Wi-Fi address: {a}"));
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
        ui.label("user");
        ui.add(egui::TextEdit::singleline(&mut s.user).id_salt("tablets-user").desired_width(150.0));
    });
    ui.horizontal(|ui| {
        ui.label("SSH password");
        ui.add(egui::TextEdit::singleline(&mut s.password).id_salt("tablets-pw").password(true).desired_width(150.0));
    });
    ui.label(egui::RichText::new("first time only · on the tablet: Settings → General → Help → About → Copyrights and licenses, \
        under General information (username and password; needs developer mode)").weak().small());
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && !s.host.trim().is_empty(), egui::Button::new("Enrol")).clicked() {
            let host = s.host.trim().to_string();
            let user = s.user.trim().to_string();
            let pw = std::mem::take(&mut s.password); // consumed, not kept
            let found = s.found.clone();
            let changed = s.hostkey_changed.clone();
            s.task.run_logged(move |log| {
                let mut e = ef::SshEnroller { user: user.clone(), host: host.clone() };
                *changed.lock().unwrap() = false;
                let pw = Some(pw).filter(|p| !p.is_empty());
                match ef::enroll(&mut e, pw.as_deref(), &mut |ev| log.push(&format!("{ev:?}"))) {
                    Ok(r) => {
                        let mut lines = describe(&r.probe, &r.displays).join("\n");
                        *found.lock().unwrap() = Some((r.probe, r.displays));
                        // What the device HAS, stored for builds to pick features from.
                        let mut rr = ef::SshRemote { user: user.clone(), host: host.clone() };
                        if let Ok(out) = ef::Remote::run(&mut rr, &cockpit::capabilities::survey_cmd()) {
                            let _ = cockpit::capabilities::save(&host, &out);
                            for l in cockpit::capabilities::parse(&out).summary() { lines.push('\n'); lines.push_str(&l); }
                        }
                        format!("enrolled {user}@{host}\n{lines}")
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
            let user = s.user.trim().to_string();
            let found = s.found.clone();
            s.task.run_logged(move |_| {
                let mut r = ef::SshRemote { user: user.clone(), host: host.clone() };
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
        ui.label(egui::RichText::new("The tablet's host key changed. That happens after an OS update; \
            re-trust only if you know it was updated.").strong());
        if ui.add_enabled(!busy, egui::Button::new("Re-trust this tablet")).clicked() {
            let (host, changed) = (s.host.trim().to_string(), s.hostkey_changed.clone());
            s.task.run_logged(move |_| match cockpit::deploy::retrust_host(&host) {
                Ok(m) => {
                    *changed.lock().unwrap() = false;
                    format!("{m}\nnow Enrol again")
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
        if let Some(a) = d.wlan_addr.clone().filter(|a| *a != s.host.trim()) {
            if ui.add_enabled(!busy, egui::Button::new(format!("Use Wi-Fi address {a}"))).clicked() {
                s.host = a;
            }
        }
        // Per-boot activation changes the tablet (restarts its UI or
        // switches launcher), so it takes a second, explicit tap.
        let mut actions: Vec<(Confirm, String)> = Vec::new();
        if let Ok((_, Some(a))) = d.readiness() {
            actions.push((Confirm::Activate(a), format!("Activate: {}", a.label())));
        }
        if d.wlan_ssh_tool {
            actions.push((Confirm::WifiSsh, "Enable SSH over Wi-Fi".into()));
        }
        if d.appload_dir.is_some() {
            actions.push((Confirm::RestartUi, "Restart UI (apply changed app settings)".into()));
        }
        for (c, label) in actions {
            if s.confirm == Some(c) {
                let warn = match c {
                    Confirm::Activate(ef::Activation::StartXovi) => "The tablet's screen will restart for a few seconds.",
                    Confirm::Activate(_) => "The tablet switches to its launcher.",
                    Confirm::RestartUi => "Restarts xochitl: whatever is open on the tablet closes; AppLoad re-reads every app's settings.",
                    Confirm::WifiSsh => "Runs rm-ssh-over-wlan on: the tablet then accepts SSH on its Wi-Fi address (shown under the password in Copyrights and licenses).",
                };
                ui.label(egui::RichText::new(warn).small());
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Confirm")).clicked() {
                        let host = s.host.trim().to_string();
                        let user = s.user.trim().to_string();
                        s.task.run_logged(move |_| {
                            let mut r = ef::SshRemote { user: user.clone(), host };
                            let cmd = match c {
                                Confirm::Activate(a) => ef::activation_cmd(a),
                                Confirm::WifiSsh => ef::WLAN_SSH_ON.to_string(),
                                Confirm::RestartUi => ef::RESTART_UI_CMD.to_string(),
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
    ui.label(egui::RichText::new("build").strong());
    // eink-kit apps in this checkout, found once per session.
    if s.apps.is_none() {
        s.apps = Some(ef::discover_tablet_apps(&cockpit::ops::repo_root()));
    }
    let apps = s.apps.clone().unwrap_or_default();
    if apps.is_empty() {
        ui.label(egui::RichText::new("no eink-kit apps found in this checkout (GITMIND_ROOT)").weak().small());
    } else {
        s.selected = s.selected.min(apps.len() - 1);
        egui::ComboBox::from_id_salt("tablets-app").selected_text(&apps[s.selected].bin).show_ui(ui, |ui| {
            for (i, a) in apps.iter().enumerate() {
                ui.selectable_value(&mut s.selected, i, &a.bin);
            }
        });
        let route = found.as_ref().and_then(|(p, d)| d.readiness().ok().map(|(r, _)| (r, p.model)));
        if let Some((r, m)) = &route {
            let t = ef::build_target(r, *m);
            let native = ef::native_for(&t);
            ui.label(egui::RichText::new(format!("{}{}", t.triple, if native { " (native on this phone)" } else { "" })).small());
            ui.label(egui::RichText::new(t.why).weak().small());
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("build with");
            ui.radio_value(&mut s.engine, ef::Engine::Auto, "auto");
            ui.radio_value(&mut s.engine, ef::Engine::Host, "host toolchain");
            ui.radio_value(&mut s.engine, ef::Engine::Container, "container (podman)");
        });
        if ui.add_enabled(!busy && route.is_some(), egui::Button::new("Build for this tablet")).clicked() {
            let (r, m) = route.clone().unwrap();
            let app = apps[s.selected].clone();
            let engine = s.engine;
            let host = s.host.trim().to_string();
            let built = s.built.clone();
            s.task.run_logged(move |log| {
                let t = ef::build_target(&r, m);
                let path = std::env::var("PATH").unwrap_or_default();
                // Features follow the device's surveyed hardware (enrolment stores it).
                let caps = cockpit::capabilities::load(&host);
                if caps.is_none() { log.push("no hardware survey for this tablet yet: default features (enrol again, or `gitmind eink survey`)"); }
                let plan = match ef::plan_build_for(engine, &cockpit::ops::repo_root(), &app, &t, &path,
                    std::env::var("HOME").ok().as_deref(), std::env::var("CARGO").ok().as_deref(), caps.as_ref()) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                log.push(&format!("{}", plan.note));
                log.push(&format!("$ {}  (in {})", plan.argv.join(" "), plan.cwd.display()));
                let mut errors: Vec<String> = Vec::new();
                // The shim patch rewrites Cargo.lock: saved and restored around the build.
                let status = ef::with_lock_preserved(&plan, || -> Result<std::process::ExitStatus, String> {
                let mut cmd = std::process::Command::new(&plan.argv[0]);
                cmd.args(&plan.argv[1..]).current_dir(&plan.cwd).envs(plan.env.clone())
                    .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
                let mut child = match cmd.spawn() { Ok(c) => c, Err(e) => return Err(format!("could not run {}: {e}", plan.argv[0])) };
                // cargo (and podman) report progress on stderr; stream it.
                if let Some(err) = child.stderr.take() {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                        let tl = line.trim_start();
                        if tl.starts_with("error") || tl.starts_with("= note") || tl.starts_with("--> ") || tl.contains("exec format error") || tl.starts_with("Error:") {
                            errors.push(line.clone());
                        }
                        log.push(&line);
                    }
                }
                    child.wait().map_err(|e| format!("cargo: {e}"))
                });
                let status = match status { Ok(s) => s, Err(e) => return e };
                if !status.success() {
                    // cargo's own error lines go into the result, where
                    // diagnose() turns them into findings.
                    let first: Vec<String> = errors.into_iter().take(12).collect();
                    return format!("build failed (exit {:?}) for {}\n{}", status.code(), t.triple, first.join("\n"));
                }
                let Some(path) = [plan.output.clone()].into_iter().chain(
                    // a host build in a workspace member lands in the workspace's target dir
                    app.dir.ancestors().map(|d| d.join(ef::tablet_build_output(&app.bin, &t, ef::native_for(&t))))
                ).find(|p| p.exists()) else { return format!("built, but {} was not found", plan.output.display()) };
                let path = path.to_string_lossy().to_string();
                match ef::preflight_binary(&path, &r, m) {
                    Ok(_) => {
                        *built.lock().unwrap() = Some((path.clone(), app.bin.clone()));
                        format!("built {path}\nready to deploy")
                    }
                    Err(e) => format!("built, but it will not run on the tablet: {e}"),
                }
            });
        }
    }
    if let Some((path, bin)) = s.built.lock().unwrap().take() {
        s.bin_path = path;
        s.app_name = bin.clone();
        if s.display_name.is_empty() {
            s.display_name = bin;
        }
    }

    ui.separator();
    ui.label(egui::RichText::new("deploy").strong());
    // Full width and wrapping: build paths are long, and a clipped path
    // hides exactly the part that says which build you are about to push.
    ui.label("binary");
    ui.add(egui::TextEdit::multiline(&mut s.bin_path).id_salt("tablets-bin").desired_rows(2)
        .desired_width(f32::INFINITY).hint_text("built above, or a path to an aarch64 / armv7 build"));
    ui.horizontal_wrapped(|ui| {
        ui.label("name");
        ui.add(egui::TextEdit::singleline(&mut s.app_name).id_salt("tablets-name").desired_width(140.0)
            .hint_text("defaults to the file name"));
    });
    let appload_route = matches!(found.as_ref().and_then(|(_, d)| d.readiness().ok()), Some((ef::Route::AppLoad { .. }, _)));
    if appload_route {
        ui.horizontal_wrapped(|ui| {
            ui.label("sidebar title");
            ui.add(egui::TextEdit::singleline(&mut s.display_name).id_salt("tablets-title").desired_width(140.0)
                .hint_text("as shown in AppLoad"));
        });
        ui.checkbox(&mut s.windowed, "allow windowed mode (scaled down; softer text)");
    }
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
            let user = s.user.trim().to_string();
            let title = if s.display_name.trim().is_empty() { name.clone() } else { s.display_name.trim().to_string() };
            let fullscreen = !s.windowed;
            s.task.run_logged(move |log| {
                // Re-probe: the route may have changed since enrolment.
                let mut r = ef::SshRemote { user: user.clone(), host: host.clone() };
                let p = match ef::Remote::run(&mut r, ef::probe_cmd()) { Ok(o) => ef::parse_probe(&o), Err(e) => return format!("probe failed: {e}") };
                let d = match ef::Remote::run(&mut r, ef::displays_probe_cmd()) { Ok(o) => ef::parse_displays(&o), Err(e) => return format!("probe failed: {e}") };
                let (route, activation) = match d.readiness() { Ok(r) => r, Err(e) => return e };
                if let (ef::Route::Blight, Some(a)) = (&route, activation) {
                    return format!("the tablet needs \"{}\" since its last boot — use Activate above", a.label());
                }
                // Never push something the tablet cannot run.
                if let Err(e) = ef::preflight_binary(&bin, &route, p.model) {
                    return format!("not deployed: {e}");
                }
                let profile = choice.unwrap_or_else(|| ef::RefreshProfile::fastest(p.model.map(|m| m.panel()).unwrap_or(ef::Panel::Mono)));
                let env = ef::LaunchEnv { profile, model: p.model, blight_lib: d.blight_lib.clone() };
                // Sidebar entry: title, fullscreen, and a letter-tile icon.
                let opts = ef::AppLoadOptions { display_name: title, fullscreen, args: vec![] };
                let icon = std::env::temp_dir().join(format!("gitmind-icon-{name}.png"));
                let icon_ok = std::fs::write(&icon, eink_kit::icon::letter_tile_png(&name, 128)).is_ok();
                let icon_path = icon.to_string_lossy().to_string();
                let appload = Some((&opts, icon_ok.then_some(icon_path.as_str())));
                let res = ef::deliver(&mut r, &route, &name, &bin, &env, &[], appload, &mut |ev| log.push(&format!("{ev:?}")));
                let _ = std::fs::remove_file(&icon);
                match res {
                    Ok(ef::Delivered::Launched(ef::Outcome::Running { pid })) => format!("{name} running (pid {pid}, {})", profile.as_env()),
                    Ok(ef::Delivered::Launched(ef::Outcome::ExitedEarly { log_tail })) => format!("{name} exited right after launch:\n{log_tail}"),
                    Ok(ef::Delivered::Installed { path }) =>
                        format!("installed tool {path} (not an app: no AppLoad entry)"),
                    Ok(ef::Delivered::Registered { change: ef::ManifestChange::Updated, .. }) =>
                        format!("{name} updated. AppLoad keeps the old settings until the tablet's UI restarts (reload is not enough): use Restart UI above"),
                    Ok(ef::Delivered::Registered { .. }) if activation.is_some() =>
                        format!("{name} installed — activate xovi (above), then open AppLoad in the tablet's sidebar"),
                    Ok(ef::Delivered::Registered { .. }) =>
                        format!("{name} installed in AppLoad — on the tablet open AppLoad from the sidebar and tap ↻ once to list it"),
                    Err(e) => format!("deploy failed: {e}"),
                }
            });
        }
        for (label, stop) in [("Status", false), ("Stop", true)] {
            if ui.add_enabled(!busy && found.is_some() && name_ok, egui::Button::new(label)).clicked() {
                let (host, name) = (s.host.trim().to_string(), name.clone());
                let user = s.user.trim().to_string();
                s.task.run_logged(move |_| {
                    let mut r = ef::SshRemote { user: user.clone(), host };
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

    // What gitmind has put in AppLoad's sidebar on this tablet.
    if let Some(dir) = found.as_ref().and_then(|(_, d)| d.appload_dir.clone()) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("in the tablet's AppLoad").strong());
            if ui.add_enabled(!busy, egui::Button::new("List")).clicked() {
                let (host, installed, dir) = (s.host.trim().to_string(), s.installed.clone(), dir.clone());
                let user = s.user.trim().to_string();
                s.task.run_logged(move |_| {
                    let mut r = ef::SshRemote { user: user.clone(), host };
                    match ef::appload_list_cmd(&dir).and_then(|c| ef::Remote::run(&mut r, &c)) {
                        Ok(out) => {
                            let list = ef::parse_appload_list(&out);
                            let n = list.len();
                            *installed.lock().unwrap() = list;
                            format!("{n} app(s) installed by gitmind")
                        }
                        Err(e) => format!("list failed: {e}"),
                    }
                });
            }
        });
        let list = s.installed.lock().unwrap().clone();
        for (app, title) in list {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{title}  ({app})"));
                if s.remove_confirm.as_deref() == Some(app.as_str()) {
                    if ui.add_enabled(!busy, egui::Button::new("Confirm remove")).clicked() {
                        let (host, installed, dir, a) = (s.host.trim().to_string(), s.installed.clone(), dir.clone(), app.clone());
                        let user = s.user.trim().to_string();
                        s.task.run_logged(move |_| {
                            let mut r = ef::SshRemote { user: user.clone(), host };
                            match ef::appload_remove_cmd(&a, &dir).and_then(|c| ef::Remote::run(&mut r, &c)) {
                                Ok(_) => {
                                    installed.lock().unwrap().retain(|(n, _)| n != &a);
                                    format!("{a} removed; tap ↻ in AppLoad to update the list")
                                }
                                Err(e) => format!("remove failed: {e}"),
                            }
                        });
                        s.remove_confirm = None;
                    }
                    if ui.button("Cancel").clicked() {
                        s.remove_confirm = None;
                    }
                } else if ui.add_enabled(!busy, egui::Button::new("Remove")).clicked() {
                    s.remove_confirm = Some(app.clone());
                }
            });
        }
    }

    if let Some(r) = s.task.take_result() {
        s.last_result = r;
    }
    if busy {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        crate::show::task_log(ui, "tablets", &s.task.log(), 140.0);
    }
    if !s.last_result.is_empty() {
        ui.separator();
        // What went wrong, explained (stoat's lesson): findings first, the
        // raw text below them, folded once findings have said it.
        let found = ef::diagnose(&s.last_result);
        let c = uikit_egui::palette_colors(&app.prefs.palette);
        crate::show::findings(ui, &c, &found);
        if found.is_empty() {
            for line in s.last_result.lines() {
                ui.add(egui::Label::new(line).wrap());
            }
        } else {
            egui::CollapsingHeader::new("raw output").id_salt("tablets-raw").show(ui, |ui| {
                for line in s.last_result.lines() {
                    ui.add(egui::Label::new(egui::RichText::new(line).monospace().small()).wrap());
                }
            });
        }
    }
}
