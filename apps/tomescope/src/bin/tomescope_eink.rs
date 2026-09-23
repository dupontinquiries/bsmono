//! tomescope on an e-ink tablet: the same `TomeApp` core as the desktop
//! window, driven by eink-kit (CPU raster, refresh policy, touch) instead of
//! eframe. Launched by AppLoad from xochitl's sidebar, or by gitmind over SSH.
//!
//! Persistence: eframe calls `save` periodically and on exit. Here the app
//! is saved every `SAVE_EVERY` of wall time while in use, and on drop (the
//! runner returns, dropping the app, when the display server goes away). A
//! process killed outright loses at most the last interval.

use std::time::{Duration, Instant};
use tomescope::app::TomeApp;

const SAVE_EVERY: Duration = Duration::from_secs(60);

struct Shell {
    app: Option<TomeApp>,
    open: Option<std::path::PathBuf>,
    last_save: Instant,
}

impl eink_kit::EinkApp for Shell {
    fn ui(&mut self, ui: &mut eink_kit::egui::Ui) {
        let ctx = ui.ctx().clone();
        // TomeApp::new needs a Context, which exists only once frames run.
        let app = self.app.get_or_insert_with(|| {
            let mut a = TomeApp::new(&ctx);
            a.set_eink(&ctx, true);
            // tomescope's light palette, then eink-kit's paper-and-ink preset
            // over it (black text, outlined controls, press = inverted).
            let color = eink_kit::diagnostics(&ctx).color_panel;
            ctx.global_style_mut(|s| eink_kit::style::apply(s, color));
            if let Some(p) = self.open.take() {
                a.open_from_path(&p);
            }
            a
        });
        app.update(&ctx);
        if self.last_save.elapsed() >= SAVE_EVERY {
            app.save();
            self.last_save = Instant::now();
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        if let Some(a) = self.app.as_mut() {
            a.save();
        }
    }
}

fn main() {
    let open = std::env::args_os().skip(1).map(std::path::PathBuf::from).find(|p| !p.to_string_lossy().starts_with('-'));
    let shell = Shell { app: None, open, last_save: Instant::now() };
    if let Err(e) = eink_kit::run(shell) {
        eprintln!("tomescope-eink: {e}");
        std::process::exit(1);
    }
}
