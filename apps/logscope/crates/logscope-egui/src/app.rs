//! app.rs — shell. Unified model picker (Ollama via build_model_entries + local
//! GGUF via list_local_gguf), a threaded inference worker (Chat streams off the
//! UI thread), and the restyled analytics theme.
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use egui::RichText;
use logscope::llm_backend;
use logscope::model_selector::{ModelInfo, ModelTier};
use logscope::providers::{build_model_entries, ModelEntry};

use crate::analysis;
use crate::backend::{Evt, Inference};
use crate::model::{model_combo, ModelState, PickerItem};
use crate::tabs::{Tab, Tabs};
use crate::theme;

/// A frosted translucent overlay frame for the glass-mode header + tab bar — they now read as in-view overlays
/// floating over the fullscreen scene (the scene shows through), rather than solid bars. Still readable; Esc and
/// the in-app Glass toggle remain the escape hatches, and the scene lives in the central panel (it can't cover
/// these), so translucency no longer risks trapping the view.
fn glass_chrome_frame() -> egui::Frame {
    let c = theme::PANEL();
    let f = |x: u8, t: u8| ((x as u16 * 7 + t as u16 * 3) / 10) as u8;
    let fill = egui::Color32::from_rgba_unmultiplied(f(c.r(), 18), f(c.g(), 32), f(c.b(), 46), 196);
    egui::Frame::default().fill(fill).inner_margin(egui::Margin::symmetric(10, 5))
}

type HostBatch = (String, Vec<ModelInfo>);

pub struct LogscopeApp {
    tab: Tab,
    last_tab: Tab,
    tab_changed_at: Instant,
    model: ModelState,
    tabs: Tabs,
    doc: analysis::Doc,
    scan_rx: Option<Receiver<HostBatch>>,
    host_batches: Vec<HostBatch>,
    ollama_entries: Vec<ModelEntry>,
    local_models: Vec<crate::model::LocalModel>,
    recency: Vec<String>,
    scanning: bool,
    models_dir: String,
    dir_picker: crate::filepicker::FilePicker,
    local_status: String,
    infer: Inference,
    model_loaded: bool,
    toaster: crate::theme::Toaster,
    wizard: crate::wizard::Wizard,
    glass: bool, // LOGSCOPE_GLASS: render the mind-palace behind a translucent UI (in-progress immersive shell)
}

impl Default for LogscopeApp {
    fn default() -> Self {
        let dir = llm_backend::default_models_dir().to_string_lossy().into_owned();
        let mut app = Self {
            tab: Tab::Sources,
            last_tab: Tab::Sources,
            tab_changed_at: Instant::now(),
            model: ModelState::default(),
            tabs: Tabs::default(),
            doc: analysis::Doc::default(),
            scan_rx: None,
            host_batches: Vec::new(),
            ollama_entries: Vec::new(),
            local_models: Vec::new(),
            recency: Vec::new(),
            scanning: false,
            models_dir: dir,
            dir_picker: Default::default(),
            local_status: String::new(),
            infer: Inference::spawn(),
            model_loaded: false,
            toaster: crate::theme::Toaster::default(),
            wizard: crate::wizard::Wizard::default(),
            glass: std::env::var("LOGSCOPE_GLASS").map(|v| v != "0").unwrap_or(false),
        };
        app.rescan();
        app.boot_env();
        if app.glass {
            app.tab = Tab::Chat; app.last_tab = Tab::Chat; // land on the mind-palace
            // ensure the full-window backdrop has a map to show even when launched without a seed/convo
            if std::env::var("LOGSCOPE_SEED").is_err() && std::env::var("LOGSCOPE_SPATIAL").is_err() && app.tabs.chat.is_empty() {
                app.tabs.chat.boot_demo(true); // glass opens straight into the spatial mind-palace
            }
        }
        app
    }
}

impl LogscopeApp {
    /// Env-gated launch hook for headless self-reflection. Inert unless `LOGSCOPE_BOOT` is set.
    /// `LOGSCOPE_TAB=chat`, `LOGSCOPE_SPATIAL=1`, `LOGSCOPE_SEED=1` (defaults to SPATIAL).
    pub fn boot_env(&mut self) {
        if std::env::var("LOGSCOPE_BOOT").is_err() { return; }
        self.wizard.open = false;
        if let Ok(t) = std::env::var("LOGSCOPE_TAB") {
            self.tab = match t.to_lowercase().as_str() {
                "chat" => Tab::Chat, "sources" => Tab::Sources, "peek" => Tab::Peek,
                "tokens" => Tab::Tokens, "blocks" => Tab::Blocks, "rotate" => Tab::Rotate,
                "cargo" => Tab::Cargo, "code" => Tab::Code, "modify" => Tab::Modify,
                "bloc" => Tab::Bloc, _ => self.tab,
            };
            self.last_tab = self.tab;
        }
        let flag = |k: &str| std::env::var(k).map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
        let spatial = flag("LOGSCOPE_SPATIAL");
        if spatial || flag("LOGSCOPE_SEED") {
            self.tabs.chat.boot_demo(spatial);
        }
    }
}

