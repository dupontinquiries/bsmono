//! Code tab — syntax-aware code view with AI block actions. Ports
//! `ui/code_tab.rs` (tree-sitter bloc detection + Ollama actions). Each action
//! composes a focused prompt for the selected block and copies it to the clipboard,
//! ready to paste into the Chat tab (or any model).
use crate::theme;
use egui::RichText;
use std::time::Instant;

const ACTIONS: &[&str] = &["Explain", "Refactor", "Generate tests", "Add doc comment",
    "Predict next block", "Suggest improvements"];

fn task_line(action: &str) -> &'static str {
    match action {
        "Explain" => "Explain what the following code does, step by step.",
        "Refactor" => "Refactor the following code for clarity and idiomatic style, and explain the changes.",
        "Generate tests" => "Write thorough unit tests for the following code.",
        "Add doc comment" => "Add a concise doc comment to the following code and return the documented version.",
        "Predict next block" => "Given the following code, predict and write the next logical block.",
        "Suggest improvements" => "Suggest concrete improvements to the following code.",
        _ => "Review the following code.",
    }
}

fn prompt_for(action: &str, file: &str, code: &str) -> String {
    format!("{}\n\n// file: {file}\n```rust\n{code}\n```", task_line(action))
}

pub struct Code { files: Vec<String>, sel: usize, code: String, note: Option<(String, Instant)> }
impl Default for Code {
    fn default() -> Self {
        Self {
            files: vec!["chunk_manager.rs".into(), "world_renderer.rs".into(), "prefetch_predict.rs".into()],
            sel: 0,
            note: None,
            code: "pub fn update(&mut self, px: f32, py: f32, pz: f32) {\n    self.drain();\n    self.evict();\n    let to_request = self.request_visible();\n    if prefetch_enabled() {\n        self.prefetch_ahead(px, py, pz, &to_request);\n    }\n}".into(),
        }
    }
}
impl Code {
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let mut send: Option<String> = None;
        ui.horizontal(|ui| {
            let _ = ui.button("Scan project files").on_hover_text("populate the list from a project directory");
            ui.label(RichText::new("tree-sitter blocs").color(theme::DIM()).small());
        });
        ui.separator();
        let file = self.files.get(self.sel).cloned().unwrap_or_default();
        ui.columns(2, |c| {
            c[0].label(RichText::new("Files").color(theme::DIM()).small());
            egui::ScrollArea::vertical().id_salt("code_files").show(&mut c[0], |ui| {
                for (i, f) in self.files.iter().enumerate() {
                    ui.selectable_value(&mut self.sel, i, RichText::new(f).monospace());
                }
            });
            c[1].label(RichText::new("Block").color(theme::DIM()).small());
            egui::ScrollArea::vertical().id_salt("code_view").show(&mut c[1], |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.code)
                    .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(10));
            });
        });
        ui.add_space(theme::PAD_SM as f32);
        ui.horizontal_wrapped(|ui| {
            for a in ACTIONS {
                if ui.button(*a).clicked() {
                    send = Some(prompt_for(a, &file, &self.code));
                    self.note = Some((format!("sent “{a}” to Chat"), Instant::now()));
                }
            }
            if theme::action_button(ui, "⧉ Copy block", theme::CYAN(), true).clicked() {
                ui.ctx().copy_text(self.code.clone());
                self.note = Some(("copied block to clipboard".into(), Instant::now()));
            }
        });
        if let Some((msg, at)) = &self.note {
            let alpha = 1.0 - theme::appear(*at, 2.5);
            if alpha <= 0.0 { self.note = None; }
            else {
                ui.ctx().request_repaint();
                theme::fade(ui, alpha, |ui| { ui.label(RichText::new(msg).color(theme::GREEN()).small()); });
            }
        }
        send
    }
}

#[cfg(test)]
mod tests {
    use super::prompt_for;
    #[test]
    fn prompt_embeds_task_file_and_code() {
        let p = prompt_for("Explain", "foo.rs", "fn x() {}");
        assert!(p.starts_with("Explain what the following code does"));
        assert!(p.contains("// file: foo.rs"));
        assert!(p.contains("fn x() {}"));
        assert!(p.contains("```rust"));
    }
}
