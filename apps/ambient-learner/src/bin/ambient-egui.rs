//! The runnable egui frontend.
//!
//! The `egui-app` feature existed with no binary behind it, and the manifest
//! comment beside it already said what that meant: *"a library nothing launches
//! is not a port anyone can use."* This is the launcher.
//!
//! Deliberately thin. Everything is on `App`, which holds the `Shell`, which
//! owns the page state and dispatches to the ported renderers. If this file
//! grows logic, that logic is in the wrong place.
//!
//! Run: `cargo run --no-default-features --features egui-app --bin ambient-egui`
//!
//! `--no-default-features` is not optional: the crate defaults to `gtk`, so a
//! plain run pulls gtk4, libadwaita and pango and fails on any machine without
//! the GTK toolchain — including the phones this fleet targets.

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: eframe::egui::ViewportBuilder::default()
            // Sized for the narrow case first. This fleet targets postmarketOS
            // handsets, and a window that opens at desktop width and is then
            // squeezed hides every layout bug that only appears when compact.
            .with_inner_size([420.0, 780.0])
            .with_min_inner_size([320.0, 480.0])
            .with_title("ambient"),
        ..Default::default()
    };
    eframe::run_native(
        "ambient",
        opts,
        Box::new(|_cc| Ok(Box::new(Host::default()))),
    )
}

#[derive(Default)]
struct Host {
    app: ambient_learner_egui::App,
}

impl eframe::App for Host {
    // `ui`, not `update`: eframe 0.34 makes it the required method and hands
    // the panel in. snapshot-cam hit the same thing at d93dc6a8 — it was the
    // last app still on the old shape, and this binary was written after it
    // and repeated the mistake because there was no compiled example to copy.
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _f: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.app.ui(ui);

        // ACTIONS ARE DRAINED EVERY FRAME, even though nothing applies them
        // yet. Leaving them to accumulate would grow the vector for the
        // lifetime of the process and, worse, would make the first host that
        // does read it replay every button ever pressed.
        for a in self.app.take_actions() {
            // No storage layer on this path yet: the GTK build owns the
            // database and this frontend does not link it. Logged rather than
            // silently dropped, so a press that does nothing says so.
            eprintln!("ambient: unhandled {a:?} — the egui frontend has no storage layer yet");
        }

        // Repaint only while something is moving. An ambient display that
        // spins the GPU at 60fps to show a temperature that changes once a
        // minute is a battery bug on a handset.
        if ctx.input(|i| i.pointer.has_pointer()) {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
