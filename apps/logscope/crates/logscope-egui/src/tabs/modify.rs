//! Modify tab — clipboard-history driven edit/transform pipeline with a
//! Replace/Append/Patch paste mode. Ports `ui/modify_tab.rs`.
use crate::theme;
use egui::RichText;

#[derive(Clone, Copy, PartialEq)]
enum PasteMode { Replace, Append, Patch }
impl PasteMode { fn label(self) -> &'static str { match self { PasteMode::Replace=>"Replace", PasteMode::Append=>"Append", PasteMode::Patch=>"Patch" } } }

pub struct Modify { history: Vec<String>, sel: usize, mode: PasteMode, target: String }
impl Default for Modify {
    fn default() -> Self {
        Self {
            history: vec![
                "fn prefetch_ahead(&mut self, ..) { /* … */ }".into(),
                "UNENDLICH_PREFETCH=1".into(),
                "diff --git a/src/lib.rs b/src/lib.rs".into(),
            ],
            sel: 0, mode: PasteMode::Replace,
            target: "// paste target buffer\n".into(),
        }
    }
}
impl Modify {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Paste mode:");
            for m in [PasteMode::Replace, PasteMode::Append, PasteMode::Patch] {
                ui.selectable_value(&mut self.mode, m, m.label());
            }
            ui.separator();
            if ui.button("◀ Prev").on_hover_text("Previous clipboard entry (..p)").clicked() && self.sel > 0 { self.sel -= 1; }
            if ui.button("Next ▶").on_hover_text("Next clipboard entry (..n)").clicked() && self.sel + 1 < self.history.len() { self.sel += 1; }
            let _ = ui.button("Refresh clipboard history");
        });
        ui.separator();
        ui.columns(2, |c| {
            c[0].label(RichText::new("Clipboard history").color(theme::DIM()).small());
            egui::ScrollArea::vertical().id_salt("mod_hist").show(&mut c[0], |ui| {
                for (i, h) in self.history.iter().enumerate() {
                    let first = h.lines().next().unwrap_or(h);
                    ui.selectable_value(&mut self.sel, i, RichText::new(first).monospace());
                }
            });
            c[1].label(RichText::new("Target").color(theme::DIM()).small());
            egui::ScrollArea::vertical().id_salt("mod_target").show(&mut c[1], |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.target)
                    .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(8));
            });
        });
        ui.add_space(6.0);
        if ui.button("✦ Paste").clicked() {
            if let Some(entry) = self.history.get(self.sel) {
                match self.mode {
                    PasteMode::Replace => self.target = entry.clone(),
                    PasteMode::Append => { self.target.push('\n'); self.target.push_str(entry); }
                    PasteMode::Patch => { self.target.push_str("\n# (patch applied)\n"); self.target.push_str(entry); }
                }
            }
        }
    }
}
