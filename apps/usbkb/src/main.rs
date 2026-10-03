//! usbkb CLI and the small remote-keyboard window.
//!
//!   usbkb status                     what the hardware will do (read-only)
//!   usbkb script setup|teardown      print the root commands for the keyboard gadget
//!   usbkb script swap <port>         print the data-role swap for a USB-C port
//!   usbkb type "text" [--enter]      type through /dev/hidg0
//!   usbkb gui                        a window with keys; a masked box that types then presses Enter
//!
//! The text for `type` (or the masked box) is never stored, logged or echoed.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use usbkb::{gadget, hid, status};

const HIDG: &str = "/dev/hidg0";

/// Send reports with a pause the host's polling can keep up with.
fn send(reports: &[[u8; 8]]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(HIDG)?;
    for r in reports {
        f.write_all(r)?;
        std::thread::sleep(Duration::from_millis(15));
    }
    Ok(())
}

fn type_text(text: &str, enter: bool) -> Result<(), String> {
    let (mut reports, skipped) = hid::type_reports(text);
    if !skipped.is_empty() {
        return Err(format!("{} character(s) have no key on a US layout; nothing was sent", skipped.len()));
    }
    if enter {
        reports.push(hid::report(0, hid::ENTER));
        reports.push(hid::RELEASE);
    }
    send(&reports).map_err(|e| format!("{HIDG}: {e} (is the gadget set up? `usbkb status`)"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("status") => print!("{}", status::report(Path::new("/sys"), Path::new("/dev"))),
        Some("script") => match args.get(1).map(String::as_str) {
            Some("setup") => print!("{}", gadget::setup_script()),
            Some("teardown") => print!("{}", gadget::teardown_script()),
            Some("swap") => print!("{}", gadget::swap_script(args.get(2).map(String::as_str).unwrap_or("port2"))),
            _ => eprintln!("usbkb script setup|teardown|swap <port>"),
        },
        Some("type") => {
            let Some(text) = args.get(1) else { return eprintln!("usbkb type \"text\" [--enter]") };
            if let Err(e) = type_text(text, args.iter().any(|a| a == "--enter")) {
                eprintln!("usbkb: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(feature = "gui")]
        Some("gui") => {
            let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 560.0]).with_title("usbkb"), ..Default::default() };
            let _ = eframe::run_native("usbkb", opts, Box::new(|_| Ok(Box::new(gui::Remote::default()))));
        }
        _ => eprintln!("usbkb status | script setup|teardown|swap <port> | type \"text\" [--enter] | gui"),
    }
}

#[cfg(feature = "gui")]
mod gui {
    use super::*;

    #[derive(Default)]
    pub struct Remote {
        secret: String,
        show: bool,
        note: String,
        last_status: String,
    }

    impl Remote {
        fn key(&mut self, code: u8) {
            self.note = match send(&[hid::report(0, code), hid::RELEASE]) {
                Ok(()) => String::new(),
                Err(e) => format!("{e}"),
            };
        }
    }

    impl eframe::App for Remote {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            ui.heading("usbkb: a keyboard for the other end of the cable");
            ui.label("Type below and press Send; the text goes out as a keyboard and is not kept.");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.secret).password(!self.show).id_salt("usbkb-secret").desired_width(260.0));
                ui.checkbox(&mut self.show, "show");
            });
            ui.horizontal(|ui| {
                if ui.button("Send").clicked() {
                    self.note = type_text(&self.secret, false).err().unwrap_or_default();
                }
                if ui.button("Send + Enter").clicked() {
                    self.note = type_text(&self.secret, true).err().unwrap_or_default();
                    if self.note.is_empty() {
                        self.secret.clear();
                    }
                }
                if ui.button("Clear").clicked() {
                    self.secret.clear();
                }
            });
            ui.separator();
            ui.label("Keys");
            let keys: [(&str, u8); 8] = [("Enter", hid::ENTER), ("Esc", hid::ESC), ("Tab", hid::TAB), ("⌫", hid::BACKSPACE), ("←", hid::LEFT), ("↑", hid::UP), ("↓", hid::DOWN), ("→", hid::RIGHT)];
            ui.horizontal_wrapped(|ui| {
                for (label, code) in keys {
                    if ui.add(egui::Button::new(label).min_size(egui::vec2(64.0, 44.0))).clicked() {
                        self.key(code);
                    }
                }
            });
            ui.separator();
            if ui.button("Check hardware").clicked() {
                self.last_status = status::report(Path::new("/sys"), Path::new("/dev"));
            }
            if !self.last_status.is_empty() {
                ui.monospace(&self.last_status);
            }
            if !self.note.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(230, 90, 90), &self.note);
            }
        }
    }
}
