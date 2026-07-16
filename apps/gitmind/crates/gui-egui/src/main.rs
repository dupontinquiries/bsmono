#[cfg(feature = "app")]
fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "gitmind",
        opts,
        Box::new(|_cc| {
            let mut app = GitmindApp::default();
            // GITMIND_EGUI_TAB overrides the default starting tab — for automated screenshots/
            // testing (no UI-driving harness exists to click a tab button), not a user-facing
            // setting; unset, behaviour is identical to before.
            if let Ok(name) = std::env::var("GITMIND_EGUI_TAB") {
                if let Some(t) = gitmind_egui::app::Tab::ALL.iter().find(|t| t.label().eq_ignore_ascii_case(&name)) {
                    app.app.tab = *t;
                }
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(not(feature = "app"))]
fn main() {
    eprintln!("gitmind-egui: build with --features app for the eframe window (the lib builds headless)");
}

#[cfg(feature = "app")]
#[derive(Default)]
struct GitmindApp {
    app: gitmind_egui::app::App,
}

#[cfg(feature = "app")]
impl eframe::App for GitmindApp {
    // This eframe (0.34) makes `ui(&mut Ui, ..)` the required method and defaults/deprecates
    // `update(&Context, ..)`. gitmind's UI owns the egui Context — `app.ui(ctx)` builds its own
    // SidePanel + CentralPanel — so it must be driven at the Context level, not from inside eframe's
    // pre-wrapped Ui (which would nest panels). The runner still calls `update` every frame, so we
    // keep driving from there and leave the required `ui` hook empty.
    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.ui(ctx);
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
}
