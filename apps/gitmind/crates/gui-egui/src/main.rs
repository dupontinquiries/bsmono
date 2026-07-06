#[cfg(feature = "app")]
fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "gitmind",
        opts,
        Box::new(|_cc| Ok(Box::new(GitmindApp::default()))),
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.ui(ctx);
    }
}
