//! finding — a diagnosis as data, rendered by whichever shell is attached.
//!
//! The lesson from stoat (`apps/stoat/crates/core/src/diag.rs`): the part of
//! rustc's diagnostics worth copying is not the ASCII art, it is that a
//! message is DATA (a code, a title, notes that explain, help that says what
//! to do) built where the context is still available. gitmind's Diagnostics
//! page had verdicts and printed them with `{:?}`: correct, and unreadable.
//! Here each analyser's result explains itself; the GUI draws it as the kit's
//! callout, the CLI as rustc-style text.

use crate::fbdiag::Diagnosis;
use crate::udevcheck::Verdict;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    /// The check passed. Worth saying: "fine" is an answer.
    Ok,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// Short, stable, greppable: `udev-vendor-only`, `fastboot-bulk`.
    pub code: String,
    pub title: String,
    /// Why: what the evidence means.
    pub notes: Vec<String>,
    /// What to do.
    pub help: Vec<String>,
    /// Text to copy and apply elsewhere (a rules line, a command). Offered as
    /// text because the fix usually belongs on another machine.
    pub fix: Option<String>,
}

impl Finding {
    pub fn new(severity: Severity, code: &str, title: impl Into<String>) -> Finding {
        Finding { severity, code: code.into(), title: title.into(), notes: vec![], help: vec![], fix: None }
    }
    pub fn note(mut self, n: impl Into<String>) -> Finding {
        self.notes.push(n.into());
        self
    }
    pub fn help(mut self, h: impl Into<String>) -> Finding {
        self.help.push(h.into());
        self
    }
    pub fn fix(mut self, f: impl Into<String>) -> Finding {
        self.fix = Some(f.into());
        self
    }

    /// rustc-style text for the CLI and logs.
    pub fn render(&self) -> String {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Ok => "ok",
        };
        let mut s = format!("{sev}[{}]: {}\n", self.code, self.title);
        for n in &self.notes {
            s += &format!("  = note: {n}\n");
        }
        for h in &self.help {
            s += &format!("  = help: {h}\n");
        }
        if let Some(f) = &self.fix {
            s += "  = fix:\n";
            for l in f.lines() {
                s += &format!("      {l}\n");
            }
        }
        s
    }
}

/// udev: can this user open the device?
pub fn udev(v: &Verdict, vid: &str, pid: &str) -> Finding {
    match v {
        Verdict::Granted { line, how } => Finding::new(Severity::Ok, "udev-granted", format!("{vid}:{pid} is granted access at line {}", line + 1))
            .note(format!("by {how}")),
        Verdict::VendorOnly { vendor_line } => Finding::new(Severity::Error, "udev-vendor-only",
            format!("the rules know the vendor {vid} but not the product {pid}"))
            .note(format!("line {} matches the vendor; no product line inside that block grants {pid}, so evaluation falls out of it", vendor_line + 1))
            .note("the file supports the manufacturer, not this phone: lsusb shows it and nothing can open it")
            .help("add the rule below on the machine the phone is plugged into, then `udevadm control --reload` and replug")
            .fix(crate::udevcheck::fix_rule(vid, pid)),
        Verdict::NoVendor => Finding::new(Severity::Error, "udev-no-vendor", format!("no rule mentions vendor {vid}"))
            .help("add the rule below on the machine the phone is plugged into, then `udevadm control --reload` and replug")
            .fix(crate::udevcheck::fix_rule(vid, pid)),
    }
}

/// fastboot: what the command's own output means.
pub fn fastboot(d: &Diagnosis) -> Finding {
    match d {
        Diagnosis::Ready(serial) => Finding::new(Severity::Ok, "fastboot-ready", format!("{serial} is in fastboot and answering")),
        Diagnosis::NoDevice => Finding::new(Severity::Warning, "fastboot-none", "nothing in fastboot mode is attached")
            .help("boot the phone into fastboot (power + volume down on a Fairphone) and check the cable carries data"),
        Diagnosis::Permission => Finding::new(Severity::Error, "fastboot-permission", "the device is enumerated but this user cannot open it")
            .note("a udev/group problem, not a cable or controller one")
            .help("paste the rules file into section 1 to see which rule is missing"),
        Diagnosis::BulkTransferUnsupported => Finding::new(Severity::Error, "fastboot-bulk",
            "control transfers work and bulk OUT does not: the USB controller cannot do it")
            .note("\"Write to device failed (No such file or directory)\" is ENOENT from a bulk transfer, not a missing file")
            .note("the same phone, cable and kernel flash fine from a PCI xHCI port and fail on some SoC dwc3 ports")
            .help("use a different USB port or host (a PC's xHCI port), not a different cable or udev rule"),
        Diagnosis::DeviceRefused(msg) => Finding::new(Severity::Error, "fastboot-refused", "a command reached the device and it refused")
            .note(format!("device said: {msg}"))
            .help("the transport is fine; the refusal is about the command or the bootloader state (locked? wrong slot?)"),
        Diagnosis::Unknown => Finding::new(Severity::Info, "fastboot-unknown", "nothing in this output is a line gitmind recognises")
            .help("paste the full output of `fastboot devices` and the failing command"),
    }
}

/// Syslog level of a kernel record as a severity: 0-3 error, 4 warning, 5-7 info.
pub fn klog_severity(level: Option<u8>) -> Severity {
    match level {
        Some(0..=3) => Severity::Error,
        Some(4) => Severity::Warning,
        _ => Severity::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_explains_itself_and_offers_the_fix() {
        let f = udev(&Verdict::VendorOnly { vendor_line: 3 }, "18d1", "4ee0");
        assert_eq!((f.severity, f.code.as_str()), (Severity::Error, "udev-vendor-only"));
        assert!(f.notes[0].contains("line 4"), "1-based for people: {:?}", f.notes);
        assert!(f.fix.as_deref().unwrap().contains("18d1"));
        assert_eq!(udev(&Verdict::Granted { line: 0, how: "MODE=0666".into() }, "a", "b").severity, Severity::Ok);
    }

    #[test]
    fn the_bulk_transfer_signature_points_at_the_port_not_the_cable() {
        let f = fastboot(&Diagnosis::BulkTransferUnsupported);
        assert!(f.help.iter().any(|h| h.contains("port")));
        assert!(!f.help.iter().any(|h| h.contains("udev rule") && !h.contains("not")));
    }

    #[test]
    fn render_reads_like_rustc() {
        let r = Finding::new(Severity::Error, "x-y", "it broke").note("because").help("do this").fix("cmd one\ncmd two").render();
        assert_eq!(r, "error[x-y]: it broke\n  = note: because\n  = help: do this\n  = fix:\n      cmd one\n      cmd two\n");
    }

    #[test]
    fn kernel_levels_map_to_severities() {
        assert_eq!(klog_severity(Some(3)), Severity::Error);
        assert_eq!(klog_severity(Some(4)), Severity::Warning);
        assert_eq!(klog_severity(Some(6)), Severity::Info);
        assert_eq!(klog_severity(None), Severity::Info);
    }
}
