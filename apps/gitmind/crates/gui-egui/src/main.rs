#[cfg(feature = "app")]
fn main() -> eframe::Result<()> {
    // RENDERER PINNED, and limits clamped if wgpu is what we get.
    //
    // On an Asahi host this died at startup with
    //   LimitsExceeded(FailedLimit { name: "max_color_attachments",
    //                                requested: 8, allowed: 5 })
    // `NativeOptions::default()` leaves the renderer to eframe, and when
    // egui-wgpu is linked eframe prefers it — then asks for
    // `wgpu::Limits::default()`, which wants 8 colour attachments and
    // never clamps to what the adapter reports.
    //
    // This crate declares `features = ["glow", "wayland", "x11"]` and no
    // wgpu, so the wgpu path arrived through feature unification with
    // something else in the build. Saying `Glow` explicitly means the
    // choice no longer depends on what else happens to be compiled in —
    // which is the actual bug, not the limit.
    //
    // snapclone's camera-bridge hit exactly this and its comment records
    // the same crash on Asahi; the difference is that crate WANTS wgpu
    // and so clamps the limits, while this one only ever wanted glow.
    let opts = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "gitmind",
        opts,
        Box::new(|cc| {
            // Without this, egui::Image silently draws nothing for every
            // file:// URI — no panic, no log, just a blank where the preview
            // tile should be.
            egui_extras::install_image_loaders(&cc.egui_ctx);
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
