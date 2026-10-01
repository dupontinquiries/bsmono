//! tideshell-sim — the whole shell in a phone-sized window, over simulated
//! windows that obey the shell's Actions (activate, close, maximize, launch).
//!
//!   cargo run --release -- [--trace] [--live] [--persist] [--layout right-bar]
//!                          [--demo drawer|switcher|hold|palette|bar|trackpad|fatcursor|carousel|settings|swipe]
//!
//! * Mouse = one finger. Edge zones are 28 px: pull from the top, swipe or
//!   hold on the bottom, swipe along the bottom, swipe in from the right
//!   (right-bar layout).
//! * `--trace` shows the engine's trace overlay (F1 toggles it).
//! * `--live` also runs the real effects: hatch-core launches, `hatch-egui
//!   --toggle`, rusty-board over busctl. Window actions stay simulated.
//! * `--persist` loads and saves `~/.config/tideshell/prefs` (off by default so
//!   poking at the simulator does not rewrite a real config).
//! * `--wayland` uses the compositor's real windows (cosmic-comp: toplevel
//!   list + management) instead of simulated ones; window actions are sent
//!   as zcosmic requests.
//! * `--ei-selftest DX DY`: once EI is Ready, move the pointer by (DX, DY)
//!   and left-click, through the same channel the trackpad uses.
//! * `--demo` drives the real recognizer with synthetic touches at startup, to
//!   reach a state for a screenshot without input.

use eframe::egui;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Vec2};
use tideshell_core::prefs::{Layout, ShellPrefs};
use tideshell_core::trackpad::PointerOut;
use tideshell_core::{Action, Shell, TouchEvent, WindowCard};
use tideshell_egui::effects::{self, Executor, Outcome, System};
use tideshell_egui::{app_color, AppTile, FrameInfo, ShellView};

const APP_ID: &str = "org.tideshell.Sim";

struct SimWin {
    id: u64,
    title: String,
    app_id: String,
    maximized: bool,
}

struct World {
    wins: Vec<SimWin>,
    active: Option<u64>,
    next: u64,
    cursor: Pos2,
    clicks: u32,
    keyboard: bool,
    toast: Option<(String, f64)>,
    /// The shell asked to hide the system cursor (fat cursor drawn instead).
    fat: bool,
}

impl World {
    fn new() -> World {
        let mut w = World { wins: vec![], active: None, next: 1, cursor: Pos2::new(200.0, 300.0), clicks: 0, keyboard: false, toast: None, fat: false };
        for (t, a) in [("Files", "com.system76.CosmicFiles"), ("Terminal", "com.system76.CosmicTerm"), ("Browser", "org.mozilla.firefox"), ("Notes", "org.gnome.TextEditor")] {
            w.open(t, a);
        }
        w
    }

    fn open(&mut self, title: &str, app_id: &str) {
        let id = self.next;
        self.next += 1;
        self.wins.push(SimWin { id, title: title.into(), app_id: app_id.into(), maximized: false });
        self.active = Some(id);
    }

    fn cards(&self) -> Vec<WindowCard> {
        self.wins
            .iter()
            .map(|w| WindowCard { id: w.id, title: w.title.clone(), app_id: w.app_id.clone(), activated: self.active == Some(w.id), maximized: w.maximized, width: None })
            .collect()
    }
}

struct SimExec<'a> {
    world: &'a mut World,
    live: Option<&'a mut System>,
    now: f64,
}

