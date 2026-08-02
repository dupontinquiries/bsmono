/// Must match packaging/org.faocodec.FaoCodec.desktop exactly — a mismatch fails
/// silently, leaving a working app with no icon.
const APP_ID: &str = "org.faocodec.FaoCodec";

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: uikit_egui::desktop::viewport(APP_ID, "FAO Codec", [900.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native("fao codec", opts, Box::new(|_| Ok(Box::new(Shell::default()))))
}
#[derive(Default)]
struct Shell(fao_codec_egui::App);
impl eframe::App for Shell {
    // The house eframe fork requires the root-Ui entry point (update has a default impl
    // routing through it) — same shape as the other -egui shells.
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) { self.0.ui(ui.ctx()); }
}
