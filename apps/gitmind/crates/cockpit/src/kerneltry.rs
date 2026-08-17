//! kerneltry — flash a kernel, and find out what happened when it does not boot.
//!
//! [`slots`](crate::slots) makes the attempt safe: the new kernel goes to the
//! spare half and the good one survives. This is the loop around that, and the
//! problem it exists to solve is narrower than "flash a kernel":
//!
//! **A kernel that does not boot cannot tell you why.** There is no ssh, no
//! journal, no shell. The obvious plan — flash, reboot, read the logs — works
//! only in the case where you did not need the logs.
//!
//! So evidence has to come from somewhere that survives the failure. In
//! descending order of how much they tell you:
//!
//! 1. **pstore / ramoops** — `/sys/fs/pstore`, read after you have reverted to
//!    the good slot. The kernel that died wrote its oops to memory the next
//!    boot can still read. This is the only source that requires no extra
//!    hardware and still captures a panic, which makes it the one worth
//!    building around.
//! 2. **A serial console** — everything, including the very early boot that
//!    never reaches pstore, but it needs a cable most people do not have.
//! 3. **The bootloader's own counters** — `slot-retry-count` and
//!    `slot-successful` say *that* it failed, not why. Cheap, always
//!    available, and the thing that decides whether to keep trying.
//!
//! ## Confirmation is a positive signal, never a timeout
//!
//! The device is unreachable while it boots, and unreachable if it never
//! boots. Those are the same observation until something answers. So a try is
//! only Booted when the device says so, and NoBoot when a deadline passes
//! without that — never the reverse. Getting this backwards means a slow boot
//! on a cold cache reads as a failed kernel and reverts a change that worked.
//!
//! ## Confirming the slot is a separate act from booting
//!
//! Android bootloaders keep a retry counter per slot and roll back to the
//! other one when it runs out. That is a useful safety net and a confusing
//! one: a kernel that boots but never gets marked successful will be reverted
//! underneath you a few reboots later, and the change will appear to have
//! "stopped working" for no reason. So marking the slot successful is an
//! explicit step, taken only after the device has confirmed.

use crate::slots::{Slot, SlotInfo};

/// Where a try has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing attempted yet.
    Idle,
    /// Image written to the spare slot, not yet switched to.
    Flashed { slot: Slot },
    /// Switched and rebooting. `waited_ms` accumulates until the deadline.
    Booting { slot: Slot, waited_ms: u64 },
    /// The device answered. The kernel runs.
    Booted { slot: Slot },
    /// The deadline passed with no answer.
    NoBoot { slot: Slot },
    /// Switched back to the slot that works.
    Reverted { to: Slot, from: Slot },
}

/// What the loop wants done next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Keep waiting; the deadline has not passed.
    Wait,
    /// Mark the slot successful so the bootloader stops counting retries.
    ConfirmSlot(Slot),
    /// Switch back and reboot.
    Revert { to: Slot },
    /// Read `/sys/fs/pstore` — the dead kernel's last words, readable now that
    /// a working kernel is up.
    CollectEvidence { about: Slot },
    /// Finished, one way or the other.
    Done,
}

/// How long to give a kernel before calling it dead.
///
/// Generous on purpose. A first boot after a flash does more work than a warm
/// one — filesystem checks, a cold page cache, sometimes a resize — and
/// declaring failure early reverts changes that were only slow.
pub const BOOT_DEADLINE_MS: u64 = 120_000;

/// A kernel experiment in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelTry {
    pub state: State,
    /// The slot that was known good when this started, and the one to fall
    /// back to. Recorded at the start rather than recomputed, because after a
    /// switch the device's idea of "current" is the slot under test.
    pub known_good: Slot,
    pub deadline_ms: u64,
}

impl KernelTry {
    /// Begin, given what the device reported before anything changed.
    ///
    /// `None` when the device is not A/B: there is no spare half, so there is
    /// no safe experiment to run, and the caller should say so rather than
    /// overwrite the only kernel.
    pub fn start(info: &SlotInfo) -> Option<KernelTry> {
        let good = info.current?;
        info.target()?;
        Some(KernelTry { state: State::Idle, known_good: good, deadline_ms: BOOT_DEADLINE_MS })
    }

