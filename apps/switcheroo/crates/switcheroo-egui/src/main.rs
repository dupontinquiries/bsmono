//! switcheroo — tap a Bluetooth outlet to turn it on and off.

fn main() -> eframe::Result<()> {
    // Why the window will not open, said before the panic in a dependency does.
    // `uikit_egui::backend_check`: the usual cause is a static musl build, which cannot
    // dlopen libwayland-client, and wayland-sys's own message points at a package that is
    // already installed. Silent when the setup is fine.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
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
    // This fleet's eframe requires `ui` ALONGSIDE `update`. The app is driven
    // from update() below, as every other egui frontend here does it, so this
    // is deliberately empty rather than a todo!().
    //
    // I wrote this crate from eframe's documented API without compiling it —
    // said so at the time — and got this wrong. dialer-egui carries the same
    // comment for the same reason; the convention was already established and
    // I did not look. Checking one working sibling would have cost nothing.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        switcheroo_egui::ui::draw(&mut self.app, ctx);
    }
}