impl Executor for SimExec<'_> {
    fn run(&mut self, a: &Action) -> Outcome {
        let live = |s: &mut Option<&mut System>, a: &Action| s.as_mut().map(|sys| sys.run(a).describe());
        match a {
            Action::Activate(id) => {
                self.world.active = Some(*id);
                Outcome::Done(format!("sim: activated {id}"))
            }
            Action::Close(id) => {
                self.world.wins.retain(|w| w.id != *id);
                if self.world.active == Some(*id) {
                    self.world.active = None;
                }
                Outcome::Done(format!("sim: closed {id}"))
            }
            Action::SetMaximized(id, on) => {
                if let Some(w) = self.world.wins.iter_mut().find(|w| w.id == *id) {
                    w.maximized = *on;
                }
                Outcome::Done(format!("sim: maximized {id}={on}"))
            }
            Action::LaunchApp(id) => {
                let l = live(&mut self.live, a);
                self.world.open(id.rsplit('.').next().unwrap_or(id), id);
                Outcome::Done(format!("sim: opened {id}{}", l.map(|s| format!(" / live {s}")).unwrap_or_default()))
            }
            Action::SummonLauncher => {
                let l = live(&mut self.live, a);
                self.world.toast = Some(("hatch summoned".into(), self.now));
                Outcome::Done(format!("sim: hatch{}", l.map(|s| format!(" / live {s}")).unwrap_or_default()))
            }
            Action::ShowKeyboard(on) => {
                let l = live(&mut self.live, a);
                self.world.keyboard = *on;
                Outcome::Done(format!("sim: keyboard={on}{}", l.map(|s| format!(" / live {s}")).unwrap_or_default()))
            }
            Action::Pointer(PointerOut::Absolute { x, y }) => {
                self.world.cursor = Pos2::new(*x, *y);
                Outcome::Done(String::new())
            }
            Action::DisplayPower(on) => Outcome::Done(format!("sim: display {}", if *on { "on" } else { "off" })),
            Action::Rotate(deg) => Outcome::Done(format!("sim: rotate {deg}")),
            Action::SetVoicing(t) => Outcome::Done(format!("sim: voicing {t:+.2}")),
            Action::HideSystemCursor(on) => {
                self.world.fat = *on;
                Outcome::Done(format!("sim: system cursor hidden={on}"))
            }
            Action::Pointer(PointerOut::Motion { dx, dy }) => {
                self.world.cursor += Vec2::new(*dx, *dy);
                Outcome::Done(String::new())
            }
            Action::Pointer(PointerOut::Button { pressed: true, .. }) => {
                self.world.clicks += 1;
                Outcome::Done(format!("sim: click #{}", self.world.clicks))
            }
            Action::Pointer(_) => Outcome::Done(String::new()),
            // Auto-brightness ramps: silent (the fake display dims instead).
            Action::SetSlider(..) => Outcome::Done(String::new()),
            other => Outcome::Done(format!("sim: {other:?}")),
        }
    }
}

struct App {
    shell: Shell,
    view: ShellView,
    world: World,
    system: Option<System>,
    persist: bool,
    palette: uikit_egui::PaletteColors,
    apps: Vec<AppTile>,
    demo: Option<String>,
    demo_started: Option<f64>,
    last_cards: Vec<WindowCard>,
    live: Option<tideshell_egui::wayland::Windows>,
    ei: Option<tideshell_egui::ei::EiPointer>,
    ei_status: Option<tideshell_egui::ei::Status>,
    ei_selftest: Option<(f32, f32)>,
    ei_ready_at: Option<f64>,
    /// --lux N: a constant fake ambient reading; --clock HH:MM overrides the time.
    fake_lux: Option<f32>,
    /// --lux-noise F: the fake reading wobbles by +-F (fraction), 4 Hz.
    lux_noise: f32,
    /// --camera synthetic: a moving pattern behind the cover.
    camera: bool,
    fake_clock: Option<u32>,
    /// Actions from raw_input_hook, executed in the next ui() call.
    routed: Vec<Action>,
}