    /// The slot under test.
    pub fn target(&self) -> Slot {
        self.known_good.other()
    }

    /// The image has been written.
    pub fn flashed(&mut self) {
        self.state = State::Flashed { slot: self.target() };
    }

    /// `set_active` and reboot have been issued.
    pub fn rebooted(&mut self) {
        self.state = State::Booting { slot: self.target(), waited_ms: 0 };
    }

    /// Time passed with no word from the device.
    ///
    /// Only meaningful while Booting; anywhere else it is a no-op, so a caller
    /// polling on a timer cannot accidentally advance a finished try.
    pub fn tick(&mut self, dt_ms: u64) {
        if let State::Booting { slot, waited_ms } = &self.state {
            let waited = waited_ms + dt_ms;
            self.state = if waited >= self.deadline_ms {
                State::NoBoot { slot: *slot }
            } else {
                State::Booting { slot: *slot, waited_ms: waited }
            };
        }
    }

    /// The device answered.
    ///
    /// The only way to reach Booted. Accepted late — a device that answers
    /// just after the deadline booted slowly, and calling that a failure and
    /// reverting a working kernel is the worse mistake.
    pub fn device_answered(&mut self) {
        match self.state {
            State::Booting { slot, .. } | State::NoBoot { slot } => {
                self.state = State::Booted { slot };
            }
            _ => {}
        }
    }

    /// The revert has been carried out.
    pub fn reverted(&mut self) {
        if let State::NoBoot { slot } = self.state {
            self.state = State::Reverted { to: self.known_good, from: slot };
        }
    }

    /// What to do now.
    pub fn next(&self) -> Next {
        match self.state {
            State::Idle | State::Flashed { .. } => Next::Wait,
            State::Booting { .. } => Next::Wait,
            // Booted is not the end: an unconfirmed slot gets rolled back by
            // the bootloader a few reboots later, and the change appears to
            // stop working for no reason.
            State::Booted { slot } => Next::ConfirmSlot(slot),
            State::NoBoot { .. } => Next::Revert { to: self.known_good },
            // Evidence is collected AFTER the revert, because reading pstore
            // needs a kernel that runs.
            State::Reverted { from, .. } => Next::CollectEvidence { about: from },
        }
    }

    /// Did the kernel under test come up?
    pub fn succeeded(&self) -> bool {
        matches!(self.state, State::Booted { .. })
    }

    /// One line for the operator.
    pub fn summary(&self) -> String {
        match &self.state {
            State::Idle => "ready".into(),
            State::Flashed { slot } => format!("written to slot {}", slot.letter()),
            State::Booting { slot, waited_ms } => {
                format!("booting slot {} ({}s)", slot.letter(), waited_ms / 1000)
            }
            State::Booted { slot } => format!("slot {} booted", slot.letter()),
            State::NoBoot { slot } => format!(
                "slot {} did not boot within {}s",
                slot.letter(),
                self.deadline_ms / 1000
            ),
            State::Reverted { to, from } => {
                format!("reverted to slot {} after {} failed", to.letter(), from.letter())
            }
        }
    }
}

/// Where a dead kernel's last words live.
///
/// Read after reverting. Files here are written by the *previous* boot, which
/// is exactly the one that failed — that persistence is the whole reason this
/// is the primary evidence source rather than the journal.
pub const PSTORE_DIR: &str = "/sys/fs/pstore";

