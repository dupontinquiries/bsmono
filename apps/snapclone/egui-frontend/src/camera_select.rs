//! camera_select — which camera, which way up, and which way round.
//!
//! The egui frontend has no camera view. The capture backend is a shell
//! concern — v4l2, pipewire or libcamera, chosen per platform — but the part
//! that is *not* the backend's job, and the part `CameraWidget.cpp` spends most
//! of its 3,736 lines on, is **deciding**: which node is the front sensor, which
//! way the frames are rotated, and whether to mirror.
//!
//! Porting that first is deliberate. It is the piece that differs per device,
//! it is the piece that has been wrong before, and it is testable with no
//! camera attached — which matters when the targets are a PinePhone, a
//! Fairphone and a desktop that has none.
//!
//! ## Why a rotation override exists at all
//!
//! The Qt comment is the design note: rotation cycles 0/90/180/270 because
//! *"sensor orientation is wrong (per-device DTS issues, opposite-mounted
//! front/rear sensors)"*. On mainline mobile Linux the device tree frequently
//! disagrees with the hardware, so an automatic answer is wrong on some devices
//! and there is no way to detect which. A manual override that PERSISTS is the
//! honest design, and it is why `camera_device` and its rotation belong in the
//! settings file rather than in memory.

/// Which way a sensor faces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facing { Front, Rear, Unknown }

/// A capture device as enumerated by the platform.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub label: String,
    pub facing: Facing,
}

/// Frame rotation applied before display.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rotation { None, Cw90, Half, Ccw90 }

impl Rotation {
    pub fn degrees(self) -> u32 {
        match self { Rotation::None => 0, Rotation::Cw90 => 90,
                     Rotation::Half => 180, Rotation::Ccw90 => 270 }
    }

    /// Cycle, as the Qt button does.
    pub fn next(self) -> Rotation {
        match self { Rotation::None => Rotation::Cw90, Rotation::Cw90 => Rotation::Half,
                     Rotation::Half => Rotation::Ccw90, Rotation::Ccw90 => Rotation::None }
    }

    pub fn from_degrees(d: u32) -> Rotation {
        match d % 360 { 90 => Rotation::Cw90, 180 => Rotation::Half,
                        270 => Rotation::Ccw90, _ => Rotation::None }
    }

    /// Does this rotation swap width and height?
    ///
    /// A preview that reserves the wrong aspect ratio letterboxes into the
    /// wrong axis, which reads as the camera being cropped rather than rotated.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::Cw90 | Rotation::Ccw90)
    }
}

/// What to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    /// One camera fills the view.
    Single { id: String, mirrored: bool },
    /// Rear fills the view with the front inset — the Qt build's PIP mode,
    /// available only when both exist.
    Pip { main: String, inset: String, inset_mirrored: bool },
    /// No usable device.
    None,
}

/// The sentinel the settings file stores for "both cameras at once".
///
/// A reserved id rather than a separate boolean, because the setting answers one
/// question — which camera — and two fields that can disagree ("device = front"
/// plus "pip = true") produce a state nothing can render.
pub const PIP_ID: &str = "@pip";

/// Decide what to display.
///
/// `preferred` is the persisted `camera_device`. An empty string means "no
/// choice made yet", which is different from a choice that has since
/// disappeared — the first should pick a sensible default silently, the second
/// should also fall back, and neither should show an error the user cannot act
/// on.
pub fn select(devices: &[Device], preferred: &str, mirror_front: bool) -> Selection {
    if devices.is_empty() { return Selection::None; }

    let front = devices.iter().find(|d| d.facing == Facing::Front);
    let rear = devices.iter().find(|d| d.facing == Facing::Rear);

    if preferred == PIP_ID {
        if let (Some(f), Some(r)) = (front, rear) {
            return Selection::Pip { main: r.id.clone(), inset: f.id.clone(),
                                    inset_mirrored: mirror_front };
        }
        // Asked for PIP on a device with one camera. Falling back to that
        // camera is better than showing nothing, and better than an error: the
        // setting may have come from a phone and be read on a laptop.
    }

    if let Some(d) = devices.iter().find(|d| d.id == preferred) {
        return Selection::Single { id: d.id.clone(),
                                   mirrored: mirror_front && d.facing == Facing::Front };
    }

    // No stored choice, or it is gone. Prefer the rear sensor: it is what a
    // camera app opens to, and a front-facing default means every launch starts
    // with a picture of the user's face.
    let pick = rear.or(front).unwrap_or(&devices[0]);
    Selection::Single { id: pick.id.clone(),
                        mirrored: mirror_front && pick.facing == Facing::Front }
}

/// Is PIP offerable?
///
/// Asked before showing the option rather than after choosing it — an entry that
/// fails when tapped teaches nothing.
pub fn pip_available(devices: &[Device]) -> bool {
    devices.iter().any(|d| d.facing == Facing::Front)
        && devices.iter().any(|d| d.facing == Facing::Rear)
}

