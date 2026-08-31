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

    /// What the tab is FOR, in one line.
    ///
    /// Every nav item in the reference panels carries a subtitle under its name — "GRAPH / Explore
    /// connections", "CONTEXT / Files & references" — and ours are bare words. Several of them are
    /// words that cannot be guessed: "Peek", "Rotate" and "Bloc" name internal concepts, and
    /// "Rotate" in particular reads as a view control when it configures logrotate.
    ///
    /// Taken from each module's own doc comment rather than invented, so a tab that changes what it
    /// does has one place to correct and the description cannot quietly become fiction.
    ///
    /// EXHAUSTIVE MATCH ON PURPOSE. A new tab will not compile until it has one, which is the only
    /// way a list like this stays complete — the alternative is a lookup returning Option and
    /// eleven of twelve tabs described.
    pub fn purpose(self) -> &'static str {
        match self {
            Tab::Sources => "Load a log and filter it",
            Tab::Peek    => "Read the log, folding repeated lines",
            Tab::Tokens  => "What the log is made of, counted",
            Tab::Blocks  => "Recurring shapes in the log",
            Tab::Rotate  => "Generate a logrotate config",
            Tab::Cargo   => "Run cargo check and work the diagnostics",
            Tab::Chat    => "Ask a model, in the mind palace",
            Tab::Code    => "Read code with block actions",
            Tab::Modify  => "Edit and transform from clipboard history",
            Tab::Bloc    => "AST-scored regions and suggestions",
            Tab::Models  => "The model library, with status",
        }
    }
}

#[cfg(test)]
mod purpose_tests {
    use super::Tab;

    /// A subtitle that repeats the name teaches nothing — "Rotate: rotation options" is the failure
    /// mode here, and the tabs whose names are opaque are exactly the ones at risk of it.
    #[test]
    fn no_purpose_merely_restates_the_label() {
        for t in Tab::ALL {
            let (l, p) = (t.label().to_lowercase(), t.purpose().to_lowercase());
            assert!(!p.starts_with(&l), "{}: '{}' opens by repeating its own name", t.label(), t.purpose());
            assert!(p.split_whitespace().count() >= 3, "{}: '{}' is not a sentence", t.label(), t.purpose());
        }
    }

    /// Two tabs described identically is two tabs the reader still cannot tell apart.
    #[test]
    fn every_tab_is_described_distinctly() {
        let mut seen: Vec<&str> = Vec::new();
        for t in Tab::ALL {
            let p = t.purpose();
            assert!(!p.is_empty(), "{} has no purpose", t.label());
            assert!(!seen.contains(&p), "{} reuses another tab's description", t.label());
            seen.push(p);
        }
        assert_eq!(seen.len(), Tab::ALL.len());
    }

    /// These sit under a tab in a bar that already competes for width, and on a phone.
    #[test]
    fn a_purpose_fits_beside_a_tab() {
        for t in Tab::ALL {
            let n = t.purpose().chars().count();
            assert!((12..=44).contains(&n), "{}: {n} chars — '{}'", t.label(), t.purpose());
        }
    }

    /// ELEVEN TABS ON A PHONE. The hover text above is invisible on a touch screen, so the labels
    /// carry the whole burden there — and this ships to postmarketOS, PinePhone and Fairphone. If
    /// the bar overflows at 360 logical pixels the labels are not merely unexplained, they are
    /// unreachable.
    ///
    /// Measured with `Context::run`, which lays out on the CPU: no GPU, no window.
    #[test]
    fn the_tab_bar_fits_a_phone_by_wrapping() {
        let row = |w: f32| -> (f32, f32) {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 900.0)));
            let mut used = egui::Vec2::ZERO;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = ui.horizontal_wrapped(|ui| {
                        let mut sel = Tab::Chat;
                        for t in Tab::ALL { let _ = ui.selectable_value(&mut sel, t, t.label()); }
                    });
                    used = r.response.rect.size();
                });
            });
            (used.x, used.y)
        };
        let (w360, h360) = row(360.0);
        assert!(w360 <= 360.5, "eleven tabs used {w360:.1}px of 360px");
        let (_, h1400) = row(1400.0);
        assert!(h360 > h1400, "the phone row must wrap ({h360:.1}) rather than clip ({h1400:.1})");
        assert!(h360 < 300.0, "wrapping eleven tabs took {h360:.1}px, most of a phone screen");
    }

    /// The three names nobody can guess must be the three that say the most. `Rotate` is the worst
    /// of them: it reads as a view control and configures logrotate.
    #[test]
    fn the_opaque_names_are_explained() {
        assert!(Tab::Rotate.purpose().to_lowercase().contains("logrotate"),
                "Rotate reads as a view control: {}", Tab::Rotate.purpose());
        assert!(Tab::Peek.purpose().to_lowercase().contains("log"));
        assert!(Tab::Bloc.purpose().to_lowercase().contains("ast")
                || Tab::Bloc.purpose().to_lowercase().contains("region"));
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
