//! fbdiag — what fastboot's own output means, and what to do about it.
//!
//! `fastboot.rs` handles the WIRE protocol: OKAY/FAIL/DATA/INFO packets over
//! bulk endpoints. This handles the layer above — the lines the `fastboot`
//! command prints — because that is what a fleet runner actually gets back
//! when it shells out, and because one of those lines cost this project a full
//! day of misdiagnosis.
//!
//! ## Grounded in captured output, not invented
//!
//! Every pattern here comes from `testdata/fastboot-real-*.log`, three real
//! terminal sessions against a Fairphone 5 in fastboot mode. That matters for
//! one line in particular:
//!
//! ```text
//! getvar:all   FAILED (Write to device failed (No such file or directory))
//! ```
//!
//! "No such file or directory" reads as a missing file and is not one. It is
//! ENOENT from a bulk OUT transfer on a controller that cannot do them —
//! confirmed by the same phone, cable and kernel working on a PCI xHCI port
//! and failing on the SoC's dwc3. The first diagnosis was udev permissions,
//! which was wrong, and the wrongness was expensive: a permission error and a
//! controller error look nothing alike once you know, and identical before.
//!
//! `FLASHING_TRIAGE.md` records the whole investigation. This module is that
//! knowledge in a form the runner can act on rather than a document nobody
//! reads at 2am.

/// What the output means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnosis {
    /// A device is enumerated and answering. Carries its serial.
    Ready(String),
    /// fastboot is waiting: nothing in fastboot mode is attached.
    NoDevice,
    /// Enumerated, but this user cannot open it — a udev/group problem.
    Permission,
    /// Control transfers work and bulk OUT does not. The controller, not the
    /// cable and not the permissions.
    BulkTransferUnsupported,
    /// A command reached the device and it refused.
    DeviceRefused(String),
    /// Nothing recognised.
    Unknown,
}

impl Diagnosis {
    /// Is retrying on this port worth anything?
    ///
    /// `BulkTransferUnsupported` is the one that must answer NO. A runner that
    /// retries it will retry forever: the controller will not grow the
    /// capability, and every attempt looks like a fresh transient failure.
    pub fn retry_worthwhile(&self) -> bool {
        matches!(self, Diagnosis::NoDevice | Diagnosis::DeviceRefused(_))
    }

    /// What to tell the operator to DO. `None` when there is nothing to fix.
    pub fn remedy(&self) -> Option<&'static str> {
        match self {
            Diagnosis::Ready(_) => None,
            Diagnosis::NoDevice => Some("put the device in fastboot mode and check the cable"),
            Diagnosis::Permission => Some(
                "add a udev rule for this vid:pid, or run as root to confirm that is the cause"),
            Diagnosis::BulkTransferUnsupported => Some(
                "move to a different USB controller — a rear PCI xHCI port rather than \
                 the SoC's own; this is not permissions and not the cable"),
            Diagnosis::DeviceRefused(_) => Some("the device answered and declined; check the slot and lock state"),
            Diagnosis::Unknown => None,
        }
    }
}

/// Classify one line of fastboot output.
pub fn classify_line(line: &str) -> Option<Diagnosis> {
    let l = line.trim();
    if l.is_empty() {
        return None;
    }
    // Order matters: the bulk-transfer signature is also a FAILED line, and
    // reading it as a generic device refusal is exactly the misdiagnosis this
    // module exists to prevent.
    if l.contains("Write to device failed") || l.contains("Bulk write failed") {
        return Some(Diagnosis::BulkTransferUnsupported);
    }
    if l.contains("no permissions") || l.contains("insufficient permissions") {
        return Some(Diagnosis::Permission);
    }
    if l.contains("waiting for any device") || l.contains("waiting for device") {
        return Some(Diagnosis::NoDevice);
    }
    if let Some(rest) = l.strip_prefix("FAILED") {
        return Some(Diagnosis::DeviceRefused(
            rest.trim().trim_start_matches('(').trim_end_matches(')').to_string()));
    }
    if let Some(i) = l.find("FAILED (") {
        let msg = &l[i + 8..];
        return Some(Diagnosis::DeviceRefused(msg.trim_end_matches(')').to_string()));
    }
    // `<serial> fastboot` — the device-list line. Checked last because a
    // serial is just a word and the failure lines are more specific.
    let mut parts = l.split_whitespace();
    if let (Some(serial), Some("fastboot"), None) = (parts.next(), parts.next(), parts.next()) {
        if serial.len() >= 4 && serial.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(Diagnosis::Ready(serial.to_string()));
        }
    }
    None
}

