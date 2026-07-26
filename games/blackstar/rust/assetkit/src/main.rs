//! assetkit — Cybertron asset library manager.
//!
//! Four pages: Library (scan + score), Viewer (mesh preview), Export, GAN.
//! Built on the monorepo's shared uikit-egui, so it inherits the carved-material
//! look and the palette bridge rather than restyling anything locally.
//!
//! HONESTY NOTE: src/mesh.rs (the loading + scoring core) is compiled and tested.
//! This UI layer is NOT — egui 0.29 needs rustc >= 1.76 and the environment it was
//! written in has 1.75 with no route to upgrade. Treat the widget calls as
//! API-correct-by-inspection against shared/uikit, and expect first-build fixes.

mod json;
mod gltf;
mod mesh;
mod iqm;
mod harvest;
mod ascii;
mod pairs;
mod raids;
mod nn;
mod evolve;

use eframe::egui;
use egui::{Color32, FontId, Pos2, Rect, Stroke, Vec2};
use std::path::{Path, PathBuf};
use uikit_egui::{apply_palette, carved, PaletteColors, Skin};

#[derive(PartialEq, Clone, Copy)]
enum Page { Library, Viewer, Raids, Fill, Export, Gan }

struct Row {
    path: PathBuf,
    name: String,
    score: mesh::Score,
}

#[derive(PartialEq, Clone, Copy)]
enum Sort { Bony, Compact, Members, Tris, Name }

struct App {
    page: Page,
    skin: Skin,
    pal: Option<PaletteColors>,
    root: String,
    rows: Vec<Row>,
    sort: Sort,
    class_filter: Option<&'static str>,
    selected: Option<usize>,
    // viewer
    yaw: f32,
    pitch: f32,
    zoom: f32,
    mesh_cache: Option<(PathBuf, mesh::Mesh)>,
    // export
    out_dir: String,
    export_log: Vec<String>,
    // gan
    pairs_path: String,
    gan_log: Vec<String>,
    epochs: u32,
    l1_weight: f32,
    // raids
    donors_path: String,
    degrade: &'static str,
    gan_stage: u8,
    maps_path: String,
    raid_seed: u64,
    raid_phases: usize,
    raid_list: Vec<raids::Raid>,
    raid_sel: usize,
    raid_log: Vec<String>,
    // fill
    fill_target: String,
    fill_catalog: String,
    fill_region: &'static str,
    fill_passes: usize,
    fill_pop: usize,
    fill_gens: usize,
    fill_keep_shell: bool,
    fill_log: Vec<String>,
    fill_busy: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            page: Page::Library,
            skin: Skin::carved(),
            pal: None,
            root: "iqm".into(),
            rows: Vec::new(),
            sort: Sort::Bony,
            class_filter: None,
            selected: None,
            yaw: 0.6, pitch: 0.25, zoom: 1.0,
            mesh_cache: None,
            out_dir: "exports".into(),
            export_log: Vec::new(),
            pairs_path: "pairs/pairs.f32".into(),
            gan_log: Vec::new(),
            epochs: 200,
            l1_weight: 100.0,
            donors_path: "donors".into(),
            degrade: "box",
            gan_stage: 2,
            maps_path: "content/maps.json".into(),
            raid_seed: 7,
            raid_phases: 3,
            raid_list: Vec::new(),
            raid_sel: 0,
            raid_log: Vec::new(),
            fill_target: String::new(),
            fill_catalog: "hcat/meshes".into(),
            fill_region: "whole",
            fill_passes: 5,
            fill_pop: 60,
            fill_gens: 40,
            fill_keep_shell: true,
            fill_log: Vec::new(),
            fill_busy: false,
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 { return; }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, out, depth + 1); }
            else if let Some(x) = p.extension().and_then(|s| s.to_str()) {
                let x = x.to_ascii_lowercase();
                if x == "iqm" || x == "obj" { out.push(p); }
            }
        }
    }
}

