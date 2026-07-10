//! model_picker — egui port of `qml/ModelPicker.qml` (core).
//!
//! A local llama.cpp GGUF browser/activator: rows for each candidate model — already-downloaded ones
//! first, the recommended quant starred — with a per-row action (Download an available file, or Use a
//! downloaded one), plus a download-progress readout. The QML drove this through backend hooks
//! (`download_model`, `download_progress_json`, `local_models_json`, `set_llama_model_path`); this port
//! renders the list + progress and returns the chosen [`ModelAction`] for the host to dispatch. Search
//! over Hugging Face and the actual download/activation stay in the backend.
//!
//! Row ordering is pure and unit-tested.

use egui::{Color32, RichText};

/// One selectable model.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModelRow {
    /// GGUF file name (the identity used for download/activate).
    pub name: String,
    /// Human size, e.g. "4.1 GB" (empty if unknown).
    pub size_label: String,
    /// Optional source repo (for downloads).
    pub repo: String,
    /// Already on disk.
    pub downloaded: bool,
    /// The suggested quant for this machine.
    pub recommended: bool,
    /// Currently the active model.
    pub active: bool,
}

/// What the user asked to do with a row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ModelAction {
    /// Download `file` from `repo`.
    Download { repo: String, file: String },
    /// Activate an already-downloaded model by file name.
    Use(String),
}

/// Sort rows for display: downloaded first, then recommended, then by name. Pure — the tested core.
pub fn sorted_rows(rows: &[ModelRow]) -> Vec<ModelRow> {
    let mut v = rows.to_vec();
    v.sort_by(|a, b| {
        b.downloaded
            .cmp(&a.downloaded)
            .then(b.recommended.cmp(&a.recommended))
            .then(a.name.cmp(&b.name))
    });
    v
}

/// Browser state.
#[derive(Default)]
pub struct ModelPicker {
    pub rows: Vec<ModelRow>,
    /// Free-text status from the backend ("", "downloading", "done", "error: …").
    pub download_status: String,
    /// 0..=100 while a download is in flight.
    pub download_percent: u8,
}

impl ModelPicker {
    /// Render the picker; returns a [`ModelAction`] on the frame a row's button is clicked.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<ModelAction> {
        let mut action = None;

        if !self.download_status.is_empty() {
            let downloading = self.download_status == "downloading" || self.download_status.starts_with("fetch");
            if downloading {
                ui.add(egui::ProgressBar::new(self.download_percent as f32 / 100.0).text(format!("{}%", self.download_percent)));
            } else {
                let err = self.download_status.starts_with("error");
                let col = if err { Color32::from_rgb(189, 90, 68) } else { ui.visuals().weak_text_color() };
                ui.colored_label(col, &self.download_status);
            }
            ui.add_space(6.0);
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for m in sorted_rows(&self.rows) {
                ui.horizontal(|ui| {
                    // star for the recommended quant
                    ui.label(if m.recommended { RichText::new("★").color(Color32::from_rgb(201, 185, 154)) } else { RichText::new("  ") });
                    // name + size + active/downloaded badge
                    let mut name = RichText::new(&m.name);
                    if m.active {
                        name = name.strong().color(ui.visuals().selection.bg_fill);
                    }
                    ui.label(name);
                    if !m.size_label.is_empty() {
                        ui.label(RichText::new(&m.size_label).weak().small());
                    }
                    // right-aligned action
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if m.active {
                            ui.label(RichText::new("active").small().color(ui.visuals().selection.bg_fill));
                        } else if m.downloaded {
                            if ui.button("Use").clicked() {
                                action = Some(ModelAction::Use(m.name.clone()));
                            }
                        } else if ui.button("Download").clicked() {
                            action = Some(ModelAction::Download { repo: m.repo.clone(), file: m.name.clone() });
                        }
                    });
                });
                ui.separator();
            }
            if self.rows.is_empty() {
                ui.weak_label_compat("no models — search a repo or drop a .gguf in the models folder");
            }
        });

        action
    }
}

// small compat shim so the "empty" hint reads clearly regardless of egui label helpers
trait WeakLabel {
    fn weak_label_compat(&mut self, text: &str);
}
impl WeakLabel for egui::Ui {
    fn weak_label_compat(&mut self, text: &str) {
        self.label(RichText::new(text).weak());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, downloaded: bool, recommended: bool) -> ModelRow {
        ModelRow {
            name: name.into(),
            size_label: String::new(),
            repo: String::new(),
            downloaded,
            recommended,
            active: false,
        }
    }

    #[test]
    fn downloaded_first_then_recommended_then_name() {
        let rows = vec![
            row("zeta", false, false),
            row("alpha", false, true),   // recommended, not downloaded
            row("beta", true, false),    // downloaded
            row("gamma", true, true),    // downloaded + recommended → very top
        ];
        let sorted = sorted_rows(&rows);
        let order: Vec<&str> = sorted.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(order, ["gamma", "beta", "alpha", "zeta"]);
    }

    #[test]
    fn empty_is_empty() {
        assert!(sorted_rows(&[]).is_empty());
    }
}
