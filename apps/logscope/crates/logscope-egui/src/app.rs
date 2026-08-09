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
    // the header + tab-bar glass carries the live scene light (light bleed) instead of a fixed cool tint.
    let sl = theme::scene_light();
    let c = theme::lerp_col(theme::PANEL(), sl.color, 0.14 + 0.18 * sl.intensity);
    let d = |x: u8| (x as f32 * 0.62) as u8; // darken a touch so overlaid text keeps contrast
    let fill = egui::Color32::from_rgba_unmultiplied(d(c.r()), d(c.g()), d(c.b()), 196);
    egui::Frame::default().fill(fill).inner_margin(egui::Margin::symmetric(10, 5))
}

type HostBatch = (String, Vec<ModelInfo>);

/// How long the resolved target must hold still before a load is kicked.
///
/// Long enough to cover a startup where the scan, the deployment plan and the role plan each move it;
/// short enough that a deliberate model change still feels immediate. A user picking from a dropdown
/// will not notice 400 ms; three abandoned 30 GiB loads take 45 seconds.
const AUTOLOAD_SETTLE_MS: u64 = 400;

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
    show_pipeline: bool, // Active-pipeline window (Large/Small/Voice/Image selectors) toggled by the ⚙ button
    deployment: logscope::deploy::Deployment, // multi-model/multi-device plan behind the tile picker
    autoloaded: Option<String>,               // name of the model we last auto-kicked a load for (debounce)
    /// Target that wants loading, and when it was first seen. Cleared once loaded or abandoned.
    pending_target: Option<(String, std::time::Instant)>,
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
            show_pipeline: false,
            deployment: logscope::deploy::Deployment::new(vec![logscope::deploy::Device {
                id: logscope::deploy::DeviceId(0),
                name: "this device".into(),
                kind: logscope::deploy::DeviceKind::Gpu,
                mem_bytes: logscope::model_selector::read_free_bytes().max(1),
            }]),
            autoloaded: None,
            pending_target: None,
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
        // Restore the selection BY IDENTITY, not by index. A rescan reorders the list — adding one file
        // to the models folder shifts every index after it — and the old index is still in range, just
        // pointing at a different model. The picker then shows one name while the engine loads another.
        // Hide shards that are not the entry point. Listing every shard as its own model is how the
        // wrong one gets selected, and selecting one is unrecoverable from the UI — the load simply
        // fails with a message about the file rather than about the choice.
        let items: Vec<_> = items.into_iter()
            .filter(|it| !logscope::modelorg::is_secondary_shard(it.name()))
            .collect();
        if !self.model.replace_items(items) && !self.model.items.is_empty() {
            self.local_status = format!("{} — previously selected model is gone; reset to {}",
                self.local_status, self.model.current());
        }
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
    /// The ONE resolution of "what am I talking to", with its source.
    ///
    /// This used to be a silent precedence rule: the deployment won when its plan was valid, and nothing
    /// said so. Changing the picker then did nothing at all, with no indication why — every control
    /// looked live and one of them was not. `Resolution` carries the loser too, so the UI can say which
    /// control is currently inert instead of leaving someone to conclude the app is broken.
    fn active_model(&self) -> logscope::activemodel::Resolution {
        let dep = if self.deployment.is_valid() {
            self.deployment.target().map(|t| t.model.clone())
        } else { None };
        logscope::activemodel::resolve(dep.as_deref(), Some(&self.model.current()))
    }

    /// The model generation should run.
    fn active_target_name(&self) -> Option<String> { self.active_model().name }

    /// The deployment's Draft model name, if one is configured (for speculative/tree decoding).
    fn active_draft_name(&self) -> Option<String> {
        self.deployment.draft().map(|d| d.model.clone())
    }

    /// Resolve a model name to a local `.gguf` path, if it names a scanned local model.
    fn local_path_for(&self, name: &str) -> Option<String> {
        self.local_models.iter().find(|m| m.name == name).map(|m| m.path.to_string_lossy().into_owned())
    }

    fn can_load_local(&self) -> bool {
        self.active_target_name().and_then(|n| self.local_path_for(&n)).is_some() || self.model.selected_is_local()
    }

    fn load_selected_local(&mut self) {
        // Routing: prefer the deployment Target, fall back to the dropdown selection.
        let path = self.active_target_name().and_then(|n| self.local_path_for(&n))
            .or_else(|| self.model.selected_local_path());
        if let Some(path) = path {
            self.local_status = format!("loading {path}…");
            self.model_loaded = false;
            self.infer.load(path); // worker thread owns the model
        }
    }

    /// AUTOLOAD: whenever the resolved active local target changes, kick a load automatically — no Load
    /// button. Debounced by name (only re-loads when the target actually changes) and skipped while scanning
    /// (models aren't settled yet). Ollama/remote targets need no local load, so they're left alone.
    /// Load the resolved target, but only once it has STOPPED CHANGING.
    ///
    /// This used to fire on every change, and the resolved target passes through intermediate values
    /// during startup — the scan completes and picks a default, then the deployment plan resolves, then
    /// the role plan applies. Each intermediate value triggered a full load. Observed on a real run:
    /// DeepSeek-8B, then GLM-4.7-Flash, then Qwen3-30B, three models mapped in 45 seconds for one
    /// model's worth of work, with the first two abandoned immediately.
    ///
    /// A settle delay fixes it without needing to find and serialise every writer — which is the real
    /// problem, and one this does not solve. It makes the symptom go away and buys time; the number of
    /// things that can write the target is still three.
    fn autoload_if_changed(&mut self) {
        if self.scanning { return; }
        // COLLAPSE THE SOURCES before resolving. If a deployment names a model the picker also has, move
        // the picker to it once — after that there is a single source and the override stops being a
        // standing disagreement. Done here rather than at render time so it happens whether or not the
        // pipeline panel is open.
        {
            let items: Vec<String> = self.model.items.iter().map(|i| i.name().to_string()).collect();
            // A failed load leaves the picker naming a model that is not running. The chat tab
            // observes the failure; the app owns the picker, so the decision is handed across rather
            // than written from two places — which is the split _290 had to repair.
            if let Some(revert) = self.tabs.chat.take_revert_picker() {
                self.model.revert_to(revert.as_deref());
            }

            if let Some(idx) = logscope::activemodel::write_through(&self.active_model(), &items) {
                self.model.selected = idx;
            }
        }
        let Some(name) = self.active_target_name() else { self.pending_target = None; return };
        if self.autoloaded.as_deref() == Some(name.as_str()) { return; } // already (auto)loaded this one

        // Restart the timer whenever the target moves; load only when it has held still.
        match &self.pending_target {
            Some((prev, _)) if *prev == name => {}
            _ => { self.pending_target = Some((name.clone(), std::time::Instant::now())); return; }
        }
        let Some((_, since)) = &self.pending_target else { return };
        if since.elapsed() < std::time::Duration::from_millis(AUTOLOAD_SETTLE_MS) { return; }

        let Some(path) = self.local_path_for(&name) else { self.pending_target = None; return };
        // Resolve a shard to the FIRST of its set. Metadata and the tensor index live there; handing a
        // loader shard 2 gives it a file with no header, and llama.cpp reports "null result" — accurate
        // and useless, because the file is fine and simply is not the entry point. Done here so no
        // backend has to know about shards.
        let path = logscope::modelorg::resolve_to_first_shard(&path).unwrap_or(path);
        self.pending_target = None;
        self.autoloaded = Some(name.clone());
        self.local_status = format!("auto-loading {name}…");
        self.model_loaded = false;
        self.infer.load(path);
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
    /// Mirror scanned network hosts (Ollama servers on the LAN) as Remote devices in the deployment, so the
    /// tile picker can place a model on a network device. Local stays id 0; each new host gets the next id.
    fn sync_remote_devices(&mut self) {
        use logscope::deploy::{Device, DeviceId, DeviceKind};
        for (host, _models) in &self.host_batches {
            let name = host.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_string();
            if name.is_empty() || self.deployment.devices.iter().any(|d| d.name == name) { continue; }
            let id = self.deployment.devices.iter().map(|d| d.id.0).max().unwrap_or(0) + 1;
            // remote memory is the remote's problem — set it large so it never trips the local over-budget check
            self.deployment.devices.push(Device { id: DeviceId(id), name, kind: DeviceKind::Remote, mem_bytes: u64::MAX / 2 });
        }
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
        if got { self.ollama_entries = build_model_entries(&self.host_batches, &self.recency); self.rebuild_items(); self.sync_remote_devices(); }
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
                Evt::Loaded(Err(e)) => {
                    self.model_loaded = false;
                    // A backend switch drops the loaded model and reports "…reload the model". Clear the
                    // autoload debounce so the active target reloads into the NEW backend — this is the
                    // "models autoload on llamacpp but not the custom backend" bug (after switching, the name
                    // was unchanged so autoload skipped it). Genuine load failures keep the debounce so a
                    // broken model can't spin a reload loop.
                    if e.contains("reload the model") {
                        self.autoloaded = None;
                        self.local_status = format!("backend changed — reloading… ({e})");
                    } else {
                        self.local_status = format!("load failed: {e}");
                        self.toaster.push(ctx, format!("Load failed: {e}"), theme::RED());
                    }
                }
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
        self.autoload_if_changed(); // no Load button — the active target loads itself when it changes

        // GLASS MODE: the chrome floats over the live 3D mind-palace. The header + tab bar stay SOLID (an always-
        // clickable escape hatch); only the central viewport is immersive. Esc always exits, so you can't get stuck.
        if self.glass && ctx.input(|i| i.key_pressed(egui::Key::Escape)) { self.glass = false; }
        if self.glass {
            let sl = theme::scene_light();
            let mut v = ctx.style().visuals.clone();
            // LIGHT BLEED: tint the frosted chrome toward the LIVE scene light so every panel/widget/scrollbar
            // carries the palace's illumination (warm gold at Mansion dusk, cool cyan under Atlantis water),
            // instead of a fixed cool frost. Kept dim + translucent so text stays legible over the bleed.
            let frost = |c: egui::Color32, a: u8| {
                let g = 0.20 + 0.18 * sl.intensity;                 // how strongly the scene light bleeds in
                let lc = theme::lerp_col(c, sl.color, g);
                // keep it a touch darker than the raw bleed so foreground text keeps contrast
                egui::Color32::from_rgba_unmultiplied(
                    (lc.r() as f32 * 0.9) as u8, (lc.g() as f32 * 0.9) as u8, (lc.b() as f32 * 0.9) as u8, a)
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

        // folder popup actions (handled after the header panel closure releases the &mut self borrow)
        let (mut fp_browse, mut fp_scan) = (false, false);
        // model list + size lookup for the deployment tiles (cloned so they don't hold a self borrow)
        let available: Vec<String> = self.model.items.iter().map(|i| i.name().to_string()).collect();
        let sizes: std::collections::HashMap<String, u64> = self.local_models.iter().map(|m| (m.name.clone(), m.size_bytes)).collect();
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
                    // Restore the explicit Load control dropped in the ui-branch merge: (re)load the
                    // selected local .gguf on demand. Autoload still handles deployment-target changes;
                    // this is for forcing a (re)load of the picked model without touching the deployment.
                    if self.can_load_local()
                        && theme::action_button(ui, "Load", theme::GREEN(), true).clicked()
                    {
                        self.load_selected_local();
                    }
                    // folder config lives in a popup beside the picker now, not a whole header row
                    {
                        let mut native = crate::filepicker::native_enabled();
                        let label = crate::filepicker::system_picker_label();
                        let r = crate::model::folder_popup(ui, &mut self.models_dir, &mut native, &label);
                        crate::filepicker::set_native(native);
                        fp_browse |= r.browse; fp_scan |= r.scan;
                    }
                    // multi-model / multi-device deployment picker (tree work) — a popup that STAYS open on
                    // click (CloseOnClickOutside) so the tiles/role-combos/device-combos are usable.
                    // Active pipeline: Large / Small / Voice / Image model selectors. These are
                    // ComboBoxes, so they live in a Window — NOT a click-outside popup, which a
                    // nested combo's dropdown would dismiss (the double-dropdown bug).
                    let dbtn = ui.button("⚙").on_hover_text("Active pipeline: pick Large / Small / Voice / Image models (+ advanced multi-device placement).");
                    if dbtn.clicked() { self.show_pipeline = !self.show_pipeline; }
                    if self.show_pipeline {
                        let mut open = true;
                        let sizes_ref = &sizes;
                        let est = |n: &str| sizes_ref.get(n).copied().unwrap_or(4_000_000_000);
                        egui::Window::new("Active pipeline")
                            .open(&mut open)
                            .default_width(480.0)
                            .show(ui.ctx(), |ui| {
                                ui.label(RichText::new("pick the model for each stage").color(theme::DIM()).small());
                                // WHAT YOU ARE ACTUALLY TALKING TO, resolved once and stated here. This
                                // window and the picker can disagree, and the deployment wins — which is
                                // fine, but it has to be VISIBLE or the picker reads as broken.
                                let act = self.active_model();
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new("active:").color(theme::DIM()).small());
                                    ui.label(RichText::new(act.name.as_deref().unwrap_or("—"))
                                        .color(theme::CYAN()).strong().small());
                                    ui.label(RichText::new(format!("(from {})", act.source.label()))
                                        .color(theme::FAINT()).small());
                                });
                                if let Some(note) = act.note() {
                                    ui.label(RichText::new(note).color(theme::AMBER()).small());
                                }
                                ui.add_space(4.0);
                                crate::model::pipeline_selectors(ui, &mut self.deployment, &available, &est);
                                ui.add_space(6.0);
                                ui.collapsing(RichText::new("advanced — multi-device placement").small(), |ui| {
                                    crate::model::deployment_tiles(ui, &mut self.deployment, &available, &est);
                                });
                            });
                        self.show_pipeline = open;
                    }
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
                        for v in 1..=16u8 {
                            if ui.selectable_label(theme::variant() == v, theme::variant_label(v)).clicked() {
                                theme::set_variant(v); ui.ctx().request_repaint();
                            }
                        }
                    });
                    ui.label(RichText::new("variant").color(theme::DIM()).small());
                });
            });
            // The folder chrome now lives in the 📁 popup by the picker. This row is just backend status +
            // the local-model Load button — hidden in glass mode to reclaim scene height.
            if !self.glass { ui.horizontal(|ui| {
                let avail = llm_backend::is_available();
                let (txt, col) = if self.model_loaded { ("backend: ggml loaded", theme::GREEN()) }
                    else if avail { ("backend: llamacpp ✓", theme::CYAN()) }
                    else { ("backend: stub", theme::FAINT()) };
                ui.label(RichText::new(txt).color(col).small());
                // routing: show which model actually runs (deployment Target, else dropdown) + a draft note.
                // No Load button — autoload_if_changed() loads the active target automatically.
                let act = self.active_model();
                if let Some(t) = act.name.clone() {
                    let loaded = self.model_loaded && self.autoloaded.as_deref() == Some(t.as_str());
                    let mark = if loaded { "●" } else { "○" };
                    // The top bar is where someone looks to answer "what am I talking to", so it names
                    // the SOURCE too. An amber dot means a visible control is currently inert — the
                    // picker says one thing and the deployment is doing another.
                    let inert = act.picker_is_inert();
                    let col = if inert { theme::AMBER() } else if loaded { theme::GREEN() } else { theme::DIM() };
                    let hover = act.note().unwrap_or_else(|| format!(
                        "Talking to {t}, chosen by the {}. Loads automatically when it changes.",
                        act.source.label()));
                    ui.label(RichText::new(format!("{mark} → {t}")).color(col).small())
                        .on_hover_text(hover);
                    if inert {
                        ui.label(RichText::new("⚠").color(theme::AMBER()).small())
                            .on_hover_text(act.note().unwrap_or_default());
                    }
                    if let Some(d) = self.active_draft_name() {
                        ui.label(RichText::new(format!("+draft {d}")).color(theme::FAINT()).small());
                    }
                }
                if !self.local_status.is_empty() {
                    ui.label(RichText::new(&self.local_status).color(theme::FAINT()).small());
                }
            }); }
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

        // Act on the folder popup now that the header closure released &mut self.
        if fp_browse {
            let start = std::path::Path::new(&self.models_dir).to_path_buf();
            self.dir_picker.open_dir(&start, "Choose models folder");
        }
        if fp_scan { self.scan_local(); }

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

        // LIGHT BLEED: after all chrome is drawn, spill the live scene light out of the viewport across the
        // whole UI (directional sun wash + edge glows onto the header/tab-bar + a faint overall wash), so the
        // app reads as lit by the mind-palace. Foreground layer, subtle + non-interactive. Repaint so it
        // breathes and tracks the day cycle.
        // LIGHT BLEED across the WHOLE app (both modes): the scene light spills over the UI — full strength as
        // the immersive glass backdrop, gentler as a unifying wash in the default card UI — and FLARES brighter
        // while the system is generating, so the interface visibly lights up with the palace's activity.
        if std::env::var("LOGSCOPE_BLEED").map(|v| v != "0").unwrap_or(true) {
            let t = ctx.input(|i| i.time) as f32;
            let (_n, gq) = self.tabs.chat.canopy_cues();
            let flare = 1.0 + 0.35 * (gq as f32).min(3.0);         // in-flight generation → the UI flares up
            let mut sl = theme::scene_light();
            sl.intensity = (sl.intensity * flare).min(1.4);
            let strength = if self.glass { 1.0 } else { 0.42 };    // gentler over the opaque default cards
            theme::paint_light_bleed(&ctx, ctx.viewport_rect(), sl, t, strength);
            if self.glass || gq > 0 { ctx.request_repaint_after(std::time::Duration::from_millis(50)); }
        }
    }
}
