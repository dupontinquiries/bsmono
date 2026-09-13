//! photosync — a camera roll over the daemon's library, on the shared kit.

fn main() -> eframe::Result<()> {
    // Same preflight every -egui app in this tree runs: the usual failure
    // mode is a static musl build that can't dlopen libwayland-client, and
    // wayland-sys's own error points at a package that's already installed.
    // Silent when the setup is fine.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([360.0, 480.0]),
        ..Default::default()
    };
    let app = photosync_egui::App::demo();
    eframe::run_native(
        "photosync",
        options,
        Box::new(|_cc| Ok(Box::new(Shell { app }))),
    )
}

struct Shell {
    app: photosync_egui::App,
}

impl eframe::App for Shell {
    // This fleet's eframe wants `ui` alongside `update`; the app is driven
    // from `update` below like every other egui frontend here, so this is
    // deliberately empty rather than a todo!().
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        photosync_egui::ui::draw(&mut self.app, ctx);
    }
}
