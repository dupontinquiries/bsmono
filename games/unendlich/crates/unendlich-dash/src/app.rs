//! The dashboard UI (eframe/egui 0.34), styled as a near 1:1 port of the HTML
//! telemetry dashboard: a 340px sidebar (brand, source boxes, run-list cards,
//! sort/filter controls) and a main column of panel cards built from the
//! web-style widgets in `widgets`. In-window file picker, drag-and-drop, and a
//! Claude-ready digest copy are preserved.

use crate::model::{Run, Telemetry};
use crate::{digest, parse, plots, widgets};
use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use std::collections::BTreeMap;
use std::path::PathBuf;
use widgets::{AMBER, AMBER_SOFT, BG, CYAN, DIM, FAINT, GREEN, INK, LINE, PANEL, PANEL2, RED, VIOLET};

#[derive(Clone, Copy, PartialEq)]
enum PickTarget {
    Runs,
    Events,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Inspect,
    Compare,
    Trend,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Time,
    Rd,
    Gpu,
    Dur,
}
impl Sort {
    fn label(self) -> &'static str {
        match self {
            Sort::Time => "newest first",
            Sort::Rd => "render distance",
            Sort::Gpu => "GPU total ms",
            Sort::Dur => "duration",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mesh {
    All,
    On,
    Off,
}

pub struct DashApp {
    runs_path: String,
    events_path: String,
    tel: Telemetry,
    selected: Vec<usize>,
    status: String,
    view: View,
    sort: Sort,
    mesh: Mesh,
    picker: Option<PickTarget>,
    pick_dir: PathBuf,
    show_legend: bool,
}

impl Default for DashApp {
    fn default() -> Self {
        Self {
            runs_path: "run_summaries.txt".to_string(),
            events_path: "events.jsonl".to_string(),
            tel: Telemetry::default(),
            selected: Vec::new(),
            status: "Drop run_summaries.txt (+ optional events.jsonl), or click a source box to browse.".to_string(),
            view: View::Inspect,
            sort: Sort::Time,
            mesh: Mesh::All,
            picker: None,
            pick_dir: std::env::current_dir().unwrap_or_default(),
            show_legend: false,
        }
    }
}

// ── run-derived helpers ───────────────────────────────────────────────────────
fn gpu_mesher(run: &Run) -> bool {
    run.flag("UNENDLICH_GPU_MESHER") == Some("1")
        || run.flags.iter().any(|(k, v)| k.contains("GPU_MESHER") && v == "1")
        || run.env.as_deref().map(|e| e.contains("GPU_MESHER=1")).unwrap_or(false)
}

fn render_distance(run: &Run) -> i64 {
    for (k, v) in &run.flags {
        if k.contains("RENDER_DISTANCE") || k == "RD" || k == "rd" {
            if let Ok(n) = v.trim().parse::<i64>() {
                return n;
            }
        }
    }
    if let Some(env) = &run.env {
        for tok in env.split_whitespace() {
            if let Some(rest) = tok.strip_prefix("RD=").or_else(|| tok.strip_prefix("rd=")) {
                if let Ok(n) = rest.parse::<i64>() {
                    return n;
                }
            }
        }
    }
    0
}

fn duration_secs(run: &Run) -> f64 {
    run.duration
        .as_deref()
        .map(|d| d.trim().trim_end_matches('s').trim())
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn gpu_total_p99(run: &Run) -> f64 {
    run.gpu_total().map(|m| m.p99).unwrap_or(0.0)
}

impl DashApp {
    fn load_runs(&mut self) {
        match std::fs::read_to_string(&self.runs_path) {
            Ok(t) => {
                self.tel.runs = parse::parse_runs(&t);
                self.selected.clear();
                self.status = format!("Loaded {} run(s) from {}", self.tel.runs.len(), self.runs_path);
            }
            Err(e) => self.status = format!("run_summaries error: {e}"),
        }
    }

    fn load_events(&mut self) {
        match std::fs::read_to_string(&self.events_path) {
            Ok(t) => {
                self.tel.events = parse::parse_events(&t);
                self.status = format!("Loaded {} event(s)", self.tel.events.len());
            }
            Err(e) => self.status = format!("events error: {e}"),
        }
    }

    fn clear(&mut self) {
        self.tel = Telemetry::default();
        self.selected.clear();
        self.status = "Cleared.".to_string();
    }

    fn toggle(&mut self, i: usize) {
        match self.view {
            View::Inspect => {
                self.selected = vec![i];
            }
            View::Compare => {
                if let Some(pos) = self.selected.iter().position(|&x| x == i) {
                    self.selected.remove(pos);
                } else {
                    if self.selected.len() == 2 {
                        self.selected.remove(0);
                    }
                    self.selected.push(i);
                }
            }
            // Trend shows all runs at once; clicking a run just focuses it (no
            // effect on the aggregate plots, but keeps sidebar selection sane).
            View::Trend => {
                self.selected = vec![i];
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(path) = f.path {
                let p = path.to_string_lossy().to_string();
                if p.ends_with(".jsonl") {
                    self.events_path = p;
                    self.load_events();
                } else {
                    self.runs_path = p;
                    self.load_runs();
                }
            }
        }
    }

    fn open_picker(&mut self, target: PickTarget) {
        let cur = match target {
            PickTarget::Runs => &self.runs_path,
            PickTarget::Events => &self.events_path,
        };
        let start = std::path::Path::new(cur).parent().filter(|p| p.is_dir()).map(|p| p.to_path_buf());
        self.pick_dir = start.or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        self.picker = Some(target);
    }

    // ── achievements catalog (mirrors the HTML achievements()) ────────────────
    fn ach_cells(&self, run: &Run) -> Vec<(&'static str, &'static str, &'static str, bool, bool)> {
        let evs = self.tel.events_for(run);
        let has_events = !evs.is_empty();
        let shutdown = evs.iter().any(|e| e.kind.as_deref() == Some("shutdown"));
        let spike_kind = |needle: &str| evs.iter().any(|e| e.kind.as_deref().map(|k| k.contains("spike") && k.contains(needle)).unwrap_or(false));
        let loaded_max = run.workload.iter().find(|w| w.kind == "loaded").map(|w| w.max).unwrap_or(0);
        let shadow_p99 = run.gpu.iter().find(|m| m.stage == "shadow").map(|m| m.p99).unwrap_or(0.0);
        vec![
            ("\u{1f3c1}", "Clean exit", "shutdown recorded", has_events && shutdown, false),
            ("\u{1f4a5}", "Crash / no exit", "no shutdown event", has_events && !shutdown, true),
            ("\u{26a1}", "GPU mesher", "GPU_MESHER=1", gpu_mesher(run), false),
            ("\u{1f6f0}", "High render distance", "RD \u{2265} 8", render_distance(run) >= 8, false),
            ("\u{1f3d4}", "Heavy workload", "loaded_max \u{2265} 300", loaded_max >= 300, false),
            ("\u{23f1}", "Sustained session", "duration \u{2265} 120s", duration_secs(run) >= 120.0, false),
            ("\u{1f9ca}", "Wait-fence spikes", "wait_fences_spike", spike_kind("wait"), true),
            ("\u{1f4c8}", "GPU spikes", "gpu_spike", spike_kind("gpu"), true),
            ("\u{1f6a8}", "Over frame budget", "a stage p99 > 8.3ms", gpu_total_p99(run) > 8.3 || shadow_p99 > 8.3, true),
        ]
    }

    // ── in-window file picker (unchanged behaviour) ───────────────────────────
    fn file_picker(&mut self, ctx: &egui::Context) {
        let Some(target) = self.picker else {
            return;
        };
        let title = match target {
            PickTarget::Runs => "Open run_summaries",
            PickTarget::Events => "Open events.jsonl",
        };
        let want_ext: &[&str] = match target {
            PickTarget::Runs => &["txt", "log"],
            PickTarget::Events => &["jsonl", "json"],
        };
        let mut open = true;
        let mut navigate: Option<PathBuf> = None;
        let mut chosen: Option<String> = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 440.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("\u{2b06} Up").clicked() {
                        if let Some(p) = self.pick_dir.parent() {
                            navigate = Some(p.to_path_buf());
                        }
                    }
                    ui.label(RichText::new(self.pick_dir.display().to_string()).color(INK));
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt("dash-picker").show(ui, |ui| match std::fs::read_dir(&self.pick_dir) {
                    Ok(rd) => {
                        let (mut dirs, mut files): (Vec<PathBuf>, Vec<PathBuf>) = rd.flatten().map(|e| e.path()).partition(|p| p.is_dir());
                        dirs.sort();
                        files.sort();
                        for d in &dirs {
                            let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            if ui.button(format!("\u{1f4c1} {name}")).clicked() {
                                navigate = Some(d.clone());
                            }
                        }
                        for f in &files {
                            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let hot = f.extension().and_then(|e| e.to_str()).map(|e| want_ext.contains(&e)).unwrap_or(false);
                            let txt = RichText::new(format!("\u{1f4c4} {name}"));
                            let txt = if hot { txt.color(AMBER) } else { txt.weak() };
                            if ui.button(txt).clicked() {
                                chosen = Some(f.display().to_string());
                            }
                        }
                    }
                    Err(e) => {
                        ui.colored_label(RED, format!("cannot read directory: {e}"));
                    }
                });
            });
        if let Some(p) = navigate {
            self.pick_dir = p;
        }
        if let Some(path) = chosen {
            match target {
                PickTarget::Runs => {
                    self.runs_path = path;
                    self.load_runs();
                }
                PickTarget::Events => {
                    self.events_path = path;
                    self.load_events();
                }
            }
            self.picker = None;
        }
        if !open {
            self.picker = None;
        }
    }

    // ── sidebar ───────────────────────────────────────────────────────────────
    fn seclabel(ui: &mut egui::Ui, text: &str) {
        ui.label(RichText::new(text.to_uppercase()).size(9.0).color(FAINT));
    }

    fn dropzone(&mut self, ui: &mut egui::Ui, label: &str, target: PickTarget) {
        let (path, loaded) = match target {
            PickTarget::Runs => (self.runs_path.clone(), !self.tel.runs.is_empty()),
            PickTarget::Events => (self.events_path.clone(), !self.tel.events.is_empty()),
        };
        let short = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(path);
        Self::seclabel(ui, label);
        let (stroke, sub) = if loaded { (GREEN, GREEN) } else { (LINE, DIM) };
        let r = Frame {
            inner_margin: Margin::same(11),
            outer_margin: Margin { bottom: 4, ..Default::default() },
            fill: PANEL2,
            stroke: Stroke::new(1.0, stroke),
            corner_radius: CornerRadius::same(9),
            ..Default::default()
        }
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new("drop or click").size(12.0).color(INK));
                ui.label(RichText::new(short).size(9.5).color(sub));
            });
        });
        if r.response.interact(Sense::click()).clicked() {
            self.open_picker(target);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new("\u{25cf}").size(9.0).color(GREEN));
            ui.label(RichText::new("UNENDLICH").size(15.0).strong().color(AMBER));
            ui.label(RichText::new("TELEMETRY").size(10.0).color(DIM));
        });
        ui.add_space(12.0);

        self.dropzone(ui, "run_summaries.txt", PickTarget::Runs);
        self.dropzone(ui, "events.jsonl", PickTarget::Events);

        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() {
                self.clear();
            }
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            Self::seclabel(ui, "sort");
            egui::ComboBox::from_id_salt("sortsel").selected_text(self.sort.label()).show_ui(ui, |ui| {
                for s in [Sort::Time, Sort::Rd, Sort::Gpu, Sort::Dur] {
                    ui.selectable_value(&mut self.sort, s, s.label());
                }
            });
        });
        ui.horizontal(|ui| {
            Self::seclabel(ui, "mesh");
            for (m, lbl) in [(Mesh::All, "all"), (Mesh::On, "on"), (Mesh::Off, "off")] {
                if ui.selectable_label(self.mesh == m, lbl).clicked() {
                    self.mesh = m;
                }
            }
        });

        ui.add_space(8.0);
        Self::seclabel(ui, &format!("runs ({})", self.tel.runs.len()));

        // filtered + sorted index list
        let mut idxs: Vec<usize> = (0..self.tel.runs.len()).collect();
        idxs.retain(|&i| match self.mesh {
            Mesh::All => true,
            Mesh::On => gpu_mesher(&self.tel.runs[i]),
            Mesh::Off => !gpu_mesher(&self.tel.runs[i]),
        });
        match self.sort {
            Sort::Time => idxs.reverse(),
            Sort::Rd => idxs.sort_by(|&a, &b| render_distance(&self.tel.runs[b]).cmp(&render_distance(&self.tel.runs[a]))),
            Sort::Gpu => idxs.sort_by(|&a, &b| gpu_total_p99(&self.tel.runs[b]).total_cmp(&gpu_total_p99(&self.tel.runs[a]))),
            Sort::Dur => idxs.sort_by(|&a, &b| duration_secs(&self.tel.runs[b]).total_cmp(&duration_secs(&self.tel.runs[a]))),
        }

        let mut clicked: Option<usize> = None;
        egui::ScrollArea::vertical().id_salt("dash-main").max_height(ui.available_height() - 64.0).show(ui, |ui| {
            for i in idxs {
                let run = &self.tel.runs[i];
                let ts = run.start.clone().or_else(|| run.end.clone()).unwrap_or_else(|| format!("v{}", run.version.as_deref().unwrap_or("?")));
                let ov = run
                    .flags
                    .iter()
                    .take(3)
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("  ");
                let totals = format!(
                    "RD {}  \u{00b7}  {:.2}ms  \u{00b7}  {}s",
                    render_distance(run),
                    gpu_total_p99(run),
                    duration_secs(run) as i64
                );
                if widgets::runitem(ui, i + 1, &ts, &ov, &totals, self.selected.contains(&i)) {
                    clicked = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            self.toggle(i);
        }

        ui.add_space(6.0);
        ui.label(
            RichText::new("Green dashed = frame budget. Each panel has a copy button for that section. Switch to Compare for two-run \u{0394}.")
                .size(10.0)
                .color(FAINT),
        );
    }

    // ── top bar ─────────────────────────────────────────────────────────────
    fn topbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("SESSION").size(13.0).color(DIM));
            ui.label(RichText::new("PROFILER").size(13.0).strong().color(AMBER));
            ui.add_space(14.0);
            if ui.selectable_label(self.view == View::Inspect, RichText::new("Inspect").size(11.0)).clicked() {
                self.view = View::Inspect;
            }
            if ui.selectable_label(self.view == View::Compare, RichText::new("Compare").size(11.0)).clicked() {
                self.view = View::Compare;
            }
            if ui.selectable_label(self.view == View::Trend, RichText::new("Trend").size(11.0)).clicked() {
                self.view = View::Trend;
            }
            if unendlich_ui::button(ui, "legend", CYAN).clicked() {
                self.show_legend = !self.show_legend;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("Copy digest").size(11.0)).clicked() {
                    let d = digest::digest(&self.tel, &self.selected);
                    ui.ctx().copy_text(d);
                }
                ui.add_space(6.0);
                widgets::badge(ui, "refresh \u{00b7} 16.7ms", false);
            });
        });
        ui.label(RichText::new(&self.status).size(10.0).color(FAINT));
    }

    // ── inspect view ───────────────────────────────────────────────────────────
    fn inspect_panels(&self, ui: &mut egui::Ui, idx: usize) {
        let run = &self.tel.runs[idx];

        widgets::panel(ui, AMBER, "run", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("v{}", run.version.as_deref().unwrap_or("?"))).size(14.0).strong().color(AMBER));
                ui.add_space(10.0);
                if let Some(env) = &run.env {
                    ui.label(RichText::new(env).size(11.0).color(DIM));
                }
                if let (Some(s), Some(e)) = (&run.start, &run.end) {
                    ui.label(RichText::new(format!("{s} \u{2192} {e}")).size(11.0).color(DIM));
                }
                ui.label(RichText::new(format!("{}s", duration_secs(run) as i64)).size(11.0).color(INK));
                ui.add_space(8.0);
                let crashed = !self.tel.events_for(run).is_empty() && !self.tel.events_for(run).iter().any(|e| e.kind.as_deref() == Some("shutdown"));
                if crashed {
                    widgets::chip(ui, "CRASH / NO EXIT", RED);
                } else {
                    widgets::chip(ui, "OK", GREEN);
                }
            });
        });

        if !run.gpu.is_empty() {
            let scale = run.gpu.iter().map(|m| m.max).fold(0.0_f64, f64::max);
            let copy = section_copy("GPU timers (lifetime)", &run.gpu);
            widgets::panel(ui, AMBER, "GPU timers (lifetime)", Some(copy), |ui| {
                for m in &run.gpu {
                    widgets::metric_row(ui, &m.stage, &format!("n={}", m.n), Some(m), AMBER, scale, Some(8.3));
                }
            });
        }

        if !run.mesher.is_empty() {
            let scale = run.mesher.iter().map(|m| m.max).fold(0.0_f64, f64::max);
            let copy = section_copy("Mesher per LOD", &run.mesher);
            widgets::panel(ui, VIOLET, "Mesher per LOD", Some(copy), |ui| {
                for m in &run.mesher {
                    widgets::metric_row(ui, &m.stage, &format!("n={}", m.n), Some(m), VIOLET, scale, None);
                }
            });
        }

        if !run.workload.is_empty() {
            let scale = run.workload.iter().map(|w| w.max).max().unwrap_or(1) as f64;
            widgets::panel(ui, GREEN, "Workload shape", None, |ui| {
                for w in &run.workload {
                    widgets::workload_row(ui, w, scale);
                }
            });
        }

        if let Some(mem) = &run.memory {
            let used_mib = mem.octree_bytes as f64 / (1024.0 * 1024.0);
            let cap_mib = mem.octree_capacity as f64 / (1024.0 * 1024.0);
            let pct = mem.octree_frac() * 100.0;
            widgets::panel(ui, CYAN, "Memory \u{00b7} GPU pools", None, |ui| {
                ui.label(format!("octree pool: {:.1} / {:.1} MiB ({:.0}%)", used_mib, cap_mib, pct));
                ui.label(format!("octree chunks resident: {}", mem.octree_chunks));
                ui.label(format!("indirect draw slots: cap {}", mem.indirect_slots_cap));
                ui.label(format!(
                    "LOD chunks: near {} \u{00b7} mid {} \u{00b7} far {} \u{00b7} vfar {}",
                    mem.lod_near, mem.lod_mid, mem.lod_far, mem.lod_vfar,
                ));
                ui.horizontal(|ui| {
                    ui.label("OOM CPU-fallback chunks:");
                    if mem.gpu_oom_fallback > 0 {
                        widgets::chip(ui, &format!("{} \u{2014} stripe", mem.gpu_oom_fallback), RED);
                    } else {
                        widgets::chip(ui, "0", GREEN);
                    }
                });
            });
        }

        if let Some(cp) = &run.compression {
            let raw_mib = cp.raw_bytes as f64 / (1024.0 * 1024.0);
            let srun_mib = cp.surface_run_bytes as f64 / (1024.0 * 1024.0);
            let savings_mib = raw_mib - srun_mib;
            widgets::panel(ui, CYAN, "Compression \u{00b7} SurfaceRun (measurement)", None, |ui| {
                ui.label(format!("chunks sampled: {}", cp.chunks));
                ui.label(format!("raw \u{2192} surface_run: {:.1} \u{2192} {:.1} MiB", raw_mib, srun_mib));
                ui.horizontal(|ui| {
                    ui.label("ratio:");
                    let color = if cp.ratio >= 3.0 { GREEN }
                                else if cp.ratio >= 1.5 { AMBER }
                                else { RED };
                    widgets::chip(ui, &format!("{:.2}x", cp.ratio), color);
                    ui.label(format!("({:+.1} MiB)", -savings_mib));
                });
                ui.label(format!("worst column: {} runs (max = chunk_size)", cp.worst_col_runs));
            });
        }

        if let Some(ta) = &run.tick_apply {
            widgets::panel(ui, AMBER, "Tick apply \u{00b7} loading-wave hitch", None, |ui| {
                ui.label(format!("samples: {}", ta.n));
                ui.label(format!(
                    "apply ms: p50 {:.1} \u{00b7} p90 {:.1} \u{00b7} p99 {:.1}",
                    ta.p50_ms, ta.p90_ms, ta.p99_ms,
                ));
                ui.horizontal(|ui| {
                    ui.label("worst frame:");
                    let color = if ta.max_ms >= 50.0 { RED }
                                else if ta.max_ms >= 16.7 { AMBER }
                                else { GREEN };
                    widgets::chip(ui, &format!("{:.1} ms / {} chunks", ta.max_ms, ta.max_chunks), color);
                });
            });
        }

        let curve = self.tel.frame_curve(run);
        if !curve.is_empty() {
            let spikes = self.tel.spikes(run);
            widgets::panel(ui, AMBER, &format!("Frame-time curve \u{00b7} {} windows", curve.len()), None, |ui| {
                Plot::new("frame_curve").height(190.0).legend(Legend::default()).show(ui, |pu| {
                    pu.line(plots::frame_line(&curve));
                    for l in plots::spike_lines(&spikes) {
                        pu.line(l);
                    }
                });
            });
        }

        let cells = self.ach_cells(run);
        widgets::panel(ui, GREEN, "Achievements \u{00b7} what this run exercised", None, |ui| {
            unendlich_ui::grid_fit(ui, 150.0, 8.0, cells.len(), |ui, i| {
                let (ic, t, d, got, bad) = &cells[i];
                widgets::achievement_cell(ui, ic, t, d, *got, *bad);
            });
        });

        let evs = self.tel.events_for(run);
        if !evs.is_empty() {
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for e in &evs {
                *kinds.entry(e.kind.clone().unwrap_or_else(|| "?".into())).or_insert(0) += 1;
            }
            let mut sorted: Vec<(String, usize)> = kinds.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            widgets::panel(ui, AMBER, "Event kinds (events.jsonl)", None, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (k, c) in sorted {
                        let spike = k.contains("spike");
                        widgets::event_pill(ui, &k, c, spike);
                    }
                });
            });
        }

        widgets::panel(ui, AMBER, "Flags / overrides", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                if run.flags.is_empty() {
                    widgets::badge(ui, "all defaults", false);
                } else {
                    for (k, v) in &run.flags {
                        widgets::badge(ui, &format!("{k}={v}"), true);
                    }
                }
            });
        });
    }

    // ── compare view ───────────────────────────────────────────────────────────
    fn compare_panels(&self, ui: &mut egui::Ui, ai: usize, bi: usize) {
        let (a, b) = (&self.tel.runs[ai], &self.tel.runs[bi]);

        // Radar: A vs B across GPU passes, per-axis normalized so small and large
        // passes are both legible. Reuses the kit radar (mob/player profiles later).
        if !a.gpu.is_empty() {
            let axes: Vec<&str> = a.gpu.iter().map(|m| m.stage.as_str()).collect();
            let mut va: Vec<f32> = Vec::new();
            let mut vb: Vec<f32> = Vec::new();
            for ma in &a.gpu {
                let bp = b.gpu.iter().find(|m| m.stage == ma.stage).map(|m| m.p99).unwrap_or(0.0);
                let axis_max = ma.p99.max(bp).max(1e-6) * 1.15;
                va.push((ma.p99 / axis_max) as f32);
                vb.push((bp / axis_max) as f32);
            }
            widgets::panel(ui, VIOLET, "GPU pass radar \u{00b7} A vs B", None, |ui| {
                ui.horizontal(|ui| {
                    unendlich_ui::pill(ui, "A", AMBER, true);
                    unendlich_ui::pill(ui, "B", CYAN, true);
                    ui.label(RichText::new("per-axis normalized p99").size(10.0).color(FAINT));
                });
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    unendlich_ui::radar(ui, 360.0, &axes, &[("A", AMBER, va), ("B", CYAN, vb)]);
                });
            });
        }


        widgets::panel(ui, AMBER, "render distance \u{2194} performance", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::stat_card(ui, &format!("A \u{00b7} v{}", a.version.as_deref().unwrap_or("?")), &format!("{:.2} ms", gpu_total_p99(a)), AMBER);
                widgets::stat_card(ui, &format!("B \u{00b7} v{}", b.version.as_deref().unwrap_or("?")), &format!("{:.2} ms", gpu_total_p99(b)), CYAN);
                let d = gpu_total_p99(b) - gpu_total_p99(a);
                widgets::stat_card(ui, "\u{0394} total p99", &format!("{d:+.2} ms"), if d > 0.0 { RED } else { GREEN });
            });
        });

        let copy = {
            let mut s = String::from("stage\tA p99\tB p99\tdelta\n");
            for ma in &a.gpu {
                let bp = b.gpu.iter().find(|m| m.stage == ma.stage).map(|m| m.p99);
                s.push_str(&format!("{}\t{:.2}\t{}\t{}\n", ma.stage, ma.p99, bp.map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into()), bp.map(|v| format!("{:+.2}", v - ma.p99)).unwrap_or_else(|| "-".into())));
            }
            s
        };
        widgets::panel(ui, CYAN, "GPU pass p99 \u{00b7} A vs B with \u{0394}", Some(copy), |ui| {
            for ma in &a.gpu {
                let bp = b.gpu.iter().find(|m| m.stage == ma.stage).map(|m| m.p99);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(egui::vec2(120.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
                        ui.label(RichText::new(&ma.stage).size(12.0).color(INK));
                    });
                    ui.label(RichText::new(format!("A {:.2}", ma.p99)).size(11.0).color(AMBER_SOFT));
                    ui.label(RichText::new(bp.map(|v| format!("B {v:.2}")).unwrap_or_else(|| "B  -".into())).size(11.0).color(CYAN));
                    if let Some(v) = bp {
                        let d = v - ma.p99;
                        ui.label(RichText::new(format!("{d:+.2} ms")).size(11.0).strong().color(if d > 0.0 { RED } else { GREEN }));
                    }
                });
            }
        });

        // Scatter: every run as RD \u{2194} total p99; cyan = GPU mesher, amber = CPU,
        // A and B enlarged. (The web's RD \u{2194} perf view.)
        widgets::panel(ui, GREEN, "RD \u{2194} total p99 \u{00b7} all runs", None, |ui| {
            Plot::new("cmp_scatter").height(220.0).legend(Legend::default()).show(ui, |pu| {
                for (i, run) in self.tel.runs.iter().enumerate() {
                    let rd = render_distance(run) as f64;
                    let y = gpu_total_p99(run);
                    let emph = i == ai || i == bi;
                    let color = if gpu_mesher(run) { CYAN } else { AMBER };
                    let pts: PlotPoints = vec![[rd, y]].into_iter().collect();
                    pu.points(
                        egui_plot::Points::new(format!("run {}", i + 1), pts)
                            .radius(if emph { 6.0 } else { 3.5 })
                            .color(color),
                    );
                }
            });
        });

        let ca = self.tel.frame_curve(a);
        let cb = self.tel.frame_curve(b);
        if !ca.is_empty() || !cb.is_empty() {
            widgets::panel(ui, AMBER, "Frame-time curves overlaid", None, |ui| {
                Plot::new("cmp_curve").height(220.0).legend(Legend::default()).show(ui, |pu| {
                    if !ca.is_empty() {
                        let p: PlotPoints = ca.iter().copied().collect();
                        pu.line(Line::new("A", p));
                    }
                    if !cb.is_empty() {
                        let p: PlotPoints = cb.iter().copied().collect();
                        pu.line(Line::new("B", p));
                    }
                });
            });
        }
    }

    fn empty_state(&self, ui: &mut egui::Ui) {
        ui.add_space(80.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("no telemetry loaded").size(20.0).color(PANEL2));
            ui.add_space(8.0);
            ui.label(RichText::new("Drop run_summaries.txt onto the window, or click a source box.").size(11.0).color(FAINT));
        });
    }

    // ── trend view ──────────────────────────────────────────────────────────
    // Watch the webs shrink over project history. Each GPU stage gets one plot
    // with p50/p90/p99 trended across every run in order; the gap between the
    // lines is the box-plot "web" made into a timeline, so improvement reads as
    // all three lines descending AND p99 collapsing toward p50.
    fn trend_panels(&self, ui: &mut egui::Ui) {
        use crate::plots::Pctl;
        let runs = &self.tel.runs;
        let n = runs.len();
        if n < 2 {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("load more than one run to see trends").size(13.0).color(DIM));
            });
            return;
        }
        ui.label(
            RichText::new(format!("{n} runs in load order \u{00b7} lower is better \u{00b7} watch p99 collapse toward p50"))
                .size(11.0).color(FAINT),
        );

        // Horizontal frame-budget reference spanning all runs.
        let budget = 8.3f64;
        let budget_line = |label: &str| {
            let pts: PlotPoints = vec![[0.0, budget], [(n - 1) as f64, budget]].into_iter().collect();
            Line::new(label.to_string(), pts)
        };

        // One GPU stage → one plot with three percentile trend lines.
        let stage_panel = |ui: &mut egui::Ui, stage: &str, color: egui::Color32| {
            let p50 = plots::gpu_stage_trend(runs, stage, Pctl::P50);
            if p50.is_empty() {
                return;
            }
            widgets::panel(ui, color, &format!("{stage} \u{00b7} p50/p90/p99 over runs (ms)"), None, |ui| {
                Plot::new(format!("trend_{stage}"))
                    .height(170.0)
                    .legend(Legend::default())
                    .show(ui, |pu| {
                        pu.line(plots::trend_line("p99", plots::gpu_stage_trend(runs, stage, Pctl::P99)));
                        pu.line(plots::trend_line("p90", plots::gpu_stage_trend(runs, stage, Pctl::P90)));
                        pu.line(plots::trend_line("p50", p50.clone()));
                        pu.line(budget_line("8.3 ms budget"));
                    });
            });
        };

        stage_panel(ui, "total", AMBER);
        stage_panel(ui, "shadow", VIOLET);
        stage_panel(ui, "main", GREEN);

        // Stutter + geometry-load trends (CPU side).
        let tam = plots::tick_apply_max_trend(runs);
        if !tam.is_empty() {
            widgets::panel(ui, RED, "tick_apply max ms over runs (loading-wave hitch)", None, |ui| {
                Plot::new("trend_tick_apply")
                    .height(150.0)
                    .legend(Legend::default())
                    .show(ui, |pu| {
                        pu.line(plots::trend_line("max ms", tam.clone()));
                    });
            });
        }
        let lod = plots::lod_near_trend(runs);
        if !lod.is_empty() {
            widgets::panel(ui, CYAN, "near-LOD chunk count over runs (geometry load)", None, |ui| {
                Plot::new("trend_lod_near")
                    .height(150.0)
                    .legend(Legend::default())
                    .show(ui, |pu| {
                        pu.line(plots::trend_line("lod_near", lod.clone()));
                    });
            });
        }
    }

    fn render_view(&self, ui: &mut egui::Ui) {
        if self.tel.runs.is_empty() {
            self.empty_state(ui);
            return;
        }
        match self.view {
            View::Inspect => {
                let idx = self.selected.first().copied().unwrap_or(0).min(self.tel.runs.len() - 1);
                self.inspect_panels(ui, idx);
            }
            View::Compare => {
                if self.selected.len() < 2 {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("pick two runs in the sidebar to compare").size(13.0).color(DIM));
                    });
                } else {
                    self.compare_panels(ui, self.selected[0], self.selected[1]);
                }
            }
            View::Trend => {
                self.trend_panels(ui);
            }
        }
    }
}

