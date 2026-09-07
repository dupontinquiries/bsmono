//! GUI entry point. All modules live in the library (lib.rs); this binary just launches eframe.
use logscope_egui::app::LogscopeApp;
use logscope_egui::theme;

fn main() -> eframe::Result {
    // Why the window will not open, said before the panic in a dependency does.
    // `uikit_egui::backend_check`: the usual cause is a static musl build, which cannot
    // dlopen libwayland-client, and wayland-sys's own message points at a package that is
    // already installed. Silent when the setup is fine.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
    // Install the diagnostic writer FIRST, so a crash during setup still produces a report — the runs
    // worth reporting are exactly the ones that do not finish cleanly. Held for the whole of main.
    let _diag = logscope_egui::diag::install();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("Logscope")
            .with_transparent(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "logscope",
        options,
        Box::new(|cc| {
            // PRECEDENCE: the env var overrides the saved space, which overrides the built-in
            // default. An explicit LOGSCOPE_THEME on the command line is a deliberate one-off and
            // must not be quietly outranked by a preference file — nor should it overwrite it.
            let want = std::env::var("LOGSCOPE_THEME").ok()
                .or_else(|| logscope_egui::prefs::load().space);
            match want.and_then(|name| logscope_egui::roomtheme::spaces().into_iter()
                    .find(|p| p.name.eq_ignore_ascii_case(&name)))
            {
                Some(p) => theme::install_palette(&cc.egui_ctx, p),
                None => theme::install(&cc.egui_ctx),
            }
            // SWEEP STALE SCRATCH DIRECTORIES. Every isolated workspace a session makes lands in
            // /tmp and, until now, stayed there: the eviction policy in `ws_registry` was written,
            // tested and never called, which the repo's own testonly_check reports. On a machine
            // where an agent runs daily that is the disk filling, and disk is the resource that
            // actually runs out. Seven days, 8 GiB; best-effort, and a failure to tidy never stops
            // a session starting.
            {
                let day_ms = 24 * 60 * 60 * 1000u64;
                let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64).unwrap_or(0);
                for root in ["logscope", "logscope-fan"] {
                    let dir = std::env::temp_dir().join(root);
                    let gone = logscope_egui::ws_registry::WsRegistry::sweep(
                        &dir, now_ms, 7 * day_ms, 8 * 1024 * 1024 * 1024);
                    if !gone.is_empty() {
                        eprintln!("logscope: swept {} stale workspace(s) from {}", gone.len(), dir.display());
                    }
                }
            }
            Ok(Box::new(LogscopeApp::default()))
        }),
    )
}