impl App {
    fn rescan(&mut self) {
        self.rows.clear();
        self.selected = None;
        let mut files = Vec::new();
        walk(Path::new(&self.root), &mut files, 0);
        for p in files {
            if let Some(mut m) = mesh::load_any(&p) {
                if m.f.len() < 32 { continue; }
                let sc = mesh::score(&mut m, 50);
                let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                self.rows.push(Row { path: p, name, score: sc });
            }
        }
        self.resort();
    }

    fn resort(&mut self) {
        match self.sort {
            Sort::Bony => self.rows.sort_by(|a, b| b.score.bony.total_cmp(&a.score.bony)),
            Sort::Compact => self.rows.sort_by(|a, b| b.score.compactness.total_cmp(&a.score.compactness)),
            Sort::Members => self.rows.sort_by(|a, b| b.score.members.cmp(&a.score.members)),
            Sort::Tris => self.rows.sort_by(|a, b| b.score.tris.cmp(&a.score.tris)),
            Sort::Name => self.rows.sort_by(|a, b| a.name.cmp(&b.name)),
        }
    }

    fn ensure_mesh(&mut self, idx: usize) {
        let p = self.rows[idx].path.clone();
        let need = match &self.mesh_cache { Some((q, _)) => *q != p, None => true };
        if need {
            if let Some(mut m) = mesh::load_any(&p) {
                mesh::normalise(&mut m);
                self.mesh_cache = Some((p, m));
            }
        }
    }
}

