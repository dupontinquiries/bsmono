//! Files page — browse the repo tree (cockpit::browse::list_repo_dir) and peek a file's contents.
//! Local repo, so navigation + reads are synchronous.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub subpath: String,
    pub dirs: Vec<String>,
    pub files: Vec<String>,
    pub loaded: bool,
    pub err: String,
    pub peek_name: String,
    pub peek_content: String,
    /// Fuzzy filter over the current directory (voiceterm's nav-palette pattern).
    pub filter: String,
}

fn load(state: &mut State) {
    state.loaded = true;
    state.err.clear();
    let root = cockpit::ops::repo_root();
    match cockpit::browse::list_repo_dir(&root, &state.subpath) {
        Ok((rel, dirs, files)) => {
            state.subpath = rel;
            state.dirs = dirs;
            state.files = files;
        }
        Err(e) => state.err = e,
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Files");
    ui.separator();
    if !app.files.loaded {
        load(&mut app.files);
    }

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("/{}", app.files.subpath)).monospace().weak());
        if !app.files.subpath.is_empty() && ui.button("⬆ up").clicked() {
            let up = std::path::Path::new(&app.files.subpath)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            app.files.subpath = up;
            load(&mut app.files);
        }
        if ui.button("↻").clicked() {
            load(&mut app.files);
        }
    });
    if !app.files.err.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(0xf7, 0x76, 0x8e), &app.files.err);
    }

    // Fuzzy filter over this directory — type a few letters, subsequence-matched (voiceterm palette).
    ui.horizontal(|ui| {
        ui.label("🔍");
        ui.add(
            egui::TextEdit::singleline(&mut app.files.filter)
                .hint_text("filter — type a few letters")
                .desired_width(240.0),
        );
        if !app.files.filter.is_empty() && ui.button("✕").clicked() {
            app.files.filter.clear();
        }
    });

    let filter = app.files.filter.clone();
    // dirs: keep matches in listing order
    let dirs: Vec<String> = app
        .files
        .dirs
        .iter()
        .filter(|d| uikit_egui::fuzzy::subsequence_match(&filter, d))
        .cloned()
        .collect();
    // files: matches, ranked best-first while filtering
    let mut files: Vec<String> = app
        .files
        .files
        .iter()
        .filter(|f| uikit_egui::fuzzy::subsequence_match(&filter, f))
        .cloned()
        .collect();
    if !filter.is_empty() {
        files.sort_by_key(|f| -uikit_egui::fuzzy::score(&filter, f).unwrap_or(0));
    }
    let hidden = (app.files.dirs.len() + app.files.files.len()) - (dirs.len() + files.len());

    uikit_egui::scroll::bounded(ui, 240.0, |ui| {
        for d in dirs {
            if ui.button(format!("📁 {d}")).clicked() {
                app.files.subpath = if app.files.subpath.is_empty() {
                    d.clone()
                } else {
                    format!("{}/{}", app.files.subpath, d)
                };
                app.files.filter.clear();
                load(&mut app.files);
            }
        }
        for f in files {
            if ui.selectable_label(app.files.peek_name == f, format!("📄 {f}")).clicked() {
                let root = cockpit::ops::repo_root();
                let path = if app.files.subpath.is_empty() {
                    root.join(&f)
                } else {
                    root.join(&app.files.subpath).join(&f)
                };
                app.files.peek_name = f.clone();
                app.files.peek_content = std::fs::read_to_string(&path)
                    .map(|s| s.chars().take(20000).collect())
                    .unwrap_or_else(|e| format!("(cannot read: {e})"));
            }
        }
        if hidden > 0 {
            ui.label(egui::RichText::new(format!("… {hidden} hidden by filter")).weak().small());
        }
    });

    if !app.files.peek_name.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new(&app.files.peek_name).strong());
        uikit_egui::scroll::bounded(ui, 260.0, |ui| {
            ui.label(egui::RichText::new(&app.files.peek_content).monospace().small());
        });
    }
}
