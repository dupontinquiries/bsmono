// drivemind-egui — rendering for the shell state. Layout is phone-first
// (the fleet includes PinePhone/Fairphone): one column, a horizontal
// device rail, tab strip, content, log. Widgets and palette come from
// shared/uikit; the log pane renders drivemind-core's Event stream
// verbatim — the same loop the CLI prints to stderr.

pub mod shell;

use drivemind_core::{device::Attach, human_bytes, smart};
use egui::{Color32, RichText, Ui};
use shell::{ShellState, Task};

fn sev_color(colors: &uikit_egui::PaletteColors, s: smart::Severity) -> Color32 {
    match s {
        smart::Severity::Ok => colors.success,
        smart::Severity::Advisory => colors.accent,
        smart::Severity::Warning => colors.warning,
        smart::Severity::Critical => colors.danger,
    }
}

fn attach_label(a: Attach) -> &'static str {
    match a {
        Attach::NativeNvme => "NVMe",
        Attach::NativeSata => "SATA",
        Attach::UsbBridge => "USB bridge",
        Attach::Virtual => "virtual",
        Attach::Unknown => "?",
    }
}

pub fn draw(ui: &mut Ui, st: &mut ShellState, colors: &uikit_egui::PaletteColors) {
    st.pump();

    // -- device rail --
    ui.horizontal_wrapped(|ui| {
        for i in 0..st.devices.len() {
            let d = &st.devices[i];
            let selected = st.selected == Some(i);
            let label = format!(
                "{}\n{} · {}",
                d.path,
                human_bytes(d.size_bytes),
                attach_label(d.attach)
            );
            if ui.selectable_label(selected, label).clicked() {
                st.selected = Some(i);
                st.smart = None;
            }
        }
        if ui.button("↻").on_hover_text("re-enumerate").clicked() {
            st.send(Task::Enumerate);
        }
    });
    ui.separator();

    // -- tabs --
    ui.horizontal(|ui| {
        for (i, t) in ["Health", "Verify", "Cleanup", "Videos", "Advise"].iter().enumerate() {
            if ui.selectable_label(st.tab == i, *t).clicked() {
                st.tab = i;
            }
        }
        if st.busy {
            ui.spinner();
            if let Some((done, Some(total))) = st.progress {
                ui.label(format!("{done}/{total}"));
            }
        }
    });
    ui.separator();

    match st.tab {
        0 => tab_health(ui, st, colors),
        1 => tab_verify(ui, st, colors),
        2 => tab_cleanup(ui, st),
        3 => tab_videos(ui, st),
        _ => tab_advise(ui, st, colors),
    }

    // -- the engine's loop, verbatim --
    ui.separator();
    ui.label(RichText::new("engine").small().color(colors.text_weak));
    egui::ScrollArea::vertical()
        .id_salt("engine-log")
        .max_height(140.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            uikit_egui::log_view::log_view(ui, &st.log);
        });
}

fn tab_health(ui: &mut Ui, st: &mut ShellState, colors: &uikit_egui::PaletteColors) {
    let Some(i) = st.selected else {
        ui.label("no device selected");
        return;
    };
    let (path, capable) = {
        let d = &st.devices[i];
        (d.path.clone(), d.smart_capable())
    };
    ui.horizontal(|ui| {
        ui.heading(&path);
        if capable {
            if ui.add_enabled(!st.busy, egui::Button::new("Probe SMART")).clicked() {
                st.send(Task::Smart(path.clone()));
            }
        } else {
            ui.label(RichText::new("no SMART on this attach type").color(colors.text_weak));
        }
    });
    let Some(rep) = &st.smart else { return };
    if rep.device != path {
        return; // stale report from a previously selected device
    }
    ui.horizontal(|ui| {
        ui.label(rep.model.as_deref().unwrap_or("?"));
        if let Some(t) = &rep.device_type_used {
            ui.label(RichText::new(format!("via -d {t}")).small().color(colors.text_weak))
                .on_hover_text("reached through the USB-NVMe bridge ladder");
        }
        if let Some(t) = rep.temperature_c {
            ui.label(format!("{t}°C"));
        }
        if let Some(h) = rep.power_on_hours {
            ui.label(format!("{h} h"));
        }
    });
    for f in &rep.findings {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{:?}", f.severity))
                    .strong()
                    .color(sev_color(colors, f.severity)),
            );
            ui.label(&f.message);
        });
    }
}