impl App {
    fn execute(&mut self, actions: Vec<Action>, now: f64) {
        for a in actions {
            if a == Action::SavePrefs {
                let o = if self.persist { effects::save_prefs(&self.shell.prefs) } else { Outcome::Done("prefs not persisted (sim without --persist)".into()) };
                self.shell.note(o.describe());
                continue;
            }
            if let (Action::Pointer(p), Some(ei)) = (&a, &self.ei) {
                ei.send(*p);
                continue;
            }
            let mut sim = SimExec { world: &mut self.world, live: self.system.as_mut(), now };
            let o = match self.live.as_mut() {
                Some(windows) => tideshell_egui::wayland::Live { windows, fallback: &mut sim }.run(&a),
                None => sim.run(&a),
            };
            if let Outcome::Done(s) = &o {
                if s.is_empty() {
                    continue;
                }
            }
            self.shell.note(o.describe());
        }
        if let Some(ei) = &self.ei {
            let st = ei.status();
            if self.ei_status.as_ref() != Some(&st) {
                self.shell.note(format!("ei: {st:?}"));
                if st == tideshell_egui::ei::Status::Ready && self.ei_ready_at.is_none() {
                    self.ei_ready_at = Some(now);
                }
                self.ei_status = Some(st);
            }
            // Self-test waits 3 s past Ready: EI connects on a thread at
            // process start, and a click sent before our own window maps is
            // undone when cosmic-comp activates the newly mapped window (seen
            // live; it looked like EI clicks never change focus).
            if self.ei_ready_at.is_some_and(|t| now - t >= 3.0) {
                {
                    if let Some((dx, dy)) = self.ei_selftest.take() {
                        use tideshell_core::trackpad::{Button, PointerOut as P};
                        ei.send(P::Motion { dx, dy });
                        ei.send(P::Button { button: Button::Left, pressed: true });
                        ei.send(P::Button { button: Button::Left, pressed: false });
                        self.shell.note(format!("ei selftest: moved ({dx}, {dy}) and clicked"));
                    }
                }
            }
        }
        if let Some(sys) = self.system.as_mut() {
            for (o, undo) in sys.reap() {
                self.shell.note(o.describe());
                if undo == Some(effects::Undo::KeyboardShown) {
                    self.shell.set_keyboard_visible(false);
                }
            }
        }
        let cards = match self.live.as_mut() {
            Some(w) => match w.poll() {
                Some(c) => c,
                None => return,
            },
            None => self.world.cards(),
        };
        if cards != self.last_cards {
            self.last_cards = cards.clone();
            let more = self.shell.set_windows(cards);
            if !more.is_empty() {
                self.execute(more, now);
            }
        }
    }

    /// Per-frame part of a demo: the fatcursor finger circles on the pad for
    /// 5 s of real time, then lifts.
    fn demo_frame(&mut self, now: f64) {
        let (Some(which), Some(t0)) = (self.demo.as_deref(), self.demo_started) else { return };
        if which != "fatcursor" {
            return;
        }
        let e = now - t0;
        let t = (now * 1000.0) as u64;
        let pad = ShellView::pad_rect(egui::Vec2::new(self.shell.screen.0, self.shell.screen.1), 0.0);
        let out = if e < 5.0 {
            let a = e as f32 * 2.2;
            let (x, y) = (pad.center().x + 70.0 * a.cos(), pad.center().y + 50.0 * a.sin());
            self.shell.on_trackpad_touch(TouchEvent::Move { slot: 7, x, y, t })
        } else if self.demo.take().is_some() {
            self.shell.on_trackpad_touch(TouchEvent::Up { slot: 7, t })
        } else {
            vec![]
        };
        self.execute(out, now);
    }