fn tier_for(p: u64) -> ModelTier {
    if p < 3000 { ModelTier::Small } else if p <= 13000 { ModelTier::Medium } else { ModelTier::Large }
}
fn mi(name: &str, params_m: u64, vram: u64, host: &str) -> ModelInfo {
    ModelInfo { name: name.into(), tier: tier_for(params_m), size_bytes: params_m * 600_000,
        size_vram: vram, params_m, host: host.into(), autohost: false, quant: "Q4_K_M".into() }
}

impl LogscopeApp {
    fn rebuild_items(&mut self) {
        let mut items: Vec<PickerItem> =
            self.ollama_entries.iter().cloned().map(PickerItem::Ollama).collect();
        items.extend(self.local_models.iter().cloned().map(PickerItem::Local));
        let n = items.len();
        self.model.items = items;
        if self.model.selected >= n { self.model.selected = 0; }
    }
    fn scan_local(&mut self) {
        let entered = Path::new(&self.models_dir).to_path_buf(); // owned: no borrow of self held
        let is_file = entered.is_file();
        let dir = if is_file { entered.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| entered.clone()) } else { entered.clone() };
        self.local_models = llm_backend::list_local_gguf(&dir);
        self.local_status = format!("{} .gguf in {}", self.local_models.len(), dir.display());
        self.rebuild_items();
        if is_file {
            if let Some(pos) = self.model.items.iter()
                .position(|it| matches!(it, PickerItem::Local(m) if m.path == entered)) {
                self.model.selected = pos;
            }
        }
    }
    fn load_selected_local(&mut self) {
        if let Some(path) = self.model.selected_local_path() {
            self.local_status = format!("loading {path}…");
            self.model_loaded = false;
            self.infer.load(path); // worker thread owns the model
        }
    }
    fn rescan(&mut self) {
        self.host_batches.clear();
        self.ollama_entries.clear();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        self.scanning = true;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            let _ = tx.send(("http://localhost:11434".into(),
                vec![mi("qwen2.5:7b", 7000, 512_000_000, "localhost:11434"),
                     mi("llama3.1:8b", 8000, 0, "localhost:11434")]));
            std::thread::sleep(Duration::from_millis(400));
            let _ = tx.send(("http://192.168.1.42:11434".into(),
                vec![mi("mixtral:8x7b", 47000, 0, "192.168.1.42:11434"),
                     mi("qwen2.5:7b", 7000, 0, "192.168.1.42:11434")]));
        });
        self.scan_local();
    }
    fn drain_scan(&mut self, ctx: &egui::Context) {
        let mut got = false;
        let mut disconnected = false;
        if let Some(rx) = &self.scan_rx {
            loop {
                match rx.try_recv() {
                    Ok(b) => { self.host_batches.push(b); got = true; }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => { disconnected = true; break; }
                }
            }
        }
        if got { self.ollama_entries = build_model_entries(&self.host_batches, &self.recency); self.rebuild_items(); }
        if disconnected { self.scan_rx = None; self.scanning = false; self.toaster.push(ctx, format!("Scan complete — {} hosts", self.host_batches.len()), theme::CYAN()); }
        if self.scanning { ctx.request_repaint_after(Duration::from_millis(100)); }
    }
    fn drain_infer(&mut self, ctx: &egui::Context) {
        let mut evts = Vec::new();
        while let Ok(e) = self.infer.rx.try_recv() { evts.push(e); }
        let mut repaint = false;
        for e in evts {
            match e {
                Evt::Loaded(Ok(info)) => { self.model_loaded = true; self.local_status = format!("loaded {}", info.path); self.toaster.push(ctx, "Model loaded", theme::GREEN()); }
                Evt::Loaded(Err(e)) => { self.model_loaded = false; self.local_status = format!("load failed: {e}"); self.toaster.push(ctx, format!("Load failed: {e}"), theme::RED()); }
                other => { self.tabs.apply_chat_evt(other); repaint = true; }
            }
        }
        if repaint { ctx.request_repaint(); }
    }
}