/// Is this pstore filename about a crash, rather than routine console output?
///
/// pstore holds `console-ramoops-N` alongside `dmesg-ramoops-N`. The console
/// file exists on every boot and says nothing; the dmesg one exists because
/// something died. Surfacing both equally buries the signal in the file that
/// is always there.
pub fn is_crash_record(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("dmesg-") || n.starts_with("pmsg-") || n.contains("panic")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ab(current: Slot) -> SlotInfo {
        SlotInfo { current: Some(current), count: 2 }
    }

    fn started() -> KernelTry {
        KernelTry::start(&ab(Slot::A)).unwrap()
    }

    #[test]
    fn a_try_targets_the_spare_half_and_remembers_the_good_one() {
        let t = started();
        assert_eq!(t.known_good, Slot::A);
        assert_eq!(t.target(), Slot::B);
        // Recorded at the start, not recomputed: once the device has switched,
        // its own "current" is the slot under test.
        assert_eq!(t.known_good, Slot::A);
    }

    #[test]
    fn a_single_slot_device_has_no_safe_experiment_to_run() {
        assert!(KernelTry::start(&SlotInfo { current: None, count: 1 }).is_none());
        assert!(KernelTry::start(&SlotInfo { current: Some(Slot::A), count: 1 }).is_none());
    }

    #[test]
    fn only_the_device_answering_counts_as_a_boot() {
        // Unreachable-while-booting and unreachable-forever are the same
        // observation until something answers, so silence never means success.
        let mut t = started();
        t.flashed();
        t.rebooted();
        t.tick(30_000);
        assert_eq!(t.next(), Next::Wait);
        assert!(!t.succeeded());
        t.device_answered();
        assert!(t.succeeded());
        assert_eq!(t.state, State::Booted { slot: Slot::B });
    }

    #[test]
    fn a_slow_boot_that_answers_late_is_still_a_success() {
        // The worse mistake is reverting a kernel that worked. A first boot
        // after a flash does fsck and a cold cache; late is not dead.
        let mut t = started();
        t.rebooted();
        t.tick(BOOT_DEADLINE_MS + 1);
        assert_eq!(t.state, State::NoBoot { slot: Slot::B });
        t.device_answered();
        assert!(t.succeeded(), "an answer outranks the deadline");
    }

    #[test]
    fn booting_becomes_no_boot_only_at_the_deadline() {
        let mut t = started();
        t.rebooted();
        t.tick(BOOT_DEADLINE_MS - 1);
        assert!(matches!(t.state, State::Booting { .. }));
        t.tick(1);
        assert_eq!(t.state, State::NoBoot { slot: Slot::B });
        assert_eq!(t.next(), Next::Revert { to: Slot::A });
    }

    #[test]
    fn a_booted_slot_still_has_to_be_confirmed() {
        // The bootloader counts retries and rolls back an unconfirmed slot a
        // few reboots later — the change then appears to stop working for no
        // reason, days after it was made.
        let mut t = started();
        t.rebooted();
        t.device_answered();
        assert_eq!(t.next(), Next::ConfirmSlot(Slot::B));
    }

    #[test]
    fn evidence_is_collected_after_the_revert_and_not_before() {
        // Reading pstore needs a kernel that runs. Asking for it while the
        // dead slot is active asks a machine that is not up.
        let mut t = started();
        t.rebooted();
        t.tick(BOOT_DEADLINE_MS);
        assert_eq!(t.next(), Next::Revert { to: Slot::A }, "revert first");
        t.reverted();
        assert_eq!(t.state, State::Reverted { to: Slot::A, from: Slot::B });
        assert_eq!(t.next(), Next::CollectEvidence { about: Slot::B });
    }

    #[test]
    fn ticking_a_finished_try_does_nothing() {
        // A caller polling on a timer must not be able to walk a completed
        // attempt into another state.
        let mut t = started();
        t.rebooted();
        t.device_answered();
        let before = t.state.clone();
        t.tick(10 * BOOT_DEADLINE_MS);
        assert_eq!(t.state, before);
    }

    #[test]
    fn the_crash_record_is_told_apart_from_the_console_log() {
        // console-ramoops exists on every boot and says nothing; dmesg-ramoops
        // exists because something died. Treating them alike buries the signal
        // in the file that is always there.
        assert!(is_crash_record("dmesg-ramoops-0"));
        assert!(is_crash_record("pmsg-ramoops-0"));
        assert!(is_crash_record("dmesg-efi-panic-1"));
        assert!(!is_crash_record("console-ramoops-0"));
    }

    #[test]
    fn the_summary_says_which_slot_and_how_long() {
        let mut t = started();
        t.rebooted();
        t.tick(45_000);
        assert_eq!(t.summary(), "booting slot b (45s)");
        t.tick(BOOT_DEADLINE_MS);
        assert!(t.summary().contains("did not boot within 120s"));
        t.reverted();
        assert_eq!(t.summary(), "reverted to slot a after b failed");
    }
}
