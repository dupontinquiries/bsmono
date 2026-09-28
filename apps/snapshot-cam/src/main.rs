//! The snapshot-cam window. Everything drawn is `shell::Cam`; this is the
//! eframe host around it, behind the `app` feature.

fn main() -> eframe::Result<()> {
    const APP_ID: &str = "org.vidkit.SnapshotCam";
    eframe::run_native(
        "snapshot-cam",
        // Glow SAID, not merely implied by the feature list: eframe 0.34's
        // defaults include wgpu, whose device descriptor asks for eight
        // colour attachments and dies on a phone GPU that allows five.
        eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport: uikit_egui::desktop::viewport(APP_ID, "snapshot-cam", [420.0, 760.0]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(Host(snapshot_cam::shell::Cam::open(&cc.egui_ctx))))),
    )
}

struct Host(snapshot_cam::shell::Cam);

impl eframe::App for Host {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| self.0.ui(ui));
    }
}