impl eframe::App for LogscopeApp {
    // Force an OPAQUE window clear (alpha = 1.0). Without this, KWin/Wayland composites the desktop — and any
    // app windows behind logscope — through any pixels the scene/UI hasn't painted fully opaque (the renderer
    // clears its offscreen targets to alpha 0). Forcing the window's base clear opaque stops that ghosting.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        let c = visuals.window_fill();
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_scan(&ctx);
        self.drain_infer(&ctx);

        // GLASS MODE: the chrome floats over the live 3D mind-palace. The header + tab bar stay SOLID (an always-
        // clickable escape hatch); only the central viewport is immersive. Esc always exits, so you can't get stuck.
        if self.glass && ctx.input(|i| i.key_pressed(egui::Key::Escape)) { self.glass = false; }
        if self.glass {
            let mut v = ctx.style().visuals.clone();
            let frost = |c: egui::Color32, a: u8| {
                let f = |x: u8, t: u8| ((x as u16 * 7 + t as u16 * 3) / 10) as u8; // pull 30% toward a cool tint
                egui::Color32::from_rgba_unmultiplied(f(c.r(), 18), f(c.g(), 32), f(c.b(), 46), a)
            };
            v.panel_fill = frost(v.panel_fill, 236);
            v.window_fill = frost(v.window_fill, 240);
            v.extreme_bg_color = frost(v.extreme_bg_color, 224);
            v.faint_bg_color = frost(v.faint_bg_color, 196);
            v.widgets.noninteractive.bg_fill = frost(v.widgets.noninteractive.bg_fill, 222);
            v.widgets.inactive.bg_fill = frost(v.widgets.inactive.bg_fill, 232);
            ctx.set_visuals(v);
        }
        self.tabs.chat.set_glass(self.glass);

