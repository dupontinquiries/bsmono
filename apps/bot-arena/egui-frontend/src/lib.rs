//! bot-arena-egui — the arena dashboard, ported to egui with swipe navigation.
//!
//! The GTK4/libadwaita UI becomes an egui dashboard whose views are
//! **swipe-paged** using the shared gesture layer (`uikit_egui::GestureLayer`
//! over `uikit-gesture`), laid out by the adaptive layer (a phone swipes
//! between full-screen views; a desktop also shows a tab row), and
//! voice/a11y-navigable.
//!
//! The views are the GTK app's pages plus the studio:
//!
//! * **Setup** — the run configuration (GTK page 1), mapped onto
//!   `bot_arena::engine::Config` by `arena::SetupForm`.
//! * **Arena** — the live grid, one cell per bot, colored by lifecycle status
//!   in the GTK CSS colors (GTK page 2).
//! * **Standings** — bots ranked by fitness (the GTK grid's fitness labels,
//!   as a leaderboard).
//! * **Replays** — the engine's per-generation snapshots, scrubbable.
//! * **Logs** — the engine transcript (GTK page 3).
//! * **Studio** — CorpusStudio's pipeline stepper (`studio` + `studio_host`),
//!   which GTK ships as a second binary; here it is a page of the one app.
//!
//! This crate drives the REAL engine: `bot-arena` (the parent crate, GTK-free
//! with `default-features = false`) runs on a worker thread and streams
//! `EngineMsg` back, exactly as it does under the GTK shell.

/// The real engine behind the dashboard: host, model, colors, setup form.
pub mod arena;
/// CorpusStudio: the corpus-LM pipeline as a live stepper (the sequence).
pub mod studio;
/// CorpusStudio's process half: argv construction, spawning, streaming.
pub mod studio_host;

use egui::{Align, Layout, Ui};
use uikit_egui::{Adaptive, GestureLayer, PaletteColors};

use arena::{ArenaModel, EngineHost, SetupForm};
use studio::Studio;
use studio_host::StudioHost;

/// The dashboard views, in swipe order.
pub const VIEWS: [&str; 6] = ["Setup", "Arena", "Standings", "Replays", "Logs", "Studio"];

/// Move the view index by a swipe sign (-1 / +1), clamped to the ends.
pub fn page(idx: usize, sign: i32, n: usize) -> usize {
    (idx as i32 + sign).clamp(0, n as i32 - 1) as usize
}

/// Dashboard state: the swipe recognizer, the engine and studio hosts, and
/// their live models. The bin owns one of these and nothing else.
pub struct Dashboard {
    pub view_idx: usize,
    /// The kit's segmented strip, built once from VIEWS.
    strip: uikit_egui::segmented_tabs::TabStrip,
    gesture: GestureLayer,
    palette: String,
    /// Run configuration (Setup page).
    pub form: SetupForm,
    /// The engine worker's channel + stop flag.
    pub engine: EngineHost,
    /// Everything the engine has reported this run.
    pub model: ArenaModel,
    /// Which replay generation is open on the Replays page.
    pub replay_sel: Option<usize>,
    /// The corpus pipeline's sequence state.
    pub studio: Studio,
    /// The corpus pipeline's process host.
    pub studio_host: StudioHost,
    /// Voice/palette/glove-mode metadata for this frame's widgets. See
    /// `uikit_egui::a11y` — registering here is what makes a control
    /// reachable by voice, and (via `glove_targets()`) by cold-weather
    /// glove mode, without a second hand-written list for each.
    pub a11y: uikit_egui::a11y::A11yScene,
}

impl Default for Dashboard {
    fn default() -> Self {
        Dashboard {
            view_idx: 0,
            strip: uikit_egui::segmented_tabs::TabStrip::new(&VIEWS),
            gesture: GestureLayer::horizontal_page(320.0),
            palette: String::new(),
            form: SetupForm::default(),
            engine: EngineHost::default(),
            model: ArenaModel::default(),
            replay_sel: None,
            studio: Studio::default(),
            studio_host: StudioHost::default(),
            a11y: uikit_egui::a11y::A11yScene::new(),
        }
    }
}

