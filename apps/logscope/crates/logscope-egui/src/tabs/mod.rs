//! Tab registry. Each tab is a self-contained module with its own state struct
//! and an immediate-mode `ui()`. The non-UI logic (engine, providers, bloc,
//! conversation_db) is reused as-is in the real port; here each panel carries
//! representative state so the shell is alive and faithful to `src/ui/*`.
pub mod sources;
pub mod peek;
pub mod rotate;
pub mod cargo;
pub mod charts;
pub mod chat;
pub mod code;
pub mod modify;
pub mod bloc;
pub mod models;

use crate::model::ModelState;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab { Sources, Peek, Tokens, Blocks, Rotate, Cargo, Chat, Code, Modify, Bloc, Models }

impl Tab {
    pub const ALL: [Tab; 11] = [
        Tab::Sources, Tab::Peek, Tab::Tokens, Tab::Blocks, Tab::Rotate,
        Tab::Cargo, Tab::Chat, Tab::Code, Tab::Modify, Tab::Bloc, Tab::Models,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Sources => "Sources", Tab::Peek => "Peek", Tab::Tokens => "Tokens",
            Tab::Blocks => "Blocks", Tab::Rotate => "Rotate", Tab::Cargo => "Cargo",
            Tab::Chat => "Chat", Tab::Code => "Code", Tab::Modify => "Modify", Tab::Bloc => "Bloc",
            Tab::Models => "Models",
        }
    }
}

#[derive(Default)]
pub struct Tabs {
    pub sources: sources::Sources,
    pub peek: peek::Peek,
    pub rotate: rotate::Rotate,
    pub cargo: cargo::Cargo,
    pub chat: chat::Chat,
    pub code: code::Code,
    pub modify: modify::Modify,
    pub bloc: bloc::Bloc,
    pub models: models::Models,
}

impl Tabs {
    pub fn ui(&mut self, tab: Tab, ui: &mut egui::Ui, model: &mut ModelState,
              doc: &mut crate::analysis::Doc, scanning: bool,
              infer: &crate::backend::Inference, model_loaded: bool) -> Option<String> {
        match tab {
            Tab::Sources => self.sources.ui(ui, doc),
            Tab::Peek => { self.peek.ui(ui, doc); None }
            Tab::Tokens => { charts::tokens_ui(ui, doc); None }
            Tab::Blocks => { charts::blocks_ui(ui, doc); None }
            Tab::Rotate => { self.rotate.ui(ui); None }
            Tab::Cargo => self.cargo.ui(ui, model, scanning),
            Tab::Chat => { self.chat.ui(ui, model, doc, infer, model_loaded); None }
            Tab::Code => self.code.ui(ui),
            Tab::Modify => { self.modify.ui(ui); None }
            Tab::Bloc => self.bloc.ui(ui, model, scanning),
            Tab::Models => { self.models.ui(ui); None }
        }
    }
    /// Route a composed prompt from another tab into the Chat input.
    pub fn route_to_chat(&mut self, prompt: String) { self.chat.set_input(prompt); }
    pub fn apply_chat_evt(&mut self, e: crate::backend::Evt) { self.chat.apply_evt(e); }
}
