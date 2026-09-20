use crate::shell::ShellState;
use egui::{RichText, Ui};
use uikit_egui::{text_field, PaletteColors};

/// The loop, verbatim: every MI line both ways, the program's output, gdb's
/// console and log, and the diagnostics derived from them. Plus the escape hatch:
/// anything the UI does not model is one gdb command away.
pub fn ui(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut st.show_wire, "MI traffic");
        if ui.small_button("clear").clicked() {
            st.log.clear();
        }
        ui.label(RichText::new(format!("{} lines", st.log.len())).small().color(c.text_weak));
    });
    let input_h = 40.0;
    let h = (ui.available_height() - input_h).max(80.0);
    ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
        uikit_egui::log_view::log_view(ui, &st.log);
    });
    ui.horizontal(|ui| {
        let send_w = 56.0;
        let r = ui.add(text_field::edit("wire-raw", &mut st.raw_cmd).hint_text("gdb command, e.g. info sharedlibrary").desired_width(ui.available_width() - send_w - 8.0));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_sized([send_w, 28.0], egui::Button::new("send")).clicked() || enter {
            st.send_raw();
            r.request_focus();
        }
    });
}
