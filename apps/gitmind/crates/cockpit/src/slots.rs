//! slots — A/B, and why it is the feature that makes kernel work iterable.
//!
//! On an A/B device every slotted partition exists twice: `boot_a` and
//! `boot_b`. One slot is active; the other is a spare you can write to while
//! the running system is untouched. The loop that buys:
//!
//! 1. Flash the new kernel to the **inactive** slot.
//! 2. `set_active` to it and reboot.
//! 3. It boots → keep going. It does not → the bootloader falls back, or you
//!    switch back yourself, and the known-good kernel is still there.
//!
//! Step 3 is the point. Without it a bad kernel costs a full reflash and
//! twenty minutes; with it, the cost of trying something is a reboot. That is
//! the difference between a kernel change being an experiment and being an
//! event, and it is why this is worth having before any of the convenience
//! features.
//!
//! ## What the existing scripts do not handle
//!
//! The flash scripts in `spec-fp5-boot.json` and webflash's device table name
//! bare partitions — `boot`, `userdata` — with no slot suffix. On an A/B
//! device fastboot resolves a bare name to the **active** slot, so those
//! scripts overwrite the running system. That is correct for "install the OS"
//! and exactly wrong for "try a kernel", and nothing in the format
//! distinguishes the two. Hence this module rather than a flag.
//!
//! ## Not every partition is slotted
//!
//! `userdata` has no `_a`. Suffixing it produces a partition that does not
//! exist, and fastboot's error for that is not always loud. The device knows
//! the answer — `getvar:has-slot:<partition>` — so it is asked rather than
//! assumed.

/// Which slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    /// The suffix fastboot expects.
    pub fn suffix(self) -> &'static str {
        match self {
            Slot::A => "_a",
            Slot::B => "_b",
        }
    }

    /// The one that is not this one — the target of a kernel experiment.
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }

    /// What `set_active` takes: the bare letter, not the suffix.
    pub fn letter(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
        }
    }

    /// Parse what `getvar:current-slot` reports.
    ///
    /// Devices answer variously `a`, `_a` or `slot_a`, so all three are
    /// accepted. Anything else returns None rather than defaulting to A —
    /// guessing the slot means guessing which half of the device to overwrite.
    pub fn parse(s: &str) -> Option<Slot> {
        match s.trim().trim_start_matches("slot").trim_start_matches('_') {
            "a" | "A" => Some(Slot::A),
            "b" | "B" => Some(Slot::B),
            _ => None,
        }
    }
}

/// What the device said about its slots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SlotInfo {
    /// `getvar:current-slot`, parsed. None on a device that is not A/B.
    pub current: Option<Slot>,
    /// `getvar:slot-count`. 1 or absent means no A/B.
    pub count: u32,
}

impl SlotInfo {
    pub fn is_ab(&self) -> bool {
        self.count >= 2 && self.current.is_some()
    }

    /// Where a kernel experiment should be written.
    ///
    /// None on a device without slots — and the caller must NOT fall back to
    /// the active slot, because the whole safety property is that the running
    /// kernel survives the attempt.
    pub fn target(&self) -> Option<Slot> {
        if !self.is_ab() {
            return None;
        }
        self.current.map(|c| c.other())
    }
}

/// Apply the slot suffix to a partition name, if that partition has slots.
///
/// `has_slot` is the device's own answer to `getvar:has-slot:<partition>`.
/// Asked rather than assumed: `userdata` is unslotted on the same device where
/// `boot` is slotted, and a suffixed name for an unslotted partition targets
/// something that does not exist.
pub fn partition_for(partition: &str, slot: Option<Slot>, has_slot: bool) -> String {
    // Already suffixed by the caller: leave it. A script that says `boot_b`
    // means `boot_b`, and appending again would produce `boot_b_b`.
    if partition.ends_with("_a") || partition.ends_with("_b") {
        return partition.to_string();
    }
    match (slot, has_slot) {
        (Some(s), true) => format!("{partition}{}", s.suffix()),
        _ => partition.to_string(),
    }
}

/// A step in a kernel experiment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotStep {
    /// `flash:<partition>` with the image.
    Flash { partition: String },
    /// `set_active:<letter>`.
    SetActive(Slot),
    Reboot,
}

/// Why a kernel experiment could not be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotError {
    /// Single-slot device. Flashing a kernel here overwrites the only one.
    NotAb,
    /// The device reported a slot we could not parse.
    UnknownSlot(String),
}

/// Plan a kernel try: write to the spare slot, switch, reboot.
///
/// Deliberately refuses on a non-A/B device rather than falling back to the
/// active slot. On a single-slot phone this operation is "overwrite the only
/// kernel you have", which is a different act with a different risk, and the
/// caller should have to ask for it explicitly.
pub fn plan_kernel_try(info: &SlotInfo, partition: &str) -> Result<Vec<SlotStep>, SlotError> {
    let target = info.target().ok_or(SlotError::NotAb)?;
    Ok(vec![
        // Kernel partitions are slotted by definition on an A/B device, so
        // has_slot is true here; a caller with an odd partition should use
        // partition_for directly.
        SlotStep::Flash { partition: partition_for(partition, Some(target), true) },
        SlotStep::SetActive(target),
        SlotStep::Reboot,
    ])
}

