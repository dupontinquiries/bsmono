//! Rotate tab — rotation/compression options that generate a logrotate config,
//! with save targets and apply buttons. Ports `ui/rotation_tab.rs`.
use crate::theme;
use egui::RichText;

const TARGETS: &[&str] = &["/etc/logrotate.d", "~/.config/logrotate", "~/.config/systemd/user"];

pub struct Rotate {
    path: String,
    rotate_count: u32,
    compress: bool,
    collapse: bool,
    target: usize,
    applied: bool,
}
impl Default for Rotate {
    fn default() -> Self {
        Self { path: "/var/log/myapp/app.log".into(), rotate_count: 7, compress: true,
            collapse: false, target: 0, applied: false }
    }
}
impl Rotate {
    fn config_text(&self) -> String {
        let mut s = String::from("# logscope — generated logrotate config\n");
        s += &format!("'{}' {{\n", self.path);
        s += &format!("    rotate {}\n", self.rotate_count);
        if self.compress { s += "    compress\n    delaycompress\n"; }
        s += "    missingok\n    notifempty\n";
        if self.collapse { s += "    # logscope: collapse repeated lines (N× <line>)\n"; }
        s += "}\n";
        s
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Log path").color(theme::DIM()).small());
        ui.add(egui::TextEdit::singleline(&mut self.path).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Keep");
            ui.add(egui::DragValue::new(&mut self.rotate_count).range(1..=90));
            ui.label("rotations");
        });
        ui.checkbox(&mut self.compress, "Compression (gzip + delaycompress)");
        ui.checkbox(&mut self.collapse, "Collapse repeated lines  ('N× <line>' instead of N identical lines)");
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("Save target:");
            egui::ComboBox::from_id_salt("rot_target").selected_text(TARGETS[self.target]).show_ui(ui, |ui| {
                for (i, t) in TARGETS.iter().enumerate() { ui.selectable_value(&mut self.target, i, *t); }
            });
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Config editor").color(theme::DIM()).small());
        let mut cfg = self.config_text();
        egui::ScrollArea::vertical().max_height(220.0).id_salt("rot_cfg").show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut cfg).interactive(false)
                .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if theme::action_button(ui, "⧉ Copy config", theme::CYAN(), true).clicked() {
                ui.ctx().copy_text(self.config_text());
                self.applied = true;
            }
            let _ = ui.button("Apply (needs auth)").on_hover_text("pkexec / sudo for system targets");
            if self.applied { ui.label(RichText::new("Copied to clipboard ✓").color(theme::GREEN())); }
        });
    }
}
