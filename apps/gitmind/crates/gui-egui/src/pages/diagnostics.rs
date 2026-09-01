//! Diagnostics — the door the cockpit toolkit never had.
//!
//! `cockpit` holds ~3,400 lines of device-bring-up diagnostics with 92
//! tests and no path from any binary: `udevcheck`, `klog`, `kerneltry`,
//! `fbdiag`, `sparse`, `apkcache`, `join`, `repomap`. Not junk — the
//! cross-references between them (`klog`'s doc points at `kerneltry`)
//! show they were designed as a set. They simply had nowhere to be
//! opened from.
//!
//! ## The order
//!
//! By WHERE A BRING-UP FAILS, not alphabetically:
//!
//!   1. the host cannot see the device      → `udevcheck`
//!   2. it is seen but will not flash       → `fbdiag`
//!   3. it flashed but will not boot        → `kerneltry` + `klog`
//!
//! That is the sequence someone actually walks when a phone is dead, and
//! each step is only worth reading if the one above it passed. An
//! alphabetical list would put `apkcache` first, which is a question you
//! only reach once the device boots.
//!
//! The alternative I did not take: one tab per tool. It fits the code
//! better and the debugging worse — you would have to already know which
//! tool answers your question, which is exactly what you do not know
//! when a device is dark.
//!
//! ## Paste, do not read
//!
//! Every analyser takes TEXT. This page does not run `udevadm`,
//! `fastboot` or `dmesg` itself: the failing device is usually attached
//! to a different machine from the one running gitmind, and a page that
//! could only diagnose the local host would be useless in exactly the
//! case it exists for. Paste the output, get the verdict.

use crate::app::App;

#[derive(Default)]
pub struct State {
    /// `udevadm info` or the rules file itself.
    pub udev_rules: String,
    pub udev_vid: String,
    pub udev_pid: String,
    /// `fastboot devices` output.
    pub fastboot_out: String,
    /// `dmesg` or a pstore crash record.
    pub kernel_log: String,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Diagnostics");
    ui.label(
        egui::RichText::new(
            "paste output from the machine the device is attached to — \
             these read text, they do not run commands here")
            .weak()
            .small(),
    );
    ui.separator();

    let st = &mut app.diagnostics;

    // ── 1. the host cannot see the device ────────────────────────────
    egui::CollapsingHeader::new("1 · host cannot see the device")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(
                    "the quiet one: a rules file is installed, looks right, \
                     lsusb shows the device, and nothing works")
                    .weak()
                    .small(),
            );
            ui.horizontal(|ui| {
                ui.label("vendor");
                ui.add(egui::TextEdit::singleline(&mut st.udev_vid).id_salt("diagnostics-st_udev_vid").desired_width(70.0));
                ui.label("product");
                ui.add(egui::TextEdit::singleline(&mut st.udev_pid).id_salt("diagnostics-st_udev_pid").desired_width(70.0));
            });
            ui.add(
                egui::TextEdit::multiline(&mut st.udev_rules).id_salt("diagnostics-st_udev_rules")
                    .desired_rows(4)
                    .hint_text("paste the udev rules file"),
            );
            if !st.udev_rules.trim().is_empty()
                && !st.udev_vid.trim().is_empty()
                && !st.udev_pid.trim().is_empty()
            {
                let v = cockpit::udevcheck::evaluate(
                    &st.udev_rules, st.udev_vid.trim(), st.udev_pid.trim());
                ui.label(format!("{v:?}"));
                // The fix is offered as TEXT to copy rather than written
                // for them: the rules file is on the other machine, and
                // a button that claimed to fix it could not.
                let fix = cockpit::udevcheck::fix_rule(st.udev_vid.trim(), st.udev_pid.trim());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&fix).monospace().small());
                    if ui.small_button("copy").clicked() {
                        ui.ctx().copy_text(fix.clone());
                    }
                });
            }
        });

    // ── 2. seen but will not flash ───────────────────────────────────
    egui::CollapsingHeader::new("2 · seen but will not flash")
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut st.fastboot_out).id_salt("diagnostics-st_fastboot_out")
                    .desired_rows(3)
                    .hint_text("paste `fastboot devices` output"),
            );
            if !st.fastboot_out.trim().is_empty() {
                ui.label(format!("{:?}", cockpit::fbdiag::classify(&st.fastboot_out)));
            }
        });

    // ── 3. flashed but will not boot ─────────────────────────────────
    egui::CollapsingHeader::new("3 · flashed but will not boot")
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(
                    "kerneltry says when a crash record is worth reading; \
                     klog says what the bytes mean")
                    .weak()
                    .small(),
            );
            ui.add(
                egui::TextEdit::multiline(&mut st.kernel_log).id_salt("diagnostics-st_kernel_log")
                    .desired_rows(6)
                    .hint_text("paste dmesg or /sys/fs/pstore contents"),
            );
            if !st.kernel_log.trim().is_empty() {
                let records = cockpit::klog::parse(&st.kernel_log);
                ui.label(format!("{} record(s)", records.len()));
                // Capped: a dmesg is thousands of lines and drawing all
                // of them would make the page slower than reading the
                // log by hand, which is what it replaces.
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for r in records.iter().take(200) {
                        ui.label(egui::RichText::new(format!("{r:?}")).monospace().small());
                    }
                });
                if records.len() > 200 {
                    ui.label(
                        egui::RichText::new(format!("… {} more", records.len() - 200))
                            .weak()
                            .small(),
                    );
                }
            }
        });
}
