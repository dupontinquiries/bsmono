// drivemind-egui — rendering for the shell state. Layout is phone-first
// (the fleet includes PinePhone/Fairphone): one column, a horizontal
// device rail, tab strip, content, log. Widgets and palette come from
// shared/uikit; the log pane renders drivemind-core's Event stream
// verbatim — the same loop the CLI prints to stderr.

pub mod shell;
/// What the GUI says: banners, the overwrite warning, the checklist, help text.
pub mod ux;

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
            if ui.selectable_label(st.tab == i, *t).on_hover_text(ux::tab_help(i)).clicked() {
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

    // -- what the last action came to (ux.rs), until closed or BANNER_SECS --
    let mut close_banner = false;
    if let Some((tone, text, since)) = &st.banner {
        if since.elapsed().as_secs() < ux::BANNER_SECS {
            let fill = match tone {
                ux::Tone::Good => colors.success.gamma_multiply(0.25),
                ux::Tone::Info => colors.bg_raised,
                ux::Tone::Bad => colors.danger.gamma_multiply(0.25),
            };
            egui::Frame::new().fill(fill).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(text.as_str());
                    if ui.small_button("✕").on_hover_text("Dismiss").clicked() { close_banner = true; }
                });
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
    if close_banner { st.banner = None; }
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
    // -- getting started: ticked off from what has actually happened --
    if !st.guide_hidden {
        let steps = ux::getting_started(st.selected.is_some(), st.smart.is_some(),
            std::path::Path::new(&st.manifest_path).is_file(), st.verify.is_some());
        if let Some(next) = ux::next_step(&steps).cloned() {
            egui::Frame::new().fill(colors.bg_raised).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Getting started").strong());
                    if ui.small_button("Hide").on_hover_text("Hide this checklist").clicked() { st.guide_hidden = true; }
                });
                for s in &steps {
                    let mark = if s.done { "✓" } else if s.label == next.label { "→" } else { "○" };
                    ui.label(format!("{mark} {}", s.label));
                }
                ui.label(RichText::new(next.hint).color(colors.text_weak));
            });
            ui.add_space(6.0);
        }
    }
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
            if ui.add_enabled(!st.busy, egui::Button::new("Probe SMART"))
            .on_hover_text("Read the drive's own health counters (smartctl). Read-only; nothing is written.")
            .clicked() {
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
        ui.label("root").on_hover_text("The folder whose files are recorded and checked");
        ui.add(egui::TextEdit::singleline(&mut st.scan_root).id_salt("lib-st_scan_root-1"))
            .on_hover_text("The folder whose files are recorded and checked");
    });
    ui.horizontal(|ui| {
        ui.label("manifest").on_hover_text("The file that holds the record (one hash per file)");
        ui.add(egui::TextEdit::singleline(&mut st.manifest_path).id_salt("lib-st_manifest_path"))
            .on_hover_text("The file that holds the record (one hash per file). Keep it somewhere safe -- ideally not on the drive it describes.");
    });
    ui.horizontal(|ui| {
        let root = std::path::PathBuf::from(&st.scan_root);
        let manifest = std::path::PathBuf::from(&st.manifest_path);
        if ui.add_enabled(!st.busy, egui::Button::new("Create manifest"))
            .on_hover_text("Record every file under the folder: its size and a content hash. Asks first if a manifest is already there.")
            .clicked() {
            st.request_create();
        }
        if ui.add_enabled(!st.busy, egui::Button::new("Verify"))
            .on_hover_text("Compare the folder against the manifest: changed, missing and unreadable files. Read-only.")
            .clicked() {
            st.send(Task::VerifyCheck { root, manifest });
        }
    });
    // -- replacing a manifest needs a yes (ux::overwrite_warning) --
    if let Some(w) = st.confirm_overwrite.clone() {
        egui::Frame::new().fill(colors.danger.gamma_multiply(0.2)).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
            ui.label(RichText::new("Replace the existing manifest?").strong());
            ui.label(w);
            ui.horizontal(|ui| {
                if ui.button("Replace it").clicked() { st.start_create(); }
                if ui.button("Keep the old one").clicked() { st.confirm_overwrite = None; }
            });
        });
    }
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
        if ui.add_enabled(!st.busy, egui::Button::new("Scan"))
            .on_hover_text("Look for caches, duplicates and leftovers under the path. Suggests only; deletes nothing.")
            .clicked() {
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
        if ui.add_enabled(!st.busy, egui::Button::new("Find candidates"))
            .on_hover_text("Find videos that would shrink a lot if re-encoded (by bitrate and codec).")
            .clicked() {
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
    if !v.candidates.is_empty() && ui.button("Write av1tuner batch")
        .on_hover_text("Write the candidates to a batch file for av1tuner. Your videos are not touched.")
        .clicked() {
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
        st.log.push(LogEntry::new(if ok { LogLevel::Info } else { LogLevel::Error }, msg.clone(), id));
        st.say(if ok {
            (ux::Tone::Good, format!("{msg} -- open it in av1tuner to encode. Your videos are not touched."))
        } else {
            (ux::Tone::Bad, format!("The batch was not written: {msg}"))
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_label_covers_every_variant_with_a_distinct_string() {
        // The one genuinely pure function in this file -- everything
        // else takes `ui: &mut Ui` and needs a real rendering context
        // (egui_kittest + a Vulkan/lavapipe backend, per this session's
        // earlier aperture/voiceterm work) to exercise meaningfully,
        // which isn't available here. This file had zero tests before
        // this. A trivial lookup table, but an exhaustive match still
        // benefits from confirming no two variants accidentally share a
        // label (a copy-paste mistake in a match arm is easy to make and
        // easy to miss on a quick read).
        let labels = [
            attach_label(Attach::NativeNvme),
            attach_label(Attach::NativeSata),
            attach_label(Attach::UsbBridge),
            attach_label(Attach::Virtual),
            attach_label(Attach::Unknown),
        ];
        let mut sorted = labels.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "every Attach variant must get its own distinct label: {labels:?}");
        assert_eq!(attach_label(Attach::NativeNvme), "NVMe");
        assert_eq!(attach_label(Attach::UsbBridge), "USB bridge");
    }
}