/// Classify a whole run, worst finding first.
///
/// A session usually holds several outcomes — `fastboot devices` succeeding
/// and then `getvar` failing is the exact shape of the controller bug, and
/// reporting only the last line would say "refused" where the truth is
/// "enumerated fine, cannot transfer".
pub fn classify(output: &str) -> Diagnosis {
    let mut found: Vec<Diagnosis> = output.lines().filter_map(classify_line).collect();
    if found.is_empty() {
        return Diagnosis::Unknown;
    }
    // Severity order: a hardware limit outranks a refusal outranks absence,
    // and a Ready line does not cancel a later failure — that combination IS
    // the signature.
    found.sort_by_key(|d| match d {
        Diagnosis::BulkTransferUnsupported => 0,
        Diagnosis::Permission => 1,
        Diagnosis::DeviceRefused(_) => 2,
        Diagnosis::NoDevice => 3,
        Diagnosis::Ready(_) => 4,
        Diagnosis::Unknown => 5,
    });
    found.remove(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three captured sessions, verbatim.
    const LOG1: &str = include_str!("../testdata/fastboot-real-1.log");
    const LOG2: &str = include_str!("../testdata/fastboot-real-2.log");
    const LOG3: &str = include_str!("../testdata/fastboot-real-3.log");

    #[test]
    fn the_captured_failure_is_the_controller_not_the_permissions() {
        // The line that cost a day. "No such file or directory" reads as a
        // missing file and is ENOENT from a bulk OUT on a controller that
        // cannot do them.
        let line = "getvar:all                                         \
                    FAILED (Write to device failed (No such file or directory))";
        assert_eq!(classify_line(line), Some(Diagnosis::BulkTransferUnsupported));
        // And it must NOT be read as the device refusing, which is what a
        // generic FAILED match would give.
        assert_ne!(classify_line(line),
                   Some(Diagnosis::DeviceRefused("No such file or directory".into())));
    }

    #[test]
    fn a_controller_limit_is_never_worth_retrying() {
        // A runner that retries this retries forever: the controller will not
        // grow the capability and every attempt looks freshly transient.
        assert!(!Diagnosis::BulkTransferUnsupported.retry_worthwhile());
        assert!(!Diagnosis::Permission.retry_worthwhile(), "a udev rule is not a retry");
        assert!(Diagnosis::NoDevice.retry_worthwhile(), "the cable may yet be plugged in");
        assert!(Diagnosis::DeviceRefused("slot".into()).retry_worthwhile());
    }

    #[test]
    fn enumeration_succeeding_does_not_cancel_a_transfer_failure() {
        // `fastboot devices` works and `getvar` fails: that COMBINATION is the
        // signature, and reporting the last line would say "refused" where the
        // truth is "enumerated fine, cannot transfer".
        let session = "c1146e6f fastboot\n\
                       getvar:all   FAILED (Write to device failed (No such file or directory))";
        assert_eq!(classify(session), Diagnosis::BulkTransferUnsupported);
        assert_eq!(classify_line("c1146e6f fastboot"),
                   Some(Diagnosis::Ready("c1146e6f".into())));
    }

    #[test]
    fn the_captured_sessions_separate_the_broken_port_from_the_working_one() {
        // Three real sessions against ONE FP5, and the contrast is the whole
        // evidence. I first asserted all three failed; the test caught that,
        // and the truth is better: two fail on the SoC's dwc3 and the third
        // succeeds on a PCI xHCI port with an A-to-C adapter — same phone,
        // same cable, same kernel, 226 bootloader variables returned.
        //
        // That is what rules out permissions, udev, the cable and the device.
        // A classifier that called all three the same would have thrown away
        // the control case.
        assert_eq!(classify(LOG1), Diagnosis::BulkTransferUnsupported);
        assert_eq!(classify(LOG2), Diagnosis::BulkTransferUnsupported);
        assert_eq!(classify(LOG3), Diagnosis::Ready("c1146e6f".into()),
                   "the working port must not be diagnosed as broken");

        // Log 2 is the sudo run: root does not help, which is what removed
        // permissions from the list.
        assert!(LOG2.contains("sudo fastboot"), "the sudo run is the point of log 2");
        assert_ne!(classify(LOG2), Diagnosis::Permission,
                   "running as root failed identically, so it is not permissions");

        // And the working session really is the same device.
        assert!(LOG3.contains("serialno:c1146e6f"));
    }

    #[test]
    fn a_waiting_device_is_absence_rather_than_failure() {
        assert_eq!(classify_line("< waiting for any device >"), Some(Diagnosis::NoDevice));
        assert!(Diagnosis::NoDevice.remedy().unwrap().contains("cable"));
    }

    #[test]
    fn shell_noise_is_not_a_diagnosis() {
        // The logs are terminal sessions: prompts, lsusb output, systemd
        // chatter. Reading a prompt as a serial would report a phantom device.
        assert_eq!(classify_line("➜  ~ fastboot devices"), None);
        assert_eq!(classify_line("/:  Bus 001.Port 001: Dev 001, Class=root_hub"), None);
        assert_eq!(classify_line(""), None);
        assert_eq!(classify_line("kesslerd wheel usershares ollama docker plugdev"), None);
    }

    #[test]
    fn the_remedy_for_a_controller_limit_names_the_fix_that_worked() {
        let r = Diagnosis::BulkTransferUnsupported.remedy().unwrap();
        assert!(r.contains("controller"));
        assert!(r.contains("not permissions"),
                "the remedy must rule out the wrong answer, which is the one people try first");
    }
}
