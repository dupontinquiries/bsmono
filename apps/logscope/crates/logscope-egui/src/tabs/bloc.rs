//! Bloc tab — the bloc engine's AST-scored regions, an editor, and AI
//! suggestions. Ports `ui/bloc_tab.rs`.
use crate::model::{model_combo, ModelState};
use crate::theme;
use egui::{Color32, RichText};

#[derive(Clone, Copy)]
#[allow(dead_code)] // full kind set from the real bloc engine
enum BlocKind { Code, Snippet, Template, Prose, Data, Unknown }
impl BlocKind {
    fn label(self) -> &'static str { match self { BlocKind::Code=>"Code", BlocKind::Snippet=>"Snippet", BlocKind::Template=>"Template", BlocKind::Prose=>"Prose", BlocKind::Data=>"Data", BlocKind::Unknown=>"Unknown" } }
    fn color(self) -> Color32 { match self { BlocKind::Code=>theme::BLUE(), BlocKind::Snippet=>theme::GREEN(), BlocKind::Template=>theme::PURPLE(), BlocKind::Prose=>theme::AMBER_SOFT(), BlocKind::Data=>theme::YELLOW(), BlocKind::Unknown=>theme::DIM() } }
}
struct BlocItem { name: String, kind: BlocKind, score: f32 }

pub struct Bloc { blocs: Vec<BlocItem>, sel: usize, editor: String, note: Option<(String, std::time::Instant)> }
impl Default for Bloc {
    fn default() -> Self {
        Self {
            sel: 0,
            note: None,
            editor: "// bloc: prefetch_ahead\nfn prefetch_ahead(&mut self) { /* … */ }".into(),
            blocs: vec![
                BlocItem { name: "prefetch_ahead".into(), kind: BlocKind::Code, score: 0.94 },
                BlocItem { name: "logrotate.tmpl".into(), kind: BlocKind::Template, score: 0.81 },
                BlocItem { name: "error-triage note".into(), kind: BlocKind::Prose, score: 0.66 },
                BlocItem { name: "token-freq.csv".into(), kind: BlocKind::Data, score: 0.52 },
            ],
        }
    }
}
impl Bloc {
    pub fn ui(&mut self, ui: &mut egui::Ui, model: &mut ModelState, scanning: bool) -> Option<String> {
        let mut send: Option<String> = None;
        ui.horizontal(|ui| {
            let _ = ui.button("Open").on_hover_text("Open source file");
            ui.label(RichText::new("Ollama model for AI suggestions").color(theme::DIM()).small());
            model_combo(ui, model, "bloc_model", scanning);
        });
        ui.separator();
        ui.columns(2, |c| {
            c[0].label(RichText::new("Blocs (AST-scored)").color(theme::DIM()).small());
            egui::ScrollArea::vertical().id_salt("bloc_list").show(&mut c[0], |ui| {
                for (i, b) in self.blocs.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.sel, i, RichText::new(&b.name).monospace());
                        ui.label(RichText::new(b.kind.label()).color(b.kind.color()).small());
                        ui.label(RichText::new(format!("{:.2}", b.score)).color(theme::DIM()).small());
                    });
                }
            });
            c[1].label(RichText::new("Editor").color(theme::DIM()).small());
            egui::ScrollArea::vertical().id_salt("bloc_editor").show(&mut c[1], |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.editor)
                    .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(8));
            });
        });
        ui.add_space(theme::PAD_SM as f32);
        ui.horizontal(|ui| {
            let _ = ui.button("Open").on_hover_text("Open source file");
            if theme::action_button(ui, "⧉ Copy bloc", theme::CYAN(), true).clicked() {
                ui.ctx().copy_text(self.editor.clone());
                self.note = Some(("copied bloc to clipboard".into(), std::time::Instant::now()));
            }
            let name = self.blocs.get(self.sel).map(|b| b.name.as_str()).unwrap_or("bloc");
            if theme::action_button(ui, "✦ Suggest", theme::PURPLE(), true).clicked() {
                send = Some(format!("Suggest concrete improvements to this “{name}” bloc:\n\n```\n{}\n```", self.editor));
                self.note = Some((format!("sent “{name}” to Chat"), std::time::Instant::now()));
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
