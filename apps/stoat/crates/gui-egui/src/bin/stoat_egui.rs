// stoat-egui binary — eframe bootstrap: parse argv, spawn the worker, apply the
// shared palette, hand everything to draw().
//
//   stoat-egui [--marker LOC] [--page NAME] [BINARY [ARGS...]]
//
// With a BINARY the session starts at once and the window opens on Stack.

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use stoat_egui::shell::{worker, ShellState, PAGES};

struct App {
    state: ShellState,
    view: stoat_egui::View,
    colors: uikit_egui::PaletteColors,
    backdrop: uikit_egui::Backdrop,
}

impl eframe::App for App {
    // eframe 0.34: `ui` is the required method and `update` is deprecated. Most
    // of the tree still drives from `update` with an empty `ui`; a new shell has
    // no reason to start on the deprecated side.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.backdrop.paint(ui.ctx(), &self.colors);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(uikit_egui::glass::surface_fill(self.colors.bg_base)).inner_margin(10.0))
            .show_inside(ui, |ui| stoat_egui::draw(ui, &mut self.state, &mut self.view, &self.colors));
    }
}

fn main() -> eframe::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut take = |name: &str| {
        let i = args.iter().position(|a| a == name)?;
        args.remove(i);
        (i < args.len()).then(|| args.remove(i))
    };
    let marker = take("--marker");
    let page = take("--page").and_then(|p| PAGES.iter().position(|n| n.eq_ignore_ascii_case(&p)));
    let program: Option<(PathBuf, Vec<String>)> = (!args.is_empty()).then(|| (args.remove(0).into(), args));

    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
    let opts = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        // Placeholder id, same one the GTK shell uses; it has to equal the
        // .desktop basename once there is one.
        viewport: egui::ViewportBuilder::default().with_app_id("dev.local.Stoat").with_title("stoat").with_inner_size([400.0, 760.0]).with_min_inner_size([320.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "stoat",
        opts,
        Box::new(move |cc| {
            let colors = uikit_egui::apply_palette(&cc.egui_ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Night));
            let (task_tx, task_rx) = mpsc::channel();
            let (msg_tx, msg_rx) = mpsc::channel();
            let ctx = cc.egui_ctx.clone();
            let repaint: stoat_egui::shell::Repaint = Arc::new(move || ctx.request_repaint());
            let for_worker = repaint.clone();
            std::thread::spawn(move || worker(task_rx, msg_tx, for_worker));

            let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut state = ShellState::new(root, task_tx, msg_rx, repaint);
            if let Some(m) = &marker {
                state.marker_text = m.clone();
            }
            if let Some((program, args)) = program {
                let is_rust = stoat_core::discover::inspect(&program).is_some_and(|f| f.is_rust);
                state.start_debug(program, args, Vec::new(), marker, is_rust);
            }
            if let Some(p) = page {
                state.page = p;
            }
            Ok(Box::new(App { state, view: stoat_egui::View::default(), colors, backdrop: uikit_egui::Backdrop::for_app("stoat") }))
        }),
    )
}
