//! pearl-input-view — headless preview of the PEARL COMPOSER (cracked-pearl facet surface + temporally-
//! stable resonance text + send ripple). Auto-types LOGSCOPE_PEARL_TEXT so a screenshot shows the look;
//! LOGSCOPE_PEARL_PULSE=1 fires the send ripple once the line is in.
use logscope_egui::pearl_input::PearlInput;

struct App { pearl: PearlInput, text: String, sample: Vec<char>, fed: usize, last: f64, pulse: bool, pulsed: bool, last_pulse: f64 }
impl Default for App {
    fn default() -> Self {
        let s = std::env::var("LOGSCOPE_PEARL_TEXT")
            .unwrap_or_else(|_| "Build the file watcher: track offset, reopen on inode change.".to_string());
        Self { pearl: PearlInput::default(), text: String::new(), sample: s.chars().collect(), fed: 0, last: 0.0,
               pulse: std::env::var("LOGSCOPE_PEARL_PULSE").ok().as_deref() == Some("1"), pulsed: false, last_pulse: 0.0 }
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        let t = ui.ctx().input(|i| i.time);
        if t - self.last > 0.05 && self.fed < self.sample.len() { self.text.push(self.sample[self.fed]); self.fed += 1; self.last = t; }
        if self.pulse && self.fed >= self.sample.len() && t - self.last_pulse > 1.4 { self.pearl.pulse(); self.last_pulse = t; }
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_rgb(4, 6, 12)).inner_margin(28.0))
            .show_inside(ui, |ui| {
                ui.add_space(34.0);
                let accent = egui::Color32::from_rgb(0x4d, 0xd6, 0xe6);
                self.pearl.show(ui, &self.text, t as f32, 168.0, accent);
            });
        ui.ctx().request_repaint();
    }
}
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 300.0]).with_title("Pearl Composer"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("pearl-input", options, Box::new(|_cc| Ok(Box::new(App::default()))))
}
