//! The scribe window: `scribe-egui [file]` (a `.fountain`, or any text/markdown file).

struct Host {
    editor: scribe::editor::Editor,
    ime_repair: uikit_egui::ime::ImeRepair,
}

impl eframe::App for Host {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| self.editor.ui(ui));
    }

    /// An on-screen keyboard commits each key as `Disabled, Commit, Disabled` with no composition, which egui's
    /// TextEdit drops after the first character; `ImeRepair` turns such a commit into plain text.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.ime_repair.repair(&mut raw_input.events);
    }
}

fn main() -> eframe::Result<()> {
    const APP_ID: &str = "org.scribe.Scribe";
    // No file named (a launcher entry, a phone): keep the work in ~/Documents/scribe/untitled.fountain, which is
    // reloaded next time, so typing is never lost.
    let path = std::env::args().nth(1).map(std::path::PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Documents").join("scribe").join("untitled.fountain"))
    });
    eframe::run_native(
        "scribe",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport: uikit_egui::desktop::viewport(APP_ID, "scribe", [980.0, 720.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            uikit_egui::fonts::install(&cc.egui_ctx);
            uikit_egui::ime::allow_ime(&cc.egui_ctx);
            Ok(Box::new(Host { editor: scribe::editor::Editor::open(path), ime_repair: Default::default() }))
        }),
    )
}
