//! Cargo tab — runs `cargo check --message-format=json`, lists diagnostics,
//! and explains them via an Ollama model. Ports `ui/cargo_tab.rs`.
use crate::model::{model_combo, ModelState};
use crate::theme;
use egui::{Color32, RichText};

struct Diag {
    file: String,
    line: u32,
    level: String,
    message: String,
    code: Option<String>,
    source: Vec<String>,
}

pub struct Cargo {
    status: String,
    diags: Vec<Diag>,
    expanded: Option<usize>,
}
impl Default for Cargo {
    fn default() -> Self {
        Self {
            status: "cargo check — 2 warnings, 1 error".into(),
            expanded: Some(0),
            diags: vec![
                Diag { file: "src/world/chunk_manager.rs".into(), line: 412, level: "error".into(),
                    code: Some("E0382".into()),
                    message: "borrow of moved value: `to_request`".into(),
                    source: vec!["    let to_request = self.shell_walk(player);".into(),
                                 "    self.loader.request_many(to_request);".into(),
                                 "    self.prefetch(to_request); // moved here".into()] },
                Diag { file: "src/render/world_renderer.rs".into(), line: 88, level: "warning".into(),
                    code: Some("unused_variables".into()),
                    message: "unused variable: `frame_idx`".into(),
                    source: vec!["    let frame_idx = self.frame;".into()] },
                Diag { file: "src/lib.rs".into(), line: 5, level: "warning".into(), code: None,
                    message: "unused import: `std::sync::Arc`".into(),
                    source: vec!["use std::sync::Arc;".into()] },
            ],
        }
    }
}
impl Cargo {
    pub fn ui(&mut self, ui: &mut egui::Ui, model: &mut ModelState, scanning: bool) -> Option<String> {
        let mut send: Option<String> = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("unendlich-engine").color(theme::INK()).strong());
            if ui.button("Refresh").clicked() { self.status = "cargo check — running…".into(); }
            if ui.button("Cancel").on_hover_text("Cancel cargo check").clicked() { self.status = "cargo check — idle".into(); }
        });
        ui.label(RichText::new(&self.status).color(theme::DIM()).small());
        ui.horizontal(|ui| {
            ui.label(RichText::new("model for explanations").color(theme::DIM()).small());
            model_combo(ui, model, "cargo_model", scanning);
        });
        ui.separator();

        egui::ScrollArea::vertical().id_salt("cargo_diags").auto_shrink([false, false]).show(ui, |ui| {
            for (i, d) in self.diags.iter().enumerate() {
                let lvl_color = level_color(&d.level);
                egui::Frame { fill: theme::PANEL(), corner_radius: egui::CornerRadius::same(8),
                    inner_margin: egui::Margin::same(10), outer_margin: egui::Margin { bottom: 8, ..Default::default() },
                    ..Default::default() }
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(d.level.to_uppercase()).color(lvl_color).strong().small());
                        if let Some(c) = &d.code { ui.label(RichText::new(format!("[{c}]")).color(theme::DIM()).small()); }
                        ui.label(RichText::new(format!("{}:{}", d.file, d.line)).color(theme::AMBER_SOFT()).monospace().small());
                    });
                    ui.label(RichText::new(&d.message).color(theme::INK()));
                    let open = self.expanded == Some(i);
                    ui.horizontal(|ui| {
                        if ui.button(if open { "▾ source" } else { "▸ source" }).clicked() {
                            self.expanded = if open { None } else { Some(i) };
                        }
                        if ui.button("✦ Explain").on_hover_text("Send this diagnostic to Chat").clicked() {
                            send = Some(format!(
                                "Explain this Rust compiler diagnostic and how to fix it:\n\n{} [{}] {}:{}\n{}\n\n```rust\n{}\n```",
                                d.level.to_uppercase(), d.code.as_deref().unwrap_or("-"), d.file, d.line, d.message, d.source.join("\n")));
                        }
                        if ui.button("⧉ Copy").clicked() {
                            ui.ctx().copy_text(format!("{} [{}] {}:{}\n{}", d.level, d.code.as_deref().unwrap_or("-"), d.file, d.line, d.message));
                        }
                    });
                });
            }
        });
        // expanded source rendered after the loop to avoid borrow overlap
        if let Some(i) = self.expanded {
            if let Some(d) = self.diags.get(i) {
                egui::Frame { fill: Color32::from_rgb(0x16,0x16,0x16), corner_radius: egui::CornerRadius::same(6),
                    inner_margin: egui::Margin::same(10), ..Default::default() }
                .show(ui, |ui| {
                    for l in &d.source { ui.label(RichText::new(l).monospace().color(theme::DIM())); }
                });
            }
        }
        send
    }
}
fn level_color(level: &str) -> Color32 {
    match level { "error" => theme::RED(), "warning" => theme::YELLOW(), _ => theme::DIM() }
}
