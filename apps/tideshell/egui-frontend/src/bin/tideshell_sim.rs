//! tideshell-sim — the whole shell in a phone-sized window, over simulated
//! windows that obey the shell's Actions (activate, close, maximize, launch).
//!
//!   cargo run --release -- [--trace] [--live] [--persist] [--layout right-bar]
//!                          [--demo drawer|switcher|hold|bar|trackpad|settings|swipe]
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
}

impl World {
    fn new() -> World {
        let mut w = World { wins: vec![], active: None, next: 1, cursor: Pos2::new(200.0, 300.0), clicks: 0, keyboard: false, toast: None };
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
            .map(|w| WindowCard { id: w.id, title: w.title.clone(), app_id: w.app_id.clone(), activated: self.active == Some(w.id), maximized: w.maximized })
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
            Action::Pointer(PointerOut::Motion { dx, dy }) => {
                self.world.cursor += Vec2::new(*dx, *dy);
                Outcome::Done(String::new())
            }
            Action::Pointer(PointerOut::Button { pressed: true, .. }) => {
                self.world.clicks += 1;
                Outcome::Done(format!("sim: click #{}", self.world.clicks))
            }
            Action::Pointer(_) => Outcome::Done(String::new()),
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
}

impl App {
    fn execute(&mut self, actions: Vec<Action>, now: f64) {
        for a in actions {
            if a == Action::SavePrefs {
                let o = if self.persist { effects::save_prefs(&self.shell.prefs) } else { Outcome::Done("prefs not persisted (sim without --persist)".into()) };
                self.shell.note(o.describe());
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
            "swipe" => drag(&mut self.shell, (w / 2.0, h - 4.0), (w / 2.0, h * 0.62), 30, false),
            "hold" => {
                let o = self.shell.on_edge_touch(TouchEvent::Down { slot: 9, x: w / 2.0, y: h - 6.0, t: 0 });
                // tick() past hold_ms opens the arc; then slide onto Keyboard.
                self.shell.tick(1000);
                if let Some(m) = self.shell.hold.clone() {
                    let (x, y) = m.centers[1];
                    self.shell.on_edge_touch(TouchEvent::Move { slot: 9, x, y, t: 1100 });
                }
                o
            }
            "trackpad" => self.shell.toggle_mode(tideshell_core::hold::HoldOption::Trackpad),
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

fn clock() -> String {
    // UTC: no timezone database dependency in the simulator.
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60)
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
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        let now = ctx.input(|i| i.time);
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.view.show_trace = !self.view.show_trace;
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
            let p = ui.painter_at(area);
            p.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(28, 32, 44));
            // The active window fills the space beside the bar's exclusive zone.
            let win_r = Rect::from_min_max(area.min, area.max - Vec2::new(ShellView::reserved_right(&self.shell), 0.0));
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
            if self.world.keyboard {
                let kb = Rect::from_min_max(Pos2::new(win_r.left(), win_r.bottom() - win_r.height() * 0.33), win_r.max);
                p.rect_filled(kb, CornerRadius::ZERO, Color32::from_gray(38));
                p.text(kb.center(), Align2::CENTER_CENTER, "rusty-board (simulated)", FontId::proportional(14.0), Color32::from_gray(200));
            }
            if self.shell.modes.trackpad {
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
            let info = FrameInfo { apps: &self.apps, clock: clock(), battery: battery() };
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
    let world = World::new();
    let live = if has("--wayland") {
        match tideshell_egui::wayland::Windows::connect() {
            Ok(mut w) => {
                w.exclude_app_id = Some(APP_ID.to_string());
                shell.note("wayland: connected to toplevel list");
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
    let demo = val("--demo");
    let show_trace = has("--trace");
    eframe::run_native(
        APP_ID,
        opts,
        Box::new(move |cc| {
            let prefs = settings_kit::Prefs::default();
            uikit_egui::apply_prefs(&cc.egui_ctx, &prefs);
            let palette = uikit_egui::palette_colors_ctx(&cc.egui_ctx, &prefs.palette);
            let mut view = ShellView::default();
            view.show_trace = show_trace;
            Ok(Box::new(App { shell, view, world, system, persist, palette, apps, demo, demo_started: None, last_cards: vec![], live }))
        }),
    )
}