    /// Synthetic touches through the real recognizer. Timestamps start at 0;
    /// the view's own ticks continue from egui's clock afterwards.
    fn run_demo(&mut self, which: &str) {
        let (w, h) = self.shell.screen;
        let drag = |sh: &mut Shell, from: (f32, f32), to: (f32, f32), steps: u32, lift: bool| {
            let mut out = sh.on_edge_touch(TouchEvent::Down { slot: 9, x: from.0, y: from.1, t: 0 });
            for i in 1..=steps {
                let f = i as f32 / steps as f32;
                out.extend(sh.on_edge_touch(TouchEvent::Move { slot: 9, x: from.0 + (to.0 - from.0) * f, y: from.1 + (to.1 - from.1) * f, t: 16 * i as u64 }));
            }
            if lift {
                out.extend(sh.on_edge_touch(TouchEvent::Up { slot: 9, t: 16 * steps as u64 + 16 }));
            }
            out
        };
        let out = match which {
            "drawer" => drag(&mut self.shell, (w / 2.0, 3.0), (w / 2.0, h * 0.8), 30, true),
            "switcher" => drag(&mut self.shell, (w / 2.0, h - 4.0), (w / 2.0, h * 0.72), 30, true),
            "swipe" => drag(&mut self.shell, (w / 2.0, h - 4.0), (w / 2.0, h * 0.90), 30, false),
            "hold" => {
                let o = self.shell.on_edge_touch(TouchEvent::Down { slot: 9, x: w / 2.0, y: h - 6.0, t: 0 });
                // tick() past hold_ms opens the palette; slide onto Keyboard.
                self.shell.tick(1000);
                let g = tideshell_core::palette::Geom::new((w, h));
                let (x, y, mw, mh) = g.mode_rect(1);
                self.shell.on_edge_touch(TouchEvent::Move { slot: 9, x: x + mw / 2.0, y: y + mh / 2.0, t: 1100 });
                o
            }
            "palette" => {
                self.shell.open_palette();
                vec![]
            }
            "trackpad" => self.shell.toggle_mode(tideshell_core::hold::HoldOption::Trackpad),
            "sleep" => self.shell.power_key(),
            "lock" => {
                // Mid-drag: the cover follows the finger 30% of the way.
                self.shell.lock_now();
                self.shell.lock.drag_reveal(0.3, 0);
                vec![]
            }
            "carousel" => self.shell.toggle_mode(tideshell_core::hold::HoldOption::Carousel),
            "fatcursor" => {
                let mut p = self.shell.prefs.clone();
                p.fat_cursor = true;
                let mut o = self.shell.set_prefs(p);
                o.extend(self.shell.toggle_mode(tideshell_core::hold::HoldOption::Trackpad));
                // Drag on the pad: the disc eases up-right from the centre.
                let pad = ShellView::pad_rect(egui::Vec2::new(w, h), 0.0);
                let (x0, y0) = (pad.center().x, pad.center().y);
                // The finger then circles on real time (see demo_frame), so
                // the disc is seen easing rather than already idle-faded.
                o.extend(self.shell.on_trackpad_touch(TouchEvent::Down { slot: 7, x: x0, y: y0, t: 0 }));
                o
            }
            "bar" | "settings" => {
                let mut p = self.shell.prefs.clone();
                p.layout = Layout::RightBar;
                let mut o = self.shell.set_prefs(p);
                if which == "settings" {
                    self.view.open_settings(&mut self.shell);
                } else {
                    o.extend(drag(&mut self.shell, (w - 2.0, h / 2.0), (w - 90.0, h / 2.0), 8, true));
                }
                o
            }
            other => {
                self.shell.note(format!("unknown demo '{other}'"));
                vec![]
            }
        };
        self.execute(out, 0.0);
    }
}

/// Local time, with --clock HH:MM overriding hour and minute.
fn sim_time(fake: Option<u32>) -> tideshell_core::clock::LocalTime {
    let mut t = tideshell_egui::sensors::local_time();
    if let Some(m) = fake {
        t.hour = m / 60;
        t.minute = m % 60;
    }
    t
}

fn battery() -> Option<f32> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for e in dir.flatten() {
        if let Ok(t) = std::fs::read_to_string(e.path().join("capacity")) {
            if let Ok(v) = t.trim().parse::<f32>() {
                return Some(v / 100.0);
            }
        }
    }
    None
}