impl Dashboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a committed swipe (from the gesture layer) to change view. Exposed for tests.
    pub fn swipe(&mut self, sign: i32) {
        self.view_idx = page(self.view_idx, sign, VIEWS.len());
    }

    /// Adopt cross-app preferences: theme + text/button size on the context, and the swipe
    /// feel (sensitivity / commit / flick / detent animation) on this dashboard's gesture
    /// layer. Call after `Settings::poll()` reports a change so bot-arena tracks the fleet.
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        self.palette = prefs.palette.clone();
    }

    /// Test/introspection: the gesture layer's current takeover-lock (reflects detent animation).
    pub fn detent_lock_ms(&mut self) -> f64 {
        self.gesture.config_mut().takeover_lock_ms
    }

    pub fn current(&self) -> &'static str {
        VIEWS[self.view_idx]
    }

    /// Drain both workers into their models. Call once per frame, before
    /// `ui`; returns true if anything changed. While a worker is live the
    /// shell should also keep repainting, or a headless-fast run finishes
    /// silently between pointer events.
    pub fn tick(&mut self) -> bool {
        let a = self.engine.drain(&mut self.model);
        let b = self.studio_host.drain(&mut self.studio);
        a || b
    }

    /// True while either worker is alive (the shell's repaint hint).
    pub fn busy(&self) -> bool {
        self.engine.running() || self.studio_host.worker_running()
    }

    /// Render the dashboard. Swipe left/right pages between views; a tab row
    /// is shown instead whenever `VIEWS` actually fits the available width
    /// (`uikit_egui::responsive_nav`) — a real fit check, not a device-class
    /// guess, so the phone-vs-desktop line isn't drawn in the wrong place.
    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::palette_colors(&self.palette);
        self.a11y.begin_frame();

        // swipe paging via the shared gesture layer
        let surface = ui.available_rect_before_wrap();
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe(dir.sign());
        }

        // Which shape fits: a real width check against VIEWS.len(), not a
        // device-class guess. `adaptive.is_compact()` used to decide this
        // alone, which can disagree with an actual fit in both directions —
        // six short labels fit fine on a phone in landscape, and six long
        // ones might not fit on a "regular" phone `is_compact()` says isn't
        // narrow. `choose_adaptive` still reads the device's own touch
        // target for the minimum tab width, so a touch screen gets the
        // bigger, tappable segments it needs rather than as many as fit at
        // mouse-cursor precision.
        let layout = uikit_egui::responsive_nav::choose_adaptive(
            adaptive, ui.available_width(), VIEWS.len());
        if layout == uikit_egui::responsive_nav::NavLayout::Tabs {
            // The kit's strip, not a selectable_label row. Three apps were
            // hand-rolling the same thing and getting none of what the strip
            // gives: equal segments instead of tabs that resize with their
            // labels, an underline instead of a box that fights card borders,
            // and a shared badge cap.
            self.strip.select(self.view_idx);
            if let Some(i) = uikit_egui::segmented_tabs::strip_ui(ui, &self.strip, &colors) {
                self.view_idx = i;
            }
            ui.separator();
        } else {
            // a small "swipe" affordance + current view name for the compact layout
            ui.horizontal(|ui| {
                ui.colored_label(colors.text_weak, "‹ swipe ›");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.colored_label(colors.accent, self.current());
                });
            });
        }

        match self.view_idx {
            0 => self.setup_view(ui, &colors),
            1 => self.arena_view(ui, &colors),
            2 => self.standings_view(ui, &colors),
            3 => self.replays_view(ui, &colors),
            4 => self.logs_view(ui, &colors),
            _ => self.studio_view(ui, &colors),
        }

        // paint the swipe barrier progress if a swipe is in flight
        if let Some(edge) = frame.barrier_edge {
            uikit_egui::paint_barrier(ui, surface, frame.barrier_fraction, edge, colors.accent);
        }
    }

    // ── Setup ────────────────────────────────────────────────────────────

    fn setup_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Setup");
        egui::ScrollArea::vertical().id_salt("setup").show(ui, |ui| {
            let f = &mut self.form;
            let a11y = &mut self.a11y;
            egui::Grid::new("setup_core").num_columns(2).show(ui, |ui| {
                ui.colored_label(c.text, "Population");
                ui.add(egui::DragValue::new(&mut f.population).range(2..=2000));
                ui.end_row();
                ui.colored_label(c.text, "Workers per container");
                ui.add(egui::DragValue::new(&mut f.workers_per_container).range(1..=64));
                ui.end_row();
                ui.colored_label(c.text, "Keep fraction (survivors)");
                ui.add(egui::Slider::new(&mut f.keep_fraction, 0.05..=0.9));
                ui.end_row();
                ui.colored_label(c.text, "Generations");
                ui.add(egui::DragValue::new(&mut f.generations).range(1..=10000));
                ui.end_row();
                ui.colored_label(c.text, "Backend");
                egui::ComboBox::from_id_salt("backend")
                    .selected_text(["Sim", "Docker", "Conda", "Local"][f.backend_index.min(3) as usize])
                    .show_ui(ui, |ui| {
                        for (i, name) in ["Sim", "Docker", "Conda", "Local"].iter().enumerate() {
                            ui.selectable_value(&mut f.backend_index, i as u32, *name);
                        }
                    });
                ui.end_row();
                ui.colored_label(c.text, "Container image");
                uikit_egui::a11y::semantic_text_edit(
                    ui, &mut *a11y, egui::Id::new("bot_arena.setup.image").value(),
                    "Container image", &mut f.image);
                ui.end_row();
                ui.colored_label(c.text, "Workspace (repo/crate)");
                uikit_egui::a11y::semantic_text_edit(
                    ui, &mut *a11y, egui::Id::new("bot_arena.setup.workspace").value(),
                    "Workspace", &mut f.workspace);
                ui.end_row();
                ui.colored_label(c.text, "Corpus (latent purpose)");
                uikit_egui::a11y::semantic_text_edit(
                    ui, &mut *a11y, egui::Id::new("bot_arena.setup.corpus").value(),
                    "Corpus", &mut f.corpus);
                ui.end_row();
                ui.colored_label(c.text, "Behavior niches");
                ui.add(egui::DragValue::new(&mut f.niches).range(1..=32));
                ui.end_row();
                ui.colored_label(c.text, "Test command (auto = detect)");
                uikit_egui::a11y::semantic_text_edit(
                    ui, &mut *a11y, egui::Id::new("bot_arena.setup.test_cmd").value(),
                    "Test command", &mut f.test_cmd);
                ui.end_row();
                ui.colored_label(c.text, "Seed (blank = random)");
                uikit_egui::a11y::semantic_text_edit(
                    ui, &mut *a11y, egui::Id::new("bot_arena.setup.seed").value(),
                    "Seed", &mut f.seed);
                ui.end_row();
            });
            ui.checkbox(&mut self.form.turbo, "Turbo (skip generation animation pacing)");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.engine.running(), egui::Button::new("Start run"))
                    .clicked()
                {
                    self.engine.start(self.form.to_config(), &mut self.model);
                    self.replay_sel = None;
                    self.view_idx = 1; // jump to the arena, like the GTK Start does
                }
                if ui
                    .add_enabled(self.engine.running(), egui::Button::new("Stop"))
                    .on_hover_text("The engine stops at its next boundary")
                    .clicked()
                {
                    self.engine.stop();
                }
            });
        });
    }

    // ── Arena ────────────────────────────────────────────────────────────

    fn arena_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.horizontal(|ui| {
            ui.heading("Arena");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // The run control is the one button a voice or glove user
                // most needs to reach, so it registers even though its
                // label CHANGES between states — the id is stable and the
                // label is not, which is exactly the split `Semantics`
                // exists for: focus survives the state flip, the spoken
                // text follows it.
                if self.engine.running() {
                    if uikit_egui::a11y::semantic_button(
                        ui, &mut self.a11y,
                        egui::Id::new("bot_arena.run.stop").value(), "Stop")
                    {
                        self.engine.stop();
                    }
                    ui.colored_label(c.accent, "running");
                } else if uikit_egui::a11y::semantic_button(
                    ui, &mut self.a11y,
                    egui::Id::new("bot_arena.run.start").value(), "Start run")
                {
                    self.engine.start(self.form.to_config(), &mut self.model);
                    self.replay_sel = None;
                }
            });
        });
        if let Some((n, best, mean, alive, focus)) = self.model.generation {
            ui.colored_label(
                c.text,
                format!("gen {n}  best {best:.3}  mean {mean:.3}  alive {alive}  focus {focus:.2}"),
            );
        } else if self.model.bots.is_empty() {
            ui.colored_label(c.text_weak, "No run yet — configure Setup and press Start.");
        }
        if let Some((focus, label, reward)) = &self.model.coordinator {
            ui.colored_label(
                c.text_weak,
                format!("coordinator → {label} (focus {focus:.2}, reward {reward:.3})"),
            );
        }

        // The bot grid: one colored cell per bot, the GTK FlowBox. Cell size
        // adapts so a 500-bot swarm still fits a phone.
        let cell = if self.model.bots.len() > 160 { 26.0 } else { 44.0 };
        egui::ScrollArea::vertical().id_salt("arena_grid").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for b in &self.model.bots {
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(cell, cell),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, 4.0, arena::status_color(b.status));
                    if cell > 30.0 {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{:.2}", b.fitness),
                            egui::FontId::proportional(11.0),
                            egui::Color32::WHITE,
                        );
                    }
                    resp.on_hover_text(format!(
                        "bot {}  fitness {:.3}  z {:.2}\n{}",
                        b.id, b.fitness, b.z, b.purpose
                    ));
                }
            });
            // The live activity feed (the GTK activity panel).
            if !self.model.activity.is_empty() {
                ui.add_space(6.0);
                ui.colored_label(c.text_weak, "activity");
                for (id, text) in self.model.activity.iter().rev().take(8) {
                    ui.colored_label(
                        c.text_weak,
                        egui::RichText::new(format!("#{id} {text}")).monospace().size(11.0),
                    );
                }
            }
        });
    }

    // ── Standings ────────────────────────────────────────────────────────

    fn standings_view(&self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Standings");
        if self.model.bots.is_empty() {
            ui.colored_label(c.text_weak, "No run yet.");
            return;
        }
        egui::ScrollArea::vertical().id_salt("standings").show(ui, |ui| {
            for (i, b) in self.model.ranked().iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        if i == 0 { c.accent } else { c.text },
                        format!("{}. bot {}  —  {:.3}", i + 1, b.id, b.fitness),
                    );
                    if !b.purpose.is_empty() {
                        ui.colored_label(c.text_weak, &b.purpose);
                    }
                });
            }
        });
    }

    // ── Replays ──────────────────────────────────────────────────────────

    fn replays_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Replays");
        if self.model.replays.is_empty() {
            ui.colored_label(c.text_weak, "No replays yet — snapshots buffer as a run progresses.");
            return;
        }
        // Scrub: a slider over the buffered generations, then the snapshot.
        let max = self.model.replays.len() - 1;
        let mut sel = self.replay_sel.unwrap_or(max).min(max);
        if max > 0 {
            ui.add(egui::Slider::new(&mut sel, 0..=max).text("generation"));
        }
        self.replay_sel = Some(sel);
        let snap = &self.model.replays[sel];
        ui.colored_label(
            c.text,
            format!("gen {}  best {:.3}  mean {:.3}", snap.gen, snap.best, snap.mean),
        );
        ui.colored_label(c.text_weak, "leaders (id · z · fitness):");
        egui::ScrollArea::vertical().id_salt("replay_top").show(ui, |ui| {
            for (id, z, fit) in &snap.top {
                ui.colored_label(c.text, format!("bot {id}  ·  z {z:.2}  ·  {fit:.3}"));
            }
        });
    }

    // ── Logs ─────────────────────────────────────────────────────────────

    fn logs_view(&self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Logs");
        egui::ScrollArea::vertical()
            .id_salt("logs")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.model.logs.is_empty() {
                    ui.colored_label(c.text_weak, "The engine transcript appears here.");
                }
                for line in &self.model.logs {
                    ui.colored_label(c.text, egui::RichText::new(line).monospace().size(12.0));
                }
            });
    }

    // ── Studio ───────────────────────────────────────────────────────────

    fn studio_view(&mut self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Corpus Studio");
        egui::ScrollArea::vertical().id_salt("studio_page").show(ui, |ui| {
            let editable = !self.studio_host.worker_running();
            ui.add_enabled_ui(editable, |ui| {
                let cfg = &mut self.studio_host.cfg;
                let a11y = &mut self.a11y;
                egui::Grid::new("studio_cfg").num_columns(2).show(ui, |ui| {
                    ui.colored_label(c.text, "Corpus folder");
                    path_edit(ui, a11y, egui::Id::new("bot_arena.studio.corpus").value(), "Corpus folder", &mut cfg.corpus);
                    ui.end_row();
                    ui.colored_label(c.text, "CORPUS_EXT (comma-sep, blank = .md)");
                    uikit_egui::a11y::semantic_text_edit(
                        ui, a11y, egui::Id::new("bot_arena.studio.corpus_ext").value(),
                        "Corpus extensions", &mut cfg.corpus_ext);
                    ui.end_row();
                    ui.colored_label(c.text, "Binaries dir");
                    path_edit(ui, a11y, egui::Id::new("bot_arena.studio.bin_dir").value(), "Binaries dir", &mut cfg.bin_dir);
                    ui.end_row();
                    ui.colored_label(c.text, "Output dir");
                    path_edit(ui, a11y, egui::Id::new("bot_arena.studio.out_dir").value(), "Output dir", &mut cfg.out_dir);
                    ui.end_row();
                });
                ui.checkbox(&mut cfg.preprocess, "Run preprocessing first");
                if cfg.preprocess {
                    ui.checkbox(&mut cfg.builtin_pp, "Use built-in HTML→MD converter (Rust)");
                    if !cfg.builtin_pp {
                        ui.horizontal(|ui| {
                            ui.colored_label(c.text, "Tool");
                            let mut t = cfg
                                .tool
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default();
                            // One field, one id: this is not a list, it is
                            // a single optional tool path that happens to
                            // be edited through a temporary String.
                            if uikit_egui::a11y::semantic_text_edit(
                                ui, a11y,
                                egui::Id::new("bot_arena.studio.tool").value(),
                                "Preprocessing tool", &mut t)
                            {
                                let t = t.trim();
                                cfg.tool = if t.is_empty() {
                                    None
                                } else {
                                    Some(std::path::PathBuf::from(t))
                                };
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.colored_label(c.text, "Arg template ({in}, {out})");
                        ui.text_edit_singleline(&mut cfg.tool_args);
                    });
                }
                ui.collapsing("Pipeline parameters", |ui| {
                    egui::Grid::new("studio_params").num_columns(2).show(ui, |ui| {
                        for (label, v) in [
                            ("BPE merges", &mut cfg.merges),
                            ("N-gram max n", &mut cfg.max_n),
                            ("Word2Vec dim", &mut cfg.w2v_dim),
                            ("Word2Vec epochs", &mut cfg.w2v_epochs),
                            ("tiny_gpt steps", &mut cfg.gpt_steps),
                            ("tiny_gpt dim", &mut cfg.gpt_dim),
                            ("tiny_gpt layers", &mut cfg.gpt_layers),
                            ("Niches", &mut cfg.niches),
                        ] {
                            ui.colored_label(c.text, label);
                            ui.add(egui::DragValue::new(v));
                            ui.end_row();
                        }
                    });
                });
            });
            ui.add_space(4.0);
            ui.colored_label(c.text_weak, &self.studio_host.status);
            if let Some(act) = studio::studio_ui(ui, &mut self.studio, c) {
                self.studio_host.action(act, &mut self.studio);
            }
        });
    }
}

/// A PathBuf editor: text in, trimmed path out.
///
/// Takes the a11y scene and a caller-supplied id rather than inventing
/// one: this helper is used at several places, and an id derived here
/// would be the same for all of them — `A11yScene`'s focus persists
/// across frames BY ID, so colliding ids would make focus jump between
/// unrelated fields.
fn path_edit(ui: &mut Ui, a11y: &mut uikit_egui::a11y::A11yScene, id: u64,
             label: &str, p: &mut std::path::PathBuf) {
    let mut s = p.display().to_string();
    if uikit_egui::a11y::semantic_text_edit(ui, a11y, id, label, &mut s) {
        *p = std::path::PathBuf::from(s.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_widget_id_is_distinct() {
        // `A11yScene` keeps focus across frames BY ID, so two widgets
        // sharing an id make focus jump between unrelated fields — and
        // because the ids here are string-derived, a copy-paste of a
        // nearby line is exactly how that would happen. This lists every
        // id this app registers and asserts they are all different, which
        // is a check the compiler cannot make for us.
        let ids = [
            "bot_arena.setup.image",
            "bot_arena.setup.workspace",
            "bot_arena.setup.corpus",
            "bot_arena.setup.test_cmd",
            "bot_arena.setup.seed",
            "bot_arena.run.stop",
            "bot_arena.run.start",
            "bot_arena.studio.corpus",
            "bot_arena.studio.corpus_ext",
            "bot_arena.studio.bin_dir",
            "bot_arena.studio.out_dir",
            "bot_arena.studio.tool",
        ];
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            assert!(seen.insert(egui::Id::new(id).value()), "duplicate a11y id: {id}");
        }
        assert_eq!(seen.len(), ids.len());
    }

    #[test]
    fn paging_clamps_at_ends() {
        assert_eq!(page(0, -1, 6), 0, "can't page before the first view");
        assert_eq!(page(0, 1, 6), 1);
        assert_eq!(page(5, 1, 6), 5, "can't page past the last view");
        assert_eq!(page(1, -1, 6), 0);
    }

    #[test]
    fn swipe_moves_through_every_view() {
        let mut d = Dashboard::new();
        assert_eq!(d.current(), "Setup");
        for expect in ["Arena", "Standings", "Replays", "Logs", "Studio"] {
            d.swipe(1);
            assert_eq!(d.current(), expect);
        }
        d.swipe(1); // clamped
        assert_eq!(d.current(), "Studio");
        d.swipe(-1);
        assert_eq!(d.current(), "Logs");
    }

    #[test]
    fn a_run_started_from_the_dashboard_reaches_done_through_tick() {
        // The whole wiring in one test: form → Config → engine thread →
        // EngineMsg → tick() → model. This is what the placeholder data
        // could never exercise.
        let mut d = Dashboard::new();
        d.form.population = 6;
        d.form.generations = 2;
        d.engine.start(d.form.to_config(), &mut d.model);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while d.busy() {
            d.tick();
            assert!(std::time::Instant::now() < deadline, "engine hung");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        d.tick();
        assert!(d.model.done);
        assert_eq!(d.model.bots.len(), 6);
        assert!(!d.model.replays.is_empty());
    }

    #[test]
    fn apply_settings_adopts_gesture_feel_and_theme() {
        use settings_kit::{AnimationStyle, Prefs};
        let ctx = egui::Context::default();
        let mut d = Dashboard::new();
        let mut prefs = Prefs::default();
        prefs.gesture.animation = AnimationStyle::Heavy;
        d.apply_settings(&ctx, &prefs);
        // heavy detent animation → a long takeover lock came through from the shared settings
        assert_eq!(d.detent_lock_ms(), 220.0);
        // and applying doesn't disturb the current view
        assert_eq!(d.current(), "Setup");
    }
}
