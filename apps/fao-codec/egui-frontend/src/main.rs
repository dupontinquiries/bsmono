fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions::default();
    eframe::run_native("fao codec", opts, Box::new(|_| Ok(Box::new(Shell::default()))))
}
#[derive(Default)]
struct Shell(fao_codec_egui::App);
impl eframe::App for Shell {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) { self.0.ui(ctx); }
}