/// Plan the way back after a kernel that did not boot.
///
/// Just a slot switch: the old kernel was never overwritten, which is the
/// entire point. No reflash, no image, no download.
pub fn plan_revert(info: &SlotInfo) -> Result<Vec<SlotStep>, SlotError> {
    let current = info.current.ok_or(SlotError::NotAb)?;
    if !info.is_ab() {
        return Err(SlotError::NotAb);
    }
    Ok(vec![SlotStep::SetActive(current.other()), SlotStep::Reboot])
}

/// The `getvar` names needed to fill a `SlotInfo`.
pub const SLOT_QUERIES: [&str; 2] = ["current-slot", "slot-count"];

/// The `getvar` for whether one partition is slotted.
pub fn has_slot_query(partition: &str) -> String {
    format!("has-slot:{partition}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ab(current: Slot) -> SlotInfo {
        SlotInfo { current: Some(current), count: 2 }
    }

    #[test]
    fn a_kernel_try_never_touches_the_running_slot() {
        // The whole safety property: the kernel you are currently booted from
        // is still there when the new one fails.
        let plan = plan_kernel_try(&ab(Slot::A), "boot").unwrap();
        assert_eq!(plan, vec![
            SlotStep::Flash { partition: "boot_b".into() },
            SlotStep::SetActive(Slot::B),
            SlotStep::Reboot,
        ]);
        let plan = plan_kernel_try(&ab(Slot::B), "boot").unwrap();
        assert_eq!(plan[0], SlotStep::Flash { partition: "boot_a".into() });
        assert_eq!(plan[1], SlotStep::SetActive(Slot::A));
    }

    #[test]
    fn reverting_is_a_switch_and_not_a_reflash() {
        // Nothing was overwritten, so coming back costs a reboot rather than
        // a download and a flash. This is what makes the loop cheap enough to
        // run all afternoon.
        let plan = plan_revert(&ab(Slot::B)).unwrap();
        assert_eq!(plan, vec![SlotStep::SetActive(Slot::A), SlotStep::Reboot]);
        assert!(!plan.iter().any(|s| matches!(s, SlotStep::Flash { .. })));
    }

    #[test]
    fn a_single_slot_device_is_refused_rather_than_silently_overwritten() {
        // Falling back to the active slot here would turn "try a kernel" into
        // "replace your only kernel" without the caller asking for it.
        let single = SlotInfo { current: None, count: 1 };
        assert_eq!(plan_kernel_try(&single, "boot"), Err(SlotError::NotAb));
        assert_eq!(plan_revert(&single), Err(SlotError::NotAb));
        // A device claiming a slot but only one of them is not A/B either.
        let odd = SlotInfo { current: Some(Slot::A), count: 1 };
        assert!(!odd.is_ab());
        assert_eq!(plan_kernel_try(&odd, "boot"), Err(SlotError::NotAb));
    }

    #[test]
    fn unslotted_partitions_do_not_get_a_suffix() {
        // userdata has no _a on the same device where boot does. Suffixing it
        // names a partition that does not exist, and fastboot is not reliably
        // loud about that.
        assert_eq!(partition_for("boot", Some(Slot::B), true), "boot_b");
        assert_eq!(partition_for("userdata", Some(Slot::B), false), "userdata");
        // No slot known: leave the name alone and let fastboot resolve it.
        assert_eq!(partition_for("boot", None, true), "boot");
    }

    #[test]
    fn an_already_suffixed_partition_is_left_alone() {
        // A script that says boot_b means boot_b; appending again gives
        // boot_b_b, which resolves to nothing.
        assert_eq!(partition_for("boot_b", Some(Slot::A), true), "boot_b");
        assert_eq!(partition_for("boot_a", Some(Slot::B), true), "boot_a");
    }

    #[test]
    fn the_devices_various_spellings_of_a_slot_all_parse() {
        for s in ["a", "_a", "slot_a", " a ", "A"] {
            assert_eq!(Slot::parse(s), Some(Slot::A), "failed on {s:?}");
        }
        for s in ["b", "_b", "slot_b"] {
            assert_eq!(Slot::parse(s), Some(Slot::B), "failed on {s:?}");
        }
        // Anything else is None, not a default: guessing the slot means
        // guessing which half of the device to overwrite.
        assert_eq!(Slot::parse(""), None);
        assert_eq!(Slot::parse("c"), None);
        assert_eq!(Slot::parse("unknown"), None);
    }

    #[test]
    fn set_active_takes_the_letter_and_flash_takes_the_suffix() {
        // Two different spellings of the same slot in one exchange, and
        // mixing them up is a command the device rejects at best.
        assert_eq!(Slot::B.letter(), "b");
        assert_eq!(Slot::B.suffix(), "_b");
        let plan = plan_kernel_try(&ab(Slot::A), "boot").unwrap();
        match (&plan[0], &plan[1]) {
            (SlotStep::Flash { partition }, SlotStep::SetActive(s)) => {
                assert!(partition.ends_with("_b"));
                assert_eq!(s.letter(), "b");
            }
            _ => panic!("unexpected plan shape"),
        }
    }
}
