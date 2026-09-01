//! switcheroo — tap a Bluetooth outlet to turn it on and off.

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // Portrait-ish default: this is a phone app that also runs on a
            // desktop, not the other way round.
            .with_inner_size([420.0, 760.0])
            .with_min_inner_size([320.0, 420.0]),
        ..Default::default()
    };
    let mut app = switcheroo_egui::App::new();
    // Scan on launch. Opening a Bluetooth app and then having to press
    // "scan" is a step nobody wants; the button stays for re-scanning.
    app.start_scan();
    eframe::run_native(
        "switcheroo",
        options,
        Box::new(|_cc| Ok(Box::new(Shell { app }))),
    )
}

struct Shell {
    app: switcheroo_egui::App,
}

impl eframe::App for Shell {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        switcheroo_egui::ui::draw(&mut self.app, ctx);
    }
}
