//! Branches page — the gitmind native store's branches (read-only list). Opens the repo's chunk
//! store and lists branches; HEAD is marked.
use crate::app::App;
use gitmind_core::store::ChunkStore;

#[derive(Default)]
pub struct State {
    pub loaded: bool,
    pub branches: Vec<(String, String, bool)>, // (name, oid, is_head)
    pub err: String,
}

fn open_store() -> Result<ChunkStore, String> {
    let root = cockpit::ops::repo_root();
    let dir = gitmind_core::portage::native_store_dir(&root);
    std::fs::create_dir_all(&dir).map_err(|_| "cannot create .gitmind store dir".to_string())?;
    ChunkStore::open(dir.join("store.redb"))
}

fn load(state: &mut State) {
    state.loaded = true;
    state.err.clear();
    match open_store() {
        Ok(store) => match gitmind_core::native::branch_list(&store) {
            Ok(bs) => {
                state.branches = bs.iter().map(|b| (b.name.clone(), b.oid.clone(), b.is_head)).collect();
            }
            Err(e) => state.err = e,
        },
        Err(e) => state.err = e,
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Branches");
    ui.label(egui::RichText::new("gitmind native-store branches (● = HEAD)").weak().small());
    ui.separator();

    if ui.button("↻ refresh").clicked() || !app.branches.loaded {
        load(&mut app.branches);
    }
    if !app.branches.err.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(0xf7, 0x76, 0x8e), &app.branches.err);
    }
    if app.branches.branches.is_empty() && app.branches.err.is_empty() {
        ui.label(egui::RichText::new("no branches in the native store yet").weak());
    }
    egui::ScrollArea::vertical().id_salt("branches").show(ui, |ui| {
        for (name, oid, head) in &app.branches.branches {
            ui.horizontal(|ui| {
                ui.label(if *head { "●" } else { " " });
                ui.label(egui::RichText::new(name).strong());
                let short = &oid[..oid.len().min(8)];
                ui.label(egui::RichText::new(short).monospace().weak());
            });
        }
    });
}
