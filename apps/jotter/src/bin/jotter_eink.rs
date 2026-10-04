//! jotter on an e-ink tablet: the same `Jotter` app as any egui host, driven by eink-kit (CPU raster, refresh
//! policy, touch) instead of eframe. Launched by AppLoad from xochitl's sidebar, or by gitmind over SSH.
//! Notes, identity and trusted devices live in `~/.local/share/jotter` (on the tablet, `/home/root/...`, which
//! survives reboots and OS updates).

struct Shell {
    app: Option<jotter::app::Jotter>,
    error: Option<String>,
}

impl eink_kit::EinkApp for Shell {
    fn ui(&mut self, ui: &mut eink_kit::egui::Ui) {
        if self.app.is_none() && self.error.is_none() {
            let ctx = ui.ctx().clone();
            let color = eink_kit::diagnostics(&ctx).color_panel;
            ctx.global_style_mut(|s| eink_kit::style::apply(s, color));
            let dir = jotter::app::Jotter::default_dir().unwrap_or_else(|| std::path::PathBuf::from("/home/root/.local/share/jotter"));
            let name = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "tablet".into());
            match jotter::app::Jotter::open(dir, &name) {
                Ok(a) => self.app = Some(a),
                Err(e) => self.error = Some(format!("could not open the data directory: {e}")),
            }
        }
        match (&mut self.app, &self.error) {
            (Some(a), _) => a.ui(ui),
            (None, Some(e)) => { ui.heading("jotter"); ui.label(e); }
            _ => {}
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) { if let Some(a) = self.app.as_mut() { a.save_all(); } }
}

fn main() {
    if let Err(e) = eink_kit::run(Shell { app: None, error: None }) {
        eprintln!("jotter-eink: {e}");
        std::process::exit(1);
    }
}
