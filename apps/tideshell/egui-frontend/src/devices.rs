//! Which way each device's cameras are mounted.
//!
//! Sensor orientation is not discoverable from the frames. libcamera exposes
//! a `Rotation` property only where the kernel driver declares one, and on
//! the phones this shell targets it usually does not — so the picture
//! arrives a quarter turn from the screen with nothing in the stream saying
//! so. Guessing from the image is how you get a preview that is sideways in
//! one orientation and upside down in another.
//!
//! So it is a table, keyed on what the device tree calls the machine, and it
//! grows one measured entry at a time. Three rules keep it honest:
//!
//! * **Entries are measured, never inferred.** An unmeasured device gets no
//!   entry and falls back to no rotation, which is visibly wrong rather than
//!   subtly wrong — a sideways preview is a bug report, a slightly squashed
//!   one is not.
//! * **The environment always wins**, so the value can be found on a new
//!   handset before anyone writes it down here.
//! * **Front cameras mirror.** A selfie preview that does not is disorienting
//!   in the way a non-mirrored shaving mirror is; this is about the preview
//!   only, and has nothing to do with what a capture should store.

use crate::fit::Quarter;

/// How one camera is mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraMount {
    pub rotate: Quarter,
    /// Flip horizontally when previewing.
    pub mirror: bool,
}

impl Default for CameraMount {
    fn default() -> CameraMount {
        CameraMount { rotate: Quarter::None, mirror: false }
    }
}

/// One device's cameras, and how to recognise it.
#[derive(Debug, Clone, Copy)]
pub struct DeviceCameras {
    /// Matched as a substring of the device tree's `model` or `compatible`,
    /// lowercased. Substring rather than equality because vendors decorate
    /// the string differently between kernels.
    pub matches: &'static str,
    pub name: &'static str,
    pub rear: CameraMount,
    pub front: CameraMount,
    /// What was actually checked, so a later reader knows whether to trust
    /// it. Empty means nobody has looked.
    pub note: &'static str,
}

/// The table. Add a row when a device has been looked at, not before.
pub const DEVICES: &[DeviceCameras] = &[DeviceCameras {
    matches: "fairphone 5",
    name: "Fairphone 5 (FP5, QCM6490)",
    // Measured on the lock-screen pane: the preview arrived turned a
    // quarter anticlockwise, so it is drawn back clockwise.
    rear: CameraMount { rotate: Quarter::Cw90, mirror: false },
    front: CameraMount { rotate: Quarter::Cw90, mirror: true },
    note: "rotation seen on the front camera preview, 2026-09; rear assumed the same mounting and NOT yet confirmed",
}];

/// Find a device by its device-tree model string.
pub fn lookup(model: &str) -> Option<&'static DeviceCameras> {
    let m = model.to_ascii_lowercase();
    DEVICES.iter().find(|d| m.contains(d.matches))
}

/// `/proc/device-tree/model`, which is NUL-terminated.
pub fn parse_model(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).trim_end_matches('\0').trim().to_string()
}

/// The mount to use: the environment first, then the table, then nothing.
///
/// `env_rotate` is `TIDESHELL_CAMERA_ROTATE`; it overrides the table so a
/// new handset can be worked out live. A device with no entry and no
/// override gets no rotation rather than a guess.
pub fn mount_for(model: &str, front: bool, env_rotate: Option<&str>, env_mirror: Option<&str>) -> CameraMount {
    let mut m = lookup(model).map(|d| if front { d.front } else { d.rear }).unwrap_or_default();
    if let Some(q) = env_rotate.and_then(Quarter::parse) {
        m.rotate = q;
    }
    if let Some(v) = env_mirror {
        m.mirror = matches!(v.trim(), "1" | "true" | "yes");
    }
    m
}

/// A line for `tideshell sensors` and the like: what was chosen and why,
/// because a silently wrong rotation is hard to attribute.
pub fn describe(model: &str, front: bool, m: CameraMount) -> String {
    match lookup(model) {
        Some(d) => format!("{} {} camera: rotate {:?}, mirror {} — {}", d.name, if front { "front" } else { "rear" }, m.rotate, m.mirror, d.note),
        None => format!("{model:?} is not in the device table: rotate {:?}, mirror {} (set TIDESHELL_CAMERA_ROTATE to correct it, then add a row)", m.rotate, m.mirror),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_tree_string_is_matched_loosely() {
        // Kernels decorate this differently; substring matching survives it.
        for model in ["Fairphone 5", "Fairphone 5 (FP5)", "Qualcomm Technologies, Inc. Fairphone 5 QCM6490"] {
            assert!(lookup(model).is_some(), "{model}");
        }
        assert!(lookup("FAIRPHONE 5").is_some(), "case does not matter");
        assert!(lookup("Fairphone 4").is_none(), "and a different handset is not guessed at");
        assert!(lookup("").is_none());
    }

    #[test]
    fn a_nul_terminated_model_reads_cleanly() {
        assert_eq!(parse_model(b"Fairphone 5\0"), "Fairphone 5");
        assert_eq!(parse_model(b"Fairphone 5\0\0\0"), "Fairphone 5");
        assert_eq!(parse_model(b"  Fairphone 5  "), "Fairphone 5");
        assert_eq!(parse_model(b""), "");
    }

    #[test]
    fn the_front_camera_mirrors_and_the_rear_does_not() {
        let front = mount_for("Fairphone 5", true, None, None);
        let rear = mount_for("Fairphone 5", false, None, None);
        assert!(front.mirror, "a selfie preview that does not mirror is disorienting");
        assert!(!rear.mirror, "the rear camera shows the world as it is");
        assert_eq!(front.rotate, Quarter::Cw90);
    }

    #[test]
    fn an_unknown_device_is_left_alone_rather_than_guessed() {
        let m = mount_for("Some New Phone", false, None, None);
        assert_eq!(m, CameraMount::default());
        assert_eq!(m.rotate, Quarter::None, "visibly wrong beats subtly wrong");
        assert!(describe("Some New Phone", false, m).contains("TIDESHELL_CAMERA_ROTATE"), "and it says how to fix it");
    }

    #[test]
    fn the_environment_beats_the_table() {
        // So a new handset can be worked out live, without a rebuild.
        let m = mount_for("Fairphone 5", true, Some("180"), Some("0"));
        assert_eq!(m.rotate, Quarter::Half);
        assert!(!m.mirror);
        // Nonsense in the environment does not clobber a known-good entry.
        let keep = mount_for("Fairphone 5", true, Some("sideways"), None);
        assert_eq!(keep.rotate, Quarter::Cw90);
        assert!(keep.mirror);
    }

    #[test]
    fn every_row_says_what_was_actually_checked() {
        for d in DEVICES {
            assert!(!d.note.trim().is_empty(), "{}: an entry with no provenance is a guess in disguise", d.name);
            assert_eq!(d.matches, d.matches.to_ascii_lowercase(), "{}: the matcher is compared lowercased", d.name);
            assert!(!d.matches.trim().is_empty());
        }
    }
}
