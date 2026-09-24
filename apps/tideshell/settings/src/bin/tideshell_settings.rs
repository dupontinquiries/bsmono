//! The window around tideshell_settings::App, on eframe 0.34's `App::ui`
//! (the root Ui), not the deprecated `update`.

struct Win {
    app: tideshell_settings::App,
    touch_applied: bool,
}

impl eframe::App for Win {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        uikit_egui::fonts::install(ctx);
        if !self.touch_applied {
            // Finger-sized hit areas for the detected device class; once,
            // before anything is laid out (uikit's own contract).
            uikit_egui::adaptive::Adaptive::detect(ctx).apply_touch_defaults(ctx);
            self.touch_applied = true;
        }
        for a in self.app.ui(ui) {
            eprintln!("tideshell-settings: {a:?} -> {}", self.app.status);
        }
    }
}

fn main() -> eframe::Result<()> {
    let Some(store) = cosmic_levers::Store::from_env() else {
        eprintln!("tideshell-settings: no $XDG_CONFIG_HOME or $HOME");
        std::process::exit(1);
    };
    let opts = eframe::NativeOptions {
        // Glow, said explicitly: eframe 0.34's default renderer is wgpu,
        // which asks for more colour attachments than the FP5's GPU allows
        // (the reason bsettings says so too).
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default().with_title("Settings").with_app_id("tideshell-settings"),
        ..Default::default()
    };
    eframe::run_native("tideshell-settings", opts, Box::new(|_cc| Ok(Box::new(Win { app: tideshell_settings::App::new(store), touch_applied: false }))))
}