fn section_copy(title: &str, metrics: &[crate::model::Metric]) -> String {
    let mut s = format!("{title}\nstage\tmean\tp99\tmax\tover\n");
    for m in metrics {
        s.push_str(&format!("{}\t{:.2}\t{:.2}\t{:.2}\t{}\n", m.stage, m.mean, m.p99, m.max, m.over_n.map(|n| n.to_string()).unwrap_or_else(|| "-".into())));
    }
    s
}

impl eframe::App for DashApp {
    // This eframe revision's `App` trait method is `ui(&mut self, &mut Ui, &mut Frame)`
    // (it hands us a Ui, not a Context), so panels mount with `show_inside`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        unendlich_ui::set_surface(&ctx, unendlich_ui::Surface::Solid);
        self.handle_drops(&ctx);

        egui::Panel::left("side")
            .exact_width(340.0)
            .resizable(false)
            .frame(Frame {
                fill: PANEL,
                inner_margin: Margin { left: 16, right: 16, top: 18, bottom: 16 },
                stroke: Stroke::new(1.0, LINE),
                ..Default::default()
            })
            .show_inside(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(Frame {
                fill: BG,
                inner_margin: Margin { left: 24, right: 24, top: 18, bottom: 36 },
                ..Default::default()
            })
            .show_inside(ui, |ui| {
                self.topbar(ui);
                ui.add_space(14.0);
                egui::ScrollArea::vertical().id_salt("dash-list").auto_shrink([false, false]).show(ui, |ui| {
                    self.render_view(ui);
                });
            });

        self.file_picker(&ctx);

        // Legend dialog — exercises the ask-first model end to end: no scrim, the
        // background run list is veto-protected while open, tap outside closes.
        if self.show_legend {
            // Frosted for the dialog only — validates the kit Surface seam while the
            // rest of the dashboard stays Solid. In-game this backdrop is the blur.
            unendlich_ui::set_surface(&ctx, unendlich_ui::Surface::frosted());
            let close = unendlich_ui::dialog(&ctx, CYAN, "Legend", |ui| {
                ui.label(RichText::new("Bars: mean fill, p99 whisker, max tick.").size(11.0).color(INK));
                ui.add_space(4.0);
                ui.label(RichText::new("Green dashed line = the frame budget.").size(11.0).color(DIM));
                ui.add_space(4.0);
                ui.label(RichText::new("Tap outside this card to close it.").size(10.0).color(FAINT));
            });
            unendlich_ui::set_surface(&ctx, unendlich_ui::Surface::Solid);
            if close {
                self.show_legend = false;
            }
        }
    }
}