        let header_panel = egui::Panel::top("header");
        let header_panel = if self.glass { header_panel.frame(glass_chrome_frame()) } else { header_panel };
        header_panel.show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Logscope").color(theme::AMBER()).strong().size(15.0));
                ui.label(RichText::new("egui port").color(theme::FAINT()).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("?").on_hover_text("First-run guide").clicked() { self.wizard.reopen(); }
                    if ui.button("Rescan").clicked() { self.rescan(); }
                    if self.scanning { theme::spinner(ui, 16.0, theme::CYAN()); }
                    ui.add_space(10.0);
                    // GLASS: float the whole UI over the live mind-palace as frosted glass
                    theme::switch(ui, &mut self.glass, theme::CYAN());
                    ui.label(RichText::new("Glass").color(if self.glass { theme::CYAN() } else { theme::DIM() }).small())
                        .on_hover_text("Render the 3D mind-palace as a full-window backdrop and float the UI over it as frosted glass.");
                    ui.add_space(10.0);
                    let mr = ui.scope(|ui| { model_combo(ui, &mut self.model, "hdr_model", self.scanning); }).response;
                    theme::set_model_light(Some(mr.rect.center())); // the model's "home" — a warm hearth glow
                    ui.label(RichText::new("model").color(theme::DIM()).small());
                    ui.add_space(10.0);
                    // theme picker — live-switches the palette (Folding@home + the research-journal themes)
                    let cur = theme::active();
                    crate::ui::combo(ui, "hdr_theme", cur.name, 130.0, |ui| {
                        for p in theme::PALETTES {
                            if ui.selectable_label(cur.name == p.name, p.name).clicked() {
                                theme::install_palette(ui.ctx(), *p);
                            }
                        }
                    });
                    ui.label(RichText::new("theme").color(theme::DIM()).small());
                    ui.add_space(10.0);
                    // variant picker — flip between 10 baked candidate looks live (this round: surface detail)
                    crate::ui::combo(ui, "hdr_variant", theme::variant_label(theme::variant()), 140.0, |ui| {
                        for v in 1..=10u8 {
                            if ui.selectable_label(theme::variant() == v, theme::variant_label(v)).clicked() {
                                theme::set_variant(v); ui.ctx().request_repaint();
                            }
                        }
                    });
                    ui.label(RichText::new("variant").color(theme::DIM()).small());
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("models folder:").color(theme::DIM()).small());
                ui.add(egui::TextEdit::singleline(&mut self.models_dir)
                    .font(egui::TextStyle::Monospace).desired_width(300.0)
                    .hint_text("dir to walk for .gguf (recurses, incl. HF cache)"));
                if ui.button("📂").on_hover_text("Browse for a models folder").clicked() {
                    let start = std::path::Path::new(&self.models_dir).to_path_buf();
                    self.dir_picker.open_dir(&start, "Choose models folder");
                }
                // System file dialog (KDE/Dolphin, COSMIC, GNOME via portal/kdialog/zenity) vs the
                // built-in picker (flip off for headless/SSH sessions).
                let mut native = crate::filepicker::native_enabled();
                let label = crate::filepicker::system_picker_label();
                if ui.checkbox(&mut native, RichText::new("system file picker").small())
                    .on_hover_text(format!("Use the desktop's own file dialog — detected: {label}.\nUncheck for the built-in picker on headless sessions."))
                    .changed() { crate::filepicker::set_native(native); }
                if ui.button("Scan").clicked() { self.scan_local(); }
                let avail = llm_backend::is_available();
                let (txt, col) = if self.model_loaded { ("backend: ggml loaded", theme::GREEN()) }
                    else if avail { ("backend: llamacpp ✓", theme::CYAN()) }
                    else { ("backend: stub", theme::FAINT()) };
                ui.label(RichText::new(txt).color(col).small());
                if self.model.selected_is_local() && theme::action_button(ui, "Load", theme::GREEN(), true).clicked() { self.load_selected_local(); }
                if !self.local_status.is_empty() {
                    ui.label(RichText::new(&self.local_status).color(theme::FAINT()).small());
                }
            });
            ui.add_space(3.0);
        });

        let switcher_panel = egui::Panel::bottom("switcher");
        let switcher_panel = if self.glass { switcher_panel.frame(glass_chrome_frame()) } else { switcher_panel };
        switcher_panel.show_inside(ui, |ui| {
            ui.add_space(3.0);
            let now = ui.input(|i| i.time) as f32;
            ui.horizontal_wrapped(|ui| {
                for t in Tab::ALL {
                    let resp = ui.selectable_value(&mut self.tab, t, t.label());
                    // carved-stone slab + (active) glowing inscription with a light sweep
                    theme::carved_tab(ui.painter(), resp.rect, now, self.tab == t, theme::AMBER());
                    let a = ui.ctx().animate_bool(resp.id, self.tab == t);
                    if a > 0.0 {
                        let rc = resp.rect;
                        let w = rc.width() * a;
                        let x0 = rc.center().x - w / 2.0;
                        let y = rc.bottom() - 1.0;
                        ui.painter().line_segment(
                            [egui::pos2(x0, y), egui::pos2(x0 + w, y)],
                            egui::Stroke::new(2.0, theme::AMBER()));
                    }
                }
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(40)); // inscription sweep animates
            ui.add_space(3.0);
        });

        // In glass mode the central panel is transparent so the full-window backdrop shows through it.
        let central = if self.glass {
            egui::CentralPanel::default().frame(egui::Frame::NONE)
        } else {
            egui::CentralPanel::default()
        };
        central.show_inside(ui, |ui| {
            // dusty-library ambience behind the content: a baked surface + drifting fog + live sun rays
            // that track the cursor & model. Opaque cards sit on top. Repaint gently while lit so the fog
            // and motes animate (only the light mesh + a couple image draws redraw; the bakes are cached).
            if theme::has_ambience() && !self.glass {
                let t = ui.input(|i| i.time);
                let (gn, gq) = self.tabs.chat.canopy_cues();
                theme::set_canopy(gn, gq); // living-tree: canopy grows with the graph, leaves rustle with generation
                theme::set_cursor_light(ui.input(|i| i.pointer.hover_pos()));
                theme::paint_ambience(ui.ctx(), ui.painter(), ui.max_rect(), t);
                theme::paint_light(ui.painter(), ui.max_rect(), t);
                let p = theme::active();
                if p.lit || p.canopy { ui.ctx().request_repaint_after(std::time::Duration::from_millis(40)); }
            }
            if self.tab != self.last_tab { self.last_tab = self.tab; self.tab_changed_at = Instant::now(); }
            let fade = theme::appear(self.tab_changed_at, 0.16);
            if fade < 1.0 { ui.ctx().request_repaint(); }
            ui.add_space(8.0);
            ui.heading(self.tab.label());
            ui.add_space(8.0);
            let tabs = &mut self.tabs;
            let model = &mut self.model;
            let doc = &mut self.doc;
            let infer = &self.infer;
            let (tab, scanning, loaded) = (self.tab, self.scanning, self.model_loaded);
            let mut routed: Option<String> = None;
            theme::fade(ui, fade, |ui| {
                routed = tabs.ui(tab, ui, model, doc, scanning, infer, loaded);
            });
            if let Some(prompt) = routed {
                tabs.route_to_chat(prompt);
                self.tab = Tab::Chat;
            }
        });

        if let Some(crate::filepicker::Outcome::Picked(dir)) = self.dir_picker.ui(&ctx) {
            self.models_dir = dir.display().to_string();
            self.scan_local();
        }
        self.wizard.ui(&ctx);
        self.toaster.ui(&ctx);
    }
}