impl eframe::App for App {
    /// Route edge/pad-owned input before egui sees it (see ShellView::route_raw).
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        let origin = raw.screen_rect.map(|r| r.min).unwrap_or(Pos2::ZERO);
        let now = (raw.time.unwrap_or(0.0) * 1000.0) as u64;
        let out = self.view.route_raw(&mut self.shell, &mut raw.events, origin, now);
        self.routed.extend(out);
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        let now = ctx.input(|i| i.time);
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.view.show_trace = !self.view.show_trace;
        }
        // P = the power key.
        if ctx.input(|i| i.key_pressed(egui::Key::P)) {
            let a = self.shell.power_key();
            self.execute(a, now);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(root, |ui| {
            let area = ui.max_rect();
            if let Some(which) = self.demo.clone() {
                // Wait one frame so the shell knows the real screen size.
                if self.demo_started.is_none() {
                    self.shell.set_screen((area.width(), area.height()));
                    self.demo_started = Some(now);
                    self.execute(vec![], now); // windows into the shell first
                    self.run_demo(&which);
                }
            }
            let routed = std::mem::take(&mut self.routed);
            self.execute(routed, now);
            self.demo_frame(now);
            if self.camera && self.shell.camera_wanted() {
                let f = tideshell_egui::camera::synthetic_frame(120, 160, now as f32);
                self.view.set_camera_frame(ctx, f.w, f.h, &f.rgba);
            }
            self.shell.set_clock(self.fake_clock.unwrap_or_else(tideshell_egui::sensors::local_minute));
            if let Some(l) = self.fake_lux {
                let wobble = if ((now * 4.0) as u64) % 2 == 0 { 1.0 + self.lux_noise } else { 1.0 - self.lux_noise };
                self.shell.on_lux(l * wobble, (now * 1000.0) as u64);
            }
            let p = ui.painter_at(area);
            p.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(28, 32, 44));
            // The active window fills the space beside the bar's exclusive zone.
            // The active window sits beside the bar's zone and, in carousel
            // mode, in the centre 80% between the two peek zones.
            let pk = self.shell.peek_reserve();
            let win_r = Rect::from_min_max(area.min + Vec2::new(pk, 0.0), area.max - Vec2::new(ShellView::reserved_right(&self.shell) + pk, 0.0));
            if let Some(live) = &self.live {
                let caps = live.capabilities();
                let msg = format!("live: {} windows, manager caps {}", self.last_cards.len(), if caps.received { "received" } else { "pending" });
                p.text(win_r.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), Color32::from_gray(200));
            } else {
            match self.world.active.and_then(|id| self.world.wins.iter().find(|w| w.id == id)) {
                Some(w) => {
                    p.rect_filled(win_r, CornerRadius::ZERO, app_color(&w.app_id).gamma_multiply(0.55));
                    p.text(win_r.center_top() + Vec2::new(0.0, 36.0), Align2::CENTER_CENTER, &w.title, FontId::proportional(20.0), Color32::WHITE);
                    let sub = format!("{}{}", w.app_id, if w.maximized { "  (maximized)" } else { "" });
                    p.text(win_r.center_top() + Vec2::new(0.0, 60.0), Align2::CENTER_CENTER, sub, FontId::proportional(12.0), Color32::from_gray(210));
                }
                None => {
                    p.text(win_r.center(), Align2::CENTER_CENTER, "desktop", FontId::proportional(16.0), Color32::from_gray(140));
                }
            }
            }
            // The fake display dims with the brightness level (auto or slider).
            let dim = ((1.0 - self.shell.drawer.brightness) * 0.85 * 255.0) as u8;
            p.rect_filled(win_r, CornerRadius::ZERO, Color32::from_black_alpha(dim));
            if self.world.keyboard {
                let kb = Rect::from_min_max(Pos2::new(win_r.left(), win_r.bottom() - win_r.height() * 0.33), win_r.max);
                p.rect_filled(kb, CornerRadius::ZERO, Color32::from_gray(38));
                p.text(kb.center(), Align2::CENTER_CENTER, "rusty-board (simulated)", FontId::proportional(14.0), Color32::from_gray(200));
            }
            if self.shell.modes.trackpad && !self.world.fat {
                let c = area.min + self.world.cursor.to_vec2();
                p.circle_filled(c, 6.0, Color32::WHITE);
                p.text(c + Vec2::new(10.0, -10.0), Align2::LEFT_CENTER, format!("clicks {}", self.world.clicks), FontId::proportional(11.0), Color32::WHITE);
            }
            if let Some((t, at)) = &self.world.toast {
                if now - at < 1.6 {
                    let r = Rect::from_center_size(area.center(), Vec2::new(200.0, 44.0));
                    p.rect_filled(r, CornerRadius::same(22), Color32::from_black_alpha(200));
                    p.text(r.center(), Align2::CENTER_CENTER, t, FontId::proportional(14.0), Color32::WHITE);
                    ctx.request_repaint();
                }
            }
            let info = FrameInfo { apps: &self.apps, time: sim_time(self.fake_clock), battery: battery(), charging: false, trackpad_note: None, t_secs: now as f32 };
            let actions = self.view.ui(ui, &mut self.shell, &self.palette, &info);
            self.execute(actions, now);
        });
        // The recognizer's hold timer needs ticks without input.
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let val = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let persist = has("--persist");
    let (mut prefs, errs) = if persist { effects::load_prefs() } else { (ShellPrefs::default(), vec![]) };
    // --pref key=value (repeatable): any shell.* setting, e.g. shell.clock.style=analog.
    for (i, a) in args.iter().enumerate() {
        if a == "--pref" {
            if let Some((k, v)) = args.get(i + 1).and_then(|kv| kv.split_once('=')) {
                if let Err(e) = prefs.apply(k, v) {
                    eprintln!("{e}");
                }
            }
        }
    }
    if let Some(l) = val("--layout") {
        if let Err(e) = prefs.apply("shell.layout", &l) {
            eprintln!("{e}");
        }
    }
    let mut shell = Shell::new(prefs, (408.0, 900.0));
    for e in errs {
        shell.note(format!("prefs: {e}"));
    }
    let system = has("--live").then(System::new);
    let launcher = hatch_core::Launcher::new(hatch_core::Config::load());
    let mut apps = effects::hatch_tiles(&launcher, 8);
    if apps.is_empty() {
        shell.note("hatch found no desktop entries; using sample tiles");
        apps = [("org.gnome.Console", "Console"), ("org.mozilla.firefox", "Firefox"), ("com.system76.CosmicFiles", "Files"), ("org.gnome.Maps", "Maps")]
            .iter()
            .map(|(id, n)| AppTile { id: (*id).into(), name: (*n).into() })
            .collect();
    }
    shell.set_apps(apps.iter().map(|a| (a.id.clone(), a.name.clone())).collect());
    let world = World::new();
    let live = if has("--wayland") {
        match tideshell_egui::wayland::Windows::connect() {
            Ok(mut w) => {
                w.exclude_app_id = Some(APP_ID.to_string());
                shell.note("wayland: connected to toplevel list");
                if let Some(o) = effects::cosmic_config_home().and_then(|h| effects::recover_system_cursor(&h)) {
                    shell.note(o.describe());
                }
                Some(w)
            }
            Err(e) => {
                shell.note(format!("wayland: {e:?}; using simulated windows"));
                None
            }
        }
    } else {
        None
    };
    let opts = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "tideshell sim", [408.0, 900.0]),
        ..Default::default()
    };
    // Real sessions send trackpad output to the compositor over EI.
    let ei = live.is_some().then(tideshell_egui::ei::EiPointer::spawn);
    let ei_selftest = args.iter().position(|a| a == "--ei-selftest").and_then(|i| {
        Some((args.get(i + 1)?.parse().ok()?, args.get(i + 2)?.parse().ok()?))
    });
    let fake_lux: Option<f32> = val("--lux").and_then(|v| v.parse().ok());
    let lux_noise: f32 = val("--lux-noise").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let camera = val("--camera").as_deref() == Some("synthetic");
    // --date MM-DD sets the season; --place rrggbb[,rrggbb] a fake known place.
    if let Some((m, d)) = val("--date").and_then(|v| v.split_once('-').map(|(a, b)| (a.parse::<u32>().unwrap_or(6), b.parse::<u32>().unwrap_or(21)))) {
        shell.set_date(tideshell_core::season::day_of_year(2026, m, d));
    }
    if let Some(cols) = val("--place") {
        shell.on_place(tideshell_core::ambience::PlaceKey::Landmark("sim".into()), "Sim place");
        let parsed: Vec<[u8; 3]> = cols.split(',').filter_map(|c| u32::from_str_radix(c, 16).ok()).map(|x| [(x >> 16) as u8, (x >> 8) as u8, x as u8]).collect();
        shell.on_photo_colors(&parsed);
    }
    if camera {
        shell.world.drop_anchor("door");
        shell.on_heading(0.0);
    }
    let fake_clock: Option<u32> = val("--clock").and_then(|v| tideshell_core::ambient::NightWindow::parse_span(&format!("{v}-00:00")).map(|(s, _)| s));
    if has("--locked") {
        shell.lock_now();
    }
    // The fake display starts at half brightness, so auto has a level to ramp from.
    shell.set_current_brightness(0.5);
    let demo = val("--demo");
    let show_trace = has("--trace");
    let wayland_mode = live.is_some();
    eframe::run_native(
        APP_ID,
        opts,
        Box::new(move |cc| {
            let mut prefs = settings_kit::Prefs::default();
            prefs.palette = shell.prefs.theme.clone();
            uikit_egui::apply_prefs(&cc.egui_ctx, &prefs);
            let palette = uikit_egui::palette_colors_ctx(&cc.egui_ctx, &prefs.palette);
            let mut view = ShellView::default();
            view.show_trace = show_trace;
            // The fake world's cursor is not the real pointer, so the mouse may
            // drive the pad there. With --wayland the pad feeds EI: no.
            view.pointer_pad = !wayland_mode;
            Ok(Box::new(App { shell, view, world, system, persist, palette, apps, demo, demo_started: None, last_cards: vec![], live, ei, ei_status: None, ei_selftest, ei_ready_at: None, routed: vec![], fake_lux, fake_clock, lux_noise, camera }))
        }),
    )
}
