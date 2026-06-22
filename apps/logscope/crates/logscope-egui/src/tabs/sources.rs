//! Sources tab — presets, a command runner, a live filter, and a log input
//! that loads into the shared Doc (feeding Peek/Tokens/Blocks). Ports `ui/sources_tab.rs`.
use crate::analysis::Doc;
use crate::filepicker::{FilePicker, Outcome};
use crate::theme;
use egui::RichText;
use regex::Regex;

const PRESETS: &[(&str, &str)] = &[
    ("nginx errors", "journalctl -u nginx -f | grep ERROR"),
    ("kernel", "journalctl -k -f"),
    ("this user", "journalctl --user -f"),
    ("syslog", "tail -F /var/log/syslog"),
    ("dmesg", "dmesg -w"),
];

pub struct Sources {
    cmd: String,
    input: String,
    filter: String,
    regex: bool,
    invert: bool,
    running: bool,
    picker: FilePicker,
}
impl Default for Sources {
    fn default() -> Self {
        Self {
            cmd: "journalctl -u nginx -f | grep ERROR".into(),
            input: crate::analysis::SAMPLE.into(),
            filter: String::new(), regex: false, invert: false, running: false,
            picker: FilePicker::default(),
        }
    }
}
impl Sources {
    fn matches(&self, line: &str, re: &Option<Regex>) -> bool {
        if self.filter.is_empty() { return true; }
        let m = match re {
            Some(r) => r.is_match(line),
            None => line.contains(&self.filter),
        };
        m ^ self.invert
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &mut Doc) -> Option<String> {
        let mut send: Option<String> = None;
        ui.label(RichText::new("System sources").color(theme::DIM()).small());
        ui.horizontal_wrapped(|ui| {
            for (label, cmd) in PRESETS {
                if ui.button(*label).clicked() { self.cmd = (*cmd).into(); }
            }
        });
        ui.separator();

        ui.label("Custom command");
        ui.add(egui::TextEdit::singleline(&mut self.cmd)
            .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.running, egui::Button::new("▶ Run")).clicked() { self.running = true; }
            if ui.add_enabled(self.running, egui::Button::new("■ Stop")).clicked() { self.running = false; }
            ui.label(RichText::new(if self.running { "Running…" } else { "No source running" })
                .color(theme::DIM()).small());
        });

        ui.add_space(6.0);
        ui.label("Log input (paste, then load into the view)");
        egui::ScrollArea::vertical().max_height(160.0).id_salt("src_in").show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut self.input)
                .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(6));
        });
        ui.horizontal(|ui| {
            if ui.button("Load into view →").clicked() {
                doc.set_from_text("pasted", &self.input);
            }
            if ui.button("📂 Open file…").clicked() {
                let start = std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("/var/log"));
                self.picker.open_file(&start, "Open a log file", &[]);
            }
            ui.label(RichText::new(format!("in view: {} · {} lines", doc.name, doc.lines.len()))
                .color(theme::DIM()).small());
        });
        // file picker modal (loads the chosen file straight into the view)
        if let Some(Outcome::Picked(path)) = self.picker.ui(ui.ctx()) {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let capped = if bytes.len() > 4_000_000 { &bytes[..4_000_000] } else { &bytes[..] };
                    let text = String::from_utf8_lossy(capped).into_owned();
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
                    doc.set_from_text(&name, &text);
                    self.input = text;
                }
                Err(e) => { self.input = format!("// could not read {}: {e}", path.display()); }
            }
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("pattern (blank = show all)"));
            ui.checkbox(&mut self.regex, "regex");
            ui.checkbox(&mut self.invert, "invert");
        });
        if !self.filter.is_empty() {
            let re = if self.regex { Regex::new(&self.filter).ok() } else { None };
            let invalid = self.regex && re.is_none();
            if invalid {
                ui.label(RichText::new("invalid regex").color(theme::RED()).small());
            } else {
                let hits: Vec<&String> = doc.lines.iter().filter(|l| self.matches(l, &re)).collect();
                ui.label(RichText::new(format!("{} / {} lines match", hits.len(), doc.lines.len()))
                    .color(theme::GREEN()).small());
                egui::ScrollArea::vertical().max_height(120.0).id_salt("src_filt").show(ui, |ui| {
                    for l in hits.iter().take(50) {
                        ui.label(RichText::new(*l).monospace().color(theme::INK()));
                    }
                });
            }
        }

        ui.add_space(8.0);
        if theme::action_button(ui, "✦ AI Analysis", theme::PURPLE(), !doc.lines.is_empty()).clicked() {
            let body: Vec<&str> = doc.lines.iter().map(|s| s.as_str()).take(120).collect();
            send = Some(format!(
                "Analyze these log lines: identify the distinct failures and the most likely root cause, concisely.\n\n{}",
                body.join("\n")));
        }
        send
    }
}