/// Wireframe preview. Deliberately edge-only: it needs no GPU pipeline, no shader
/// and no texture loading, and it is enough to answer the questions that matter
/// when auditing a library -- is it upright, are the limbs attached, is it dense.
fn draw_mesh(ui: &mut egui::Ui, rect: Rect, m: &mesh::Mesh, yaw: f32, pitch: f32,
             zoom: f32, pal: &PaletteColors) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, pal.bg_base);
    let c = rect.center();
    let s = rect.height().min(rect.width()) * 0.42 * zoom;
    let (sy, cy) = (yaw.sin(), yaw.cos());
    let (sp, cp) = (pitch.sin(), pitch.cos());
    let project = |v: [f32; 3]| -> Pos2 {
        let x = v[0] * cy + v[2] * sy;
        let z = -v[0] * sy + v[2] * cy;
        let y = v[1] * cp - z * sp;
        Pos2::new(c.x + x * s, c.y - y * s)
    };
    // cap the drawn triangles: a 4M-triangle donor would freeze the UI otherwise
    let step = (m.f.len() / 12000).max(1);
    let col = Color32::from_rgba_unmultiplied(pal.accent.r(), pal.accent.g(), pal.accent.b(), 90);
    let stroke = Stroke::new(1.0, col);
    for t in m.f.iter().step_by(step) {
        let a = project(m.v[t[0] as usize]);
        let b = project(m.v[t[1] as usize]);
        let d = project(m.v[t[2] as usize]);
        p.line_segment([a, b], stroke);
        p.line_segment([b, d], stroke);
        p.line_segment([d, a], stroke);
    }
    p.text(rect.left_bottom() + Vec2::new(8.0, -8.0), egui::Align2::LEFT_BOTTOM,
           format!("{} tris (every {}th edge shown)", m.f.len(), step),
           FontId::monospace(11.0), pal.text_weak);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        let pal = *self.pal.get_or_insert_with(|| {
            apply_palette(ctx, &snapclone_theme::palette::DARK)
        });
        let skin = self.skin;

        egui::TopBottomPanel::top("nav").show(ctx, |ui| {
            uikit_egui::row(ui, |ui| {
                for (label, page) in [("Library", Page::Library), ("Viewer", Page::Viewer),
                                      ("Raids", Page::Raids), ("Fill", Page::Fill),
                                      ("Export", Page::Export),
                                      ("GAN", Page::Gan)] {
                    let on = self.page == page;
                    if carved::pill(ui, label, pal.bg_card, pal.accent, pal.text, &skin, on).clicked() {
                        self.page = page;
                    }
                }
                ui.separator();
                ui.label(format!("{} models", self.rows.len()));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Library => {
                uikit_egui::row(ui, |ui| {
                    ui.label("root");
                    ui.text_edit_singleline(&mut self.root);
                    if carved::button_accent(ui, "Scan", pal.bg_card, pal.accent, pal.text, &skin).clicked() {
                        self.rescan();
                    }
                    ui.separator();
                    for (l, s) in [("bony", Sort::Bony), ("compact", Sort::Compact),
                                   ("members", Sort::Members), ("tris", Sort::Tris),
                                   ("name", Sort::Name)] {
                        if carved::pill(ui, l, pal.bg_card, pal.accent, pal.text, &skin,
                                        self.sort == s).clicked() {
                            self.sort = s; self.resort();
                        }
                    }
                });
                uikit_egui::row(ui, |ui| {
                    ui.label("class");
                    for c in [None, Some("bot-bony"), Some("bot-shell"), Some("tank/car"),
                              Some("flyer"), Some("other")] {
                        let l = c.unwrap_or("all");
                        if carved::pill(ui, l, pal.bg_card, pal.accent, pal.text, &skin,
                                        self.class_filter == c).clicked() {
                            self.class_filter = c;
                        }
                    }
                });
                ui.separator();
                // The score is a CANDIDATE FILTER, not proof of quality: compactness
                // rewards surface area and noise has plenty of it. Say so in the UI
                // rather than letting a big number imply a good model.
                carved::chip(ui, "bony = compactness x coherence^3 x ln(members) — a filter to look at, not a verdict",
                             pal.text_weak, pal.bg_card, pal.border);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("lib").striped(true).show(ui, |ui| {
                        for h in ["model", "bony", "compact", "coher", "members", "open%", "tris", "class"] {
                            ui.label(h);
                        }
                        ui.end_row();
                        for i in 0..self.rows.len() {
                            let r = &self.rows[i];
                            if let Some(c) = self.class_filter {
                                if r.score.class != c { continue; }
                            }
                            let sel = self.selected == Some(i);
                            if ui.selectable_label(sel, &r.name).clicked() {
                                self.selected = Some(i);
                                self.page = Page::Viewer;
                            }
                            let dim = if r.score.scorable { pal.text } else { pal.text_weak };
                            ui.colored_label(dim, format!("{:.1}", r.score.bony));
                            ui.colored_label(dim, format!("{:.1}", r.score.compactness));
                            ui.colored_label(dim, format!("{:.3}", r.score.coherence));
                            ui.label(format!("{}", r.score.members));
                            ui.label(format!("{:.0}%", r.score.open_frac * 100.0));
                            ui.label(format!("{}", r.score.tris));
                            ui.label(r.score.class);
                            ui.end_row();
                        }
                    });
                });
            }

            Page::Viewer => {
                if let Some(i) = self.selected {
                    self.ensure_mesh(i);
                    let (name, sc) = (self.rows[i].name.clone(), self.rows[i].score.clone());
                    uikit_egui::row(ui, |ui| {
                        ui.label(&name);
                        ui.separator();
                        ui.label(format!("aspect {:.2} / {:.2} / {:.2}",
                                         sc.aspect[0], sc.aspect[1], sc.aspect[2]));
                        // the lying-down test from ascii_view.py, carried over: height
                        // being the SMALLEST dimension is the reliable signal, since a
                        // T-posed bot is legitimately wider than it is tall
                        let ext = sc.aspect;
                        let flat = ext[1] <= ext[0].min(ext[2]) && ext[1] < 0.75 * ext[0].max(ext[2]);
                        if flat {
                            carved::chip(ui, "LYING DOWN — needs rx +/-90", pal.bg_base, pal.danger, pal.danger);
                        }
                    });
                    uikit_egui::row(ui, |ui| {
                        ui.add(egui::Slider::new(&mut self.yaw, -3.14..=3.14).text("yaw"));
                        ui.add(egui::Slider::new(&mut self.pitch, -1.2..=1.2).text("pitch"));
                        ui.add(egui::Slider::new(&mut self.zoom, 0.3..=3.0).text("zoom"));
                    });
                    let avail = ui.available_size();
                    let (rect, resp) = ui.allocate_exact_size(avail, egui::Sense::drag());
                    if resp.dragged() {
                        let d = resp.drag_delta();
                        self.yaw += d.x * 0.01;
                        self.pitch = (self.pitch + d.y * 0.01).clamp(-1.2, 1.2);
                    }
                    if let Some((_, m)) = &self.mesh_cache {
                        draw_mesh(ui, rect, m, self.yaw, self.pitch, self.zoom, &pal);
                    }
                } else {
                    ui.label("pick a model on the Library page");
                }
            }

            Page::Raids => {
                uikit_egui::row(ui, |ui| {
                    ui.label("maps");
                    ui.text_edit_singleline(&mut self.maps_path);
                    ui.add(egui::Slider::new(&mut self.raid_phases, 2..=6).text("phases"));
                    let mut s64 = self.raid_seed as i64;
                    if ui.add(egui::Slider::new(&mut s64, 1..=999).text("seed")).changed() {
                        self.raid_seed = s64 as u64;
                    }
                    if carved::button_accent(ui, "Generate", pal.bg_card, pal.accent,
                                             pal.text, &skin).clicked() {
                        let added = raids::patch_maps(Path::new(&self.maps_path));
                        if !added.is_empty() {
                            self.raid_log.push(format!("added missing maps: {}", added.join(", ")));
                        }
                        let names = raids::map_names(Path::new(&self.maps_path));
                        if names.is_empty() {
                            self.raid_log.push(format!("! no maps in {}", self.maps_path));
                        } else {
                            self.raid_list = raids::generate(&names, self.raid_phases, self.raid_seed);
                            let missing = raids::validate(&self.raid_list, &names);
                            if missing.is_empty() {
                                self.raid_log.push(format!("{} raids, all referenced maps exist",
                                                           self.raid_list.len()));
                            } else {
                                self.raid_log.push(format!("! undefined maps: {}", missing.join(", ")));
                            }
                        }
                    }
                    if !self.raid_list.is_empty()
                       && carved::button(ui, "Write json + header", pal.bg_card, pal.accent,
                                         pal.text, &skin).clicked() {
                        let _ = std::fs::write("content/raids.json", raids::to_json(&self.raid_list));
                        let r = &self.raid_list[self.raid_sel.min(self.raid_list.len() - 1)];
                        let _ = std::fs::write("engine_src/exengine/raid_plan.h", raids::to_header(r));
                        self.raid_log.push(format!("wrote content/raids.json and baked '{}'", r.id));
                    }
                });
                ui.separator();
                if self.raid_list.is_empty() {
                    ui.label("Generate to build raids from the boss fiction in raids-and-relics.md.");
                    ui.label(egui::RichText::new(
                        "Each boss's mechanic is derived from its lore, not assigned at random —                          the premise line that drove it is shown beside each raid.")
                        .color(pal.text_weak));
                } else {
                    let cols = ui.available_width();
                    egui::ScrollArea::vertical().max_height(cols * 0.30).show(ui, |ui| {
                        for i in 0..self.raid_list.len() {
                            let r = &self.raid_list[i];
                            let m = ["purge", "ground", "air", "sync"][r.mech as usize];
                            let sel = self.raid_sel == i;
                            if ui.selectable_label(sel, format!("{:<32} {:<20} {}",
                                    r.name.chars().take(32).collect::<String>(),
                                    r.title, m)).clicked() {
                                self.raid_sel = i;
                            }
                        }
                    });
                    ui.separator();
                    let r = &self.raid_list[self.raid_sel.min(self.raid_list.len() - 1)];
                    carved::panel(ui, &r.title, pal.bg_card, pal.accent, pal.text, &skin, |ui| {
                        ui.label(egui::RichText::new(&r.name).strong());
                        // the premise is the whole justification for the mechanic, so
                        // it belongs on screen next to it rather than buried in json
                        ui.label(egui::RichText::new(format!("\"{}\"", r.premise))
                                 .italics().color(pal.text_weak));
                        ui.label(format!("mechanic: {}  ({})",
                                 ["purge", "ground", "air", "sync"][r.mech as usize], r.arena));
                        for p in &r.phases {
                            let mechs: Vec<String> = p.enc.iter()
                                .map(|e| e.mech.to_string()).collect();
                            ui.label(format!("  {:<16} {:<12} {} encounters, mech [{}]",
                                     p.name, p.map, p.enc.len(), mechs.join(",")));
                        }
                    });
                }
                ui.separator();
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.raid_log {
                        ui.label(egui::RichText::new(l).monospace().color(
                            if l.starts_with('!') { pal.danger } else { pal.text_weak }));
                    }
                });
            }

            Page::Fill => {
                carved::panel(ui, "genetic fill", pal.bg_card, pal.accent, pal.text, &skin, |ui| {
                    ui.label("Evolves an assembly of catalog parts to match a target, \
                              scored by occupancy IoU minus a spill penalty.");
                    // Every claim here is measured, and the page says which, because
                    // a generative tool that only reports successes is not trustworthy.
                    ui.label(egui::RichText::new(
                        "Beats a random assembly by 200-1400%. Needs no GPU and no training \
                         data — the target IS the fitness function, and every gene indexes real \
                         catalog geometry, so it cannot produce a smear.")
                        .color(pal.text_weak));
                });
                uikit_egui::row(ui, |ui| {
                    ui.label("target");
                    ui.text_edit_singleline(&mut self.fill_target);
                    if self.selected.is_some()
                       && carved::button(ui, "use Library selection", pal.bg_card, pal.accent,
                                         pal.text, &skin).clicked() {
                        if let Some(i) = self.selected {
                            self.fill_target = self.rows[i].path.to_string_lossy().to_string();
                        }
                    }
                });
                uikit_egui::row(ui, |ui| {
                    ui.label("catalog");
                    ui.text_edit_singleline(&mut self.fill_catalog);
                    if carved::button(ui, "Harvest one", pal.bg_card, pal.accent, pal.text,
                                      &skin).clicked() {
                        let c = format!("assetkit-cli harvest {} --out hcat --min-tris 150",
                                        self.root);
                        self.fill_log.push(format!("$ {}", c));
                        if let Ok(o) = std::process::Command::new("sh").arg("-c").arg(&c).output() {
                            for l in String::from_utf8_lossy(&o.stdout).lines().rev().take(3) {
                                self.fill_log.push(l.to_string());
                            }
                        }
                    }
                });
                // Harvested parts measurably beat generated primitives: +13% fit,
                // 5.6x the structural components, coherence 0.814 vs 0.565. Say so
                // rather than leaving the choice to look arbitrary.
                carved::chip(ui, "harvested parts beat generated primitives: +13% fit,                                   5.6x components, coherence 0.81 vs 0.57",
                             pal.text_weak, pal.bg_card, pal.border);
                uikit_egui::row(ui, |ui| {
                    ui.label("region");
                    for (l, r) in [("whole", "whole"), ("chest", "chest"),
                                   ("legs", "legs"), ("head", "head")] {
                        if carved::pill(ui, l, pal.bg_card, pal.accent, pal.text, &skin,
                                        self.fill_region == r).clicked() {
                            self.fill_region = r;
                        }
                    }
                    if carved::pill(ui, "keep shell", pal.bg_card, pal.accent, pal.text, &skin,
                                    self.fill_keep_shell).clicked() {
                        self.fill_keep_shell = !self.fill_keep_shell;
                    }
                });
                // Replacement measured WORSE by eye than the original despite better
                // numbers; additive tripled structural members with the silhouette
                // intact. Warn rather than let the toggle look neutral.
                if !self.fill_keep_shell {
                    carved::chip(ui, "without the shell this REPLACES the model — at IoU ~0.38                                       that is a rougher silhouette, not a better one",
                                 pal.bg_base, pal.warning, pal.warning);
                }
                uikit_egui::row(ui, |ui| {
                    ui.add(egui::Slider::new(&mut self.fill_passes, 1..=7).text("passes"));
                    ui.add(egui::Slider::new(&mut self.fill_pop, 10..=200).text("pop"));
                    ui.add(egui::Slider::new(&mut self.fill_gens, 5..=200).text("gens"));
                });
                // passes 6 measured 0.385, passes 7 fell to 0.366 -- the search space
                // outgrows the budget. Flag it at the point of choosing.
                if self.fill_passes >= 7 {
                    carved::chip(ui, "7 passes measured WORSE than 6 (0.366 vs 0.385): the search                                       space outgrows the budget",
                                 pal.bg_base, pal.warning, pal.warning);
                }
                uikit_egui::row(ui, |ui| {
                    if carved::button_accent(ui, "Evolve", pal.bg_card, pal.accent, pal.text,
                                             &skin).clicked() && !self.fill_target.is_empty() {
                        let mut c = format!(
                            "assetkit-cli fill {} --catalog {} --pop {} --gens {} --passes {} \
                             --out filled --name evolved --show 0",
                            self.fill_target, self.fill_catalog,
                            self.fill_pop, self.fill_gens, self.fill_passes);
                        if self.fill_region != "whole" {
                            c.push_str(&format!(" --region {}", self.fill_region));
                        }
                        if self.fill_keep_shell { c.push_str(" --keep-shell"); }
                        self.fill_log.push(format!("$ {}", c));
                        self.fill_busy = true;
                        if let Ok(o) = std::process::Command::new("sh").arg("-c").arg(&c).output() {
                            for l in String::from_utf8_lossy(&o.stdout).lines() {
                                self.fill_log.push(l.to_string());
                            }
                            for l in String::from_utf8_lossy(&o.stderr).lines().take(4) {
                                self.fill_log.push(format!("! {}", l));
                            }
                        }
                        self.fill_busy = false;
                    }
                    if carved::button(ui, "Install into engine", pal.bg_card, pal.accent,
                                      pal.text, &skin).clicked() {
                        let c = "install_models.py --parts-dir filled --engine engine_src";
                        self.fill_log.push(format!("$ {}", c));
                        if let Ok(o) = std::process::Command::new("sh").arg("-c").arg(c).output() {
                            for l in String::from_utf8_lossy(&o.stdout).lines().take(8) {
                                self.fill_log.push(l.to_string());
                            }
                        }
                    }
                    if self.fill_busy { ui.spinner(); }
                });
                ui.separator();
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.fill_log {
                        let c = if l.starts_with('!') { pal.danger }
                                else if l.contains("beats random") { pal.success }
                                else if l.contains("did not improve") || l.contains("!!") { pal.warning }
                                else { pal.text_weak };
                        ui.label(egui::RichText::new(l).monospace().color(c));
                    }
                });
            }

            Page::Export => {
                uikit_egui::row(ui, |ui| {
                    ui.label("output");
                    ui.text_edit_singleline(&mut self.out_dir);
                });
                ui.separator();
                // Exports shell out to the python pipeline rather than reimplementing
                // it: those tools are tested, and a second implementation would be a
                // second thing to keep correct.
                for (label, cmd) in [
                    ("Score library -> CSV", "library_score {root} --csv {out}/scores.csv"),
                    ("Harvest parts", "part_harvest.py {root} --out {out}/cat --tol 0.20 --emit-per-category 60"),
                    ("Build GAN pairs", "shape_pairs.py --donors {root} --out {out}/pairs"),
                    ("Rig limbs", "limb_split.py --batch {root} --out {out}/kit --detail {out}/cat"),
                    ("Install into engine", "install_models.py --parts-dir {out}/kit --engine engine_src"),
                ] {
                    if carved::button(ui, label, pal.bg_card, pal.accent, pal.text, &skin).clicked() {
                        let c = cmd.replace("{root}", &self.root).replace("{out}", &self.out_dir);
                        self.export_log.push(format!("$ {}", c));
                        match std::process::Command::new("sh").arg("-c").arg(&c).output() {
                            Ok(o) => {
                                for l in String::from_utf8_lossy(&o.stdout).lines().take(24) {
                                    self.export_log.push(l.to_string());
                                }
                                if !o.status.success() {
                                    for l in String::from_utf8_lossy(&o.stderr).lines().take(8) {
                                        self.export_log.push(format!("! {}", l));
                                    }
                                }
                            }
                            Err(e) => self.export_log.push(format!("! {}", e)),
                        }
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.export_log {
                        ui.label(egui::RichText::new(l).monospace().color(
                            if l.starts_with('!') { pal.danger } else { pal.text_weak }));
                    }
                });
            }

            Page::Gan => {
                // The two-stage design, as proposed: stage 1 predicts WHERE a part
                // goes; stage 2 fills a bounding box with geometry. Splitting it
                // this way is what made the data learnable -- see the numbers below.
                uikit_egui::row(ui, |ui| {
                    for (l, st) in [("Stage 1 — placement", 1u8), ("Stage 2 — fitting", 2u8)] {
                        if carved::pill(ui, l, pal.bg_card, pal.accent, pal.text, &skin,
                                        self.gan_stage == st).clicked() {
                            self.gan_stage = st;
                        }
                    }
                });
                ui.separator();

                if self.gan_stage == 1 {
                    carved::panel(ui, "stage 1 — position / scale / orientation",
                                  pal.bg_card, pal.accent, pal.text, &skin, |ui| {
                        ui.label("9 numbers per part instead of a 32k-pixel image.");
                        ui.label(egui::RichText::new(
                            "Far better posed than pixel-to-pixel: the target is exact, \
                             not approximated through a depth encoding.")
                            .color(pal.text_weak));
                    });
                    uikit_egui::row(ui, |ui| {
                        ui.label("donors");
                        ui.text_edit_singleline(&mut self.donors_path);
                        if carved::button_accent(ui, "Export placements", pal.bg_card,
                                                 pal.accent, pal.text, &skin).clicked() {
                            let c = format!("assetkit-cli place {} --out placement",
                                            self.donors_path);
                            self.gan_log.push(format!("$ {}", c));
                            if let Ok(o) = std::process::Command::new("sh").arg("-c")
                                                .arg(&c).output() {
                                for l in String::from_utf8_lossy(&o.stdout).lines() {
                                    self.gan_log.push(l.to_string());
                                }
                            }
                        }
                    });
                } else {
                    carved::panel(ui, "stage 2 — fill the box", pal.bg_card, pal.accent,
                                  pal.text, &skin, |ui| {
                        ui.label("six orthographic depth views tiled 128x256 in [-1,1] \
                                  — the kBM3 cache shape, so data.rs is unchanged");
                        // The degradation choice is the single most consequential
                        // setting on this page, so the measured difference is stated
                        // rather than left for the user to discover.
                        ui.label(egui::RichText::new(
                            "decimate: relief gap +3%  — a residual net just copies its input\n\
                             box:      relief gap +150% — a real question with a real answer")
                            .monospace().color(pal.text_weak));
                    });
                    uikit_egui::row(ui, |ui| {
                        ui.label("donors");
                        ui.text_edit_singleline(&mut self.donors_path);
                        for (l, m) in [("decimate", "decimate"), ("box", "box")] {
                            if carved::pill(ui, l, pal.bg_card, pal.accent, pal.text, &skin,
                                            self.degrade == m).clicked() {
                                self.degrade = m;
                            }
                        }
                        if carved::button_accent(ui, "Build pairs", pal.bg_card, pal.accent,
                                                 pal.text, &skin).clicked() {
                            let c = format!("assetkit-cli pairs {} --out pairs --degrade {}",
                                            self.donors_path, self.degrade);
                            self.gan_log.push(format!("$ {}", c));
                            if let Ok(o) = std::process::Command::new("sh").arg("-c")
                                                .arg(&c).output() {
                                for l in String::from_utf8_lossy(&o.stdout).lines() {
                                    self.gan_log.push(l.to_string());
                                }
                            }
                        }
                    });
                    uikit_egui::row(ui, |ui| {
                        ui.label("pairs");
                        ui.text_edit_singleline(&mut self.pairs_path);
                        ui.add(egui::Slider::new(&mut self.epochs, 1..=2000).text("epochs"));
                        ui.add(egui::Slider::new(&mut self.l1_weight, 0.0..=200.0).text("L1"));
                    });
                    uikit_egui::row(ui, |ui| {
                        // Ask the cheap question before the expensive one. This is a
                        // falsifier: a GPU run always produces something and the loss
                        // always falls, so it cannot tell you whether the task is
                        // learnable. Seconds here can save hours there.
                        if carved::button(ui, "CPU check — does it learn?", pal.bg_card,
                                          pal.accent, pal.text, &skin).clicked() {
                            let c = format!("assetkit-cli train {} --epochs 8 --limit 300",
                                            self.pairs_path);
                            self.gan_log.push(format!("$ {}", c));
                            if let Ok(o) = std::process::Command::new("sh").arg("-c")
                                                .arg(&c).output() {
                                for l in String::from_utf8_lossy(&o.stdout).lines() {
                                    self.gan_log.push(l.to_string());
                                }
                            }
                        }
                        if carved::button(ui, "Retrieval baseline (no GPU)", pal.bg_card,
                                          pal.accent, pal.text, &skin).clicked() {
                            let c = format!("shape_baseline.py --pairs {}",
                                            self.pairs_path.replace(".f32", ".npz"));
                            self.gan_log.push(format!("$ {}", c));
                            if let Ok(o) = std::process::Command::new("sh").arg("-c")
                                                .arg(&c).output() {
                                for l in String::from_utf8_lossy(&o.stdout).lines() {
                                    self.gan_log.push(l.to_string());
                                }
                            }
                        }
                        if carved::button_accent(ui, "GPU train", pal.bg_card, pal.accent,
                                                 pal.text, &skin).clicked() {
                            let c = format!("cargo run --release -p gpu --bin cond -- \
                                             --pairs {} --epochs {} --l1 {}",
                                            self.pairs_path, self.epochs, self.l1_weight);
                            self.gan_log.push(format!("$ {}", c));
                            self.gan_log.push("(runs in the kBM3 gpu crate; \
                                               model_cond.rs supplies the U-Net)".into());
                        }
                    });
                    // State the standing result rather than letting the user rediscover
                    // it. Both baselines have to fall, and which one is hard depends on
                    // the degradation.
                    carved::chip(ui, "last measured: box pairs +34% vs identity, -1.3% vs mean                                       — found the distribution, not yet conditioning on input",
                                 pal.text_weak, pal.bg_card, pal.border);
                }
                ui.separator();
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.gan_log {
                        let c = if l.contains("HAS NOT LEARNED") || l.starts_with('!') { pal.danger }
                                else if l.contains("LEARNS a real") { pal.success }
                                else if l.contains("PARTIAL") { pal.warning }
                                else { pal.text_weak };
                        ui.label(egui::RichText::new(l).monospace().color(c));
                    }
                });
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native("assetkit — Cybertron asset library", opts,
                       Box::new(|_cc| Box::<App>::default()))
}