/// Guess a facing from a device label, when the platform does not say.
///
/// v4l2 often reports neither, and the label is the only signal available. This
/// is a guess and named as one: it returns `Unknown` rather than defaulting to
/// Rear, so the caller can pick by position instead of trusting a wrong answer.
pub fn facing_from_label(label: &str) -> Facing {
    let l = label.to_lowercase();
    if l.contains("front") || l.contains("user") || l.contains("selfie") { Facing::Front }
    else if l.contains("rear") || l.contains("back") || l.contains("world")
        || l.contains("main") { Facing::Rear }
    else { Facing::Unknown }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, label: &str, facing: Facing) -> Device {
        Device { id: id.into(), label: label.into(), facing }
    }

    fn phone() -> Vec<Device> {
        vec![dev("/dev/video0", "rear camera", Facing::Rear),
             dev("/dev/video2", "front camera", Facing::Front)]
    }

    #[test]
    fn a_fresh_install_opens_the_rear_camera() {
        // A front-facing default means every launch starts with a picture of
        // the user's face, which is not what a camera app is for.
        match select(&phone(), "", true) {
            Selection::Single { id, mirrored } => {
                assert_eq!(id, "/dev/video0");
                assert!(!mirrored, "the rear sensor is not a mirror");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_front_camera_is_mirrored_and_the_rear_is_not() {
        // Mirroring the rear sensor makes text in the world read backwards.
        match select(&phone(), "/dev/video2", true) {
            Selection::Single { mirrored, .. } => assert!(mirrored),
            other => panic!("{other:?}"),
        }
        match select(&phone(), "/dev/video2", false) {
            Selection::Single { mirrored, .. } => assert!(!mirrored, "the setting is respected"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_saved_device_that_has_gone_away_falls_back_rather_than_failing() {
        // A settings file written on one machine and read on another is
        // ordinary — the same account, a phone and a laptop.
        match select(&phone(), "/dev/video9", true) {
            Selection::Single { id, .. } => assert_eq!(id, "/dev/video0"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pip_needs_both_sensors_and_says_so_before_it_is_chosen() {
        assert!(pip_available(&phone()));
        assert!(!pip_available(&[dev("/dev/video0", "webcam", Facing::Unknown)]));
    }

    #[test]
    fn pip_puts_the_rear_camera_in_the_main_view() {
        match select(&phone(), PIP_ID, true) {
            Selection::Pip { main, inset, inset_mirrored } => {
                assert_eq!(main, "/dev/video0");
                assert_eq!(inset, "/dev/video2");
                assert!(inset_mirrored, "the inset is the selfie");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pip_requested_on_a_single_camera_machine_shows_that_camera() {
        // The setting may have come from a phone and be read on a laptop.
        // Falling back beats an error the user cannot act on.
        let laptop = vec![dev("/dev/video0", "integrated webcam", Facing::Front)];
        match select(&laptop, PIP_ID, true) {
            Selection::Single { id, mirrored } => {
                assert_eq!(id, "/dev/video0");
                assert!(mirrored, "an integrated webcam faces the user");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_cameras_is_a_state_rather_than_a_panic() {
        // Desktops without a webcam are the common development case.
        assert_eq!(select(&[], "", true), Selection::None);
        assert!(!pip_available(&[]));
    }

    #[test]
    fn rotation_cycles_through_all_four_and_returns() {
        let mut r = Rotation::None;
        let mut seen = vec![r];
        for _ in 0..3 { r = r.next(); seen.push(r); }
        assert_eq!(seen.len(), 4);
        assert_eq!(r.next(), Rotation::None, "cycles back");
        assert_eq!(seen.iter().map(|x| x.degrees()).collect::<Vec<_>>(), vec![0, 90, 180, 270]);
    }

    #[test]
    fn a_quarter_turn_swaps_the_preview_aspect_ratio() {
        // A preview reserving the wrong ratio letterboxes into the wrong axis,
        // which reads as the camera being cropped rather than rotated.
        assert!(Rotation::Cw90.swaps_axes());
        assert!(Rotation::Ccw90.swaps_axes());
        assert!(!Rotation::None.swaps_axes());
        assert!(!Rotation::Half.swaps_axes());
    }

    #[test]
    fn a_persisted_rotation_survives_any_stored_number() {
        // The override is persisted, so the file can hold anything a previous
        // version or a hand edit wrote.
        assert_eq!(Rotation::from_degrees(90), Rotation::Cw90);
        assert_eq!(Rotation::from_degrees(450), Rotation::Cw90, "wraps");
        assert_eq!(Rotation::from_degrees(37), Rotation::None, "nonsense reads as none");
        assert_eq!(Rotation::from_degrees(0), Rotation::None);
    }

    #[test]
    fn an_unlabelled_sensor_is_unknown_rather_than_guessed_as_rear() {
        // v4l2 often reports neither facing. Defaulting to Rear would mirror
        // nothing on a laptop webcam, which is the wrong answer for most of
        // them.
        assert_eq!(facing_from_label("Integrated_Webcam_HD"), Facing::Unknown);
        assert_eq!(facing_from_label("front camera"), Facing::Front);
        assert_eq!(facing_from_label("Selfie Cam"), Facing::Front);
        assert_eq!(facing_from_label("rear-facing"), Facing::Rear);
        assert_eq!(facing_from_label("world camera"), Facing::Rear);
    }

    #[test]
    fn an_all_unknown_device_list_still_picks_something() {
        // A laptop with two unlabelled nodes must show one, not nothing.
        let unknown = vec![dev("/dev/video0", "cam0", Facing::Unknown),
                           dev("/dev/video1", "cam1", Facing::Unknown)];
        match select(&unknown, "", true) {
            Selection::Single { id, mirrored } => {
                assert_eq!(id, "/dev/video0");
                assert!(!mirrored, "unknown facing is not assumed to be a selfie");
            }
            other => panic!("{other:?}"),
        }
    }
}