fn tab_verify(ui: &mut Ui, st: &mut ShellState, colors: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        ui.label("root");
        ui.add(egui::TextEdit::singleline(&mut st.scan_root).id_salt("lib-st_scan_root-1"));
    });
    ui.horizontal(|ui| {
        ui.label("manifest");
        ui.add(egui::TextEdit::singleline(&mut st.manifest_path).id_salt("lib-st_manifest_path"));
    });
    ui.horizontal(|ui| {
        let root = std::path::PathBuf::from(&st.scan_root);
        let manifest = std::path::PathBuf::from(&st.manifest_path);
        if ui.add_enabled(!st.busy, egui::Button::new("Create manifest")).clicked() {
            st.send(Task::VerifyCreate { root: root.clone(), manifest: manifest.clone() });
        }
        if ui.add_enabled(!st.busy, egui::Button::new("Verify")).clicked() {
            st.send(Task::VerifyCheck { root, manifest });
        }
    });
    if let Some(rep) = &st.verify {
        let (txt, color) = if rep.clean() {
            (format!("clean — {} file(s), {}", rep.ok, human_bytes(rep.bytes_hashed)), colors.success)
        } else {
            (
                format!(
                    "ok {} · changed {} · missing {} · new {} · READ ERRORS {}",
                    rep.ok,
                    rep.changed.len(),
                    rep.missing.len(),
                    rep.new.len(),
                    rep.read_errors.len()
                ),
                if rep.read_errors.is_empty() { colors.warning } else { colors.danger },
            )
        };
        ui.label(RichText::new(txt).color(color));
        for (p, e) in &rep.read_errors {
            ui.label(RichText::new(format!("READERR {p}: {e}")).color(colors.danger));
        }
        for (p, _) in rep.changed.iter().take(50) {
            ui.label(format!("changed {p}"));
        }
    }
}

fn tab_cleanup(ui: &mut Ui, st: &mut ShellState) {
    ui.horizontal(|ui| {
        ui.label("path");
        ui.add(egui::TextEdit::singleline(&mut st.scan_root).id_salt("lib-st_scan_root-2"));
        if ui.add_enabled(!st.busy, egui::Button::new("Scan")).clicked() {
            st.send(Task::Cleanup(std::path::PathBuf::from(&st.scan_root)));
        }
    });
    let mut copy: Option<String> = None;
    egui::ScrollArea::vertical().id_salt("cleanup").show(ui, |ui| {
        for s in &st.cleanup {
            ui.horizontal(|ui| {
                ui.label(RichText::new(human_bytes(s.reclaim_bytes)).strong());
                ui.label(&s.note);
                if let Some(c) = &s.command {
                    if ui.small_button("copy cmd").on_hover_text(c).clicked() {
                        copy = Some(c.clone());
                    }
                }
            });
        }
    });
    if let Some(c) = copy {
        ui.ctx().copy_text(c);
    }
}

fn tab_videos(ui: &mut Ui, st: &mut ShellState) {
    ui.horizontal(|ui| {
        ui.label("path");
        ui.add(egui::TextEdit::singleline(&mut st.scan_root).id_salt("lib-st_scan_root-3"));
        if ui.add_enabled(!st.busy, egui::Button::new("Find candidates")).clicked() {
            st.send(Task::Videos(std::path::PathBuf::from(&st.scan_root)));
        }
    });
    let Some(v) = &st.videos else { return };
    ui.label(format!(
        "{} candidate(s) · {} · est. {} back via av1tuner",
        v.candidates.len(),
        human_bytes(v.total_candidate_bytes),
        human_bytes(v.est_savings_bytes())
    ));
    let mut log_line: Option<(bool, String)> = None;
    if !v.candidates.is_empty() && ui.button("Write av1tuner batch").clicked() {
        let out = std::env::temp_dir().join("drivemind-av1tuner-batch.txt");
        log_line = Some(match v.write_batch(&out) {
            Ok(msg) => (true, msg),
            Err(e) => (false, e),
        });
    }
    egui::ScrollArea::vertical().id_salt("videos").show(ui, |ui| {
        for c in &v.candidates {
            ui.label(format!("{}  {}  {}", human_bytes(c.size), c.why, c.path));
        }
    });
    if let Some((ok, msg)) = log_line {
        use uikit_egui::log_view::{LogEntry, LogLevel};
        let id = 1_000_000 + st.log.len() as u64;
        st.log.push(LogEntry::new(if ok { LogLevel::Info } else { LogLevel::Error }, msg, id));
    }
}

fn tab_advise(ui: &mut Ui, st: &mut ShellState, colors: &uikit_egui::PaletteColors) {
    use drivemind_core::advisor::Priority;
    let recs = st.recommendations();
    if recs.is_empty() {
        ui.label("nothing to recommend yet — probe a device, verify, or scan first");
        return;
    }
    for r in &recs {
        let c = match r.priority {
            Priority::Emergency => colors.danger,
            Priority::Now => colors.warning,
            Priority::Soon => colors.accent,
            Priority::Info => colors.text_weak,
        };
        ui.label(RichText::new(format!("[{:?}] {}", r.priority, r.title)).strong().color(c));
        ui.label(&r.detail);
        if let Some(cmd) = &r.command {
            ui.horizontal(|ui| {
                ui.monospace(cmd);
                if ui.small_button("copy").clicked() {
                    ui.ctx().copy_text(cmd.clone());
                }
            });
        }
        ui.add_space(6.0);
    }
}
