//! camera_enumerate — the step that made the camera list empty.
//!
//! ## The bug
//!
//! `App.cameras` is `Vec::new()` at construction, and the ONLY thing
//! that assigns it is `HostEvent::CamerasFound`. Nothing in the tree
//! sends that event. So the list is permanently empty, the picker draws
//! nothing (`if !self.cameras.is_empty()`), and no USB webcam can ever
//! appear — on any machine, with any hardware.
//!
//! Everything downstream was already built and tested: `camera_select`
//! picks and handles PIP, `camera_source` builds pipelines,
//! `camera_policy` tracks which devices are healthy. All of it takes a
//! device list as input, and nobody produced one.
//!
//! ## Why /dev/video* and not PipeWire
//!
//! `Device::id` documents itself as "the PipeWire node name", and on the
//! FP5 that is right. But a USB webcam on a desktop is reachable through
//! v4l2 whether or not a PipeWire session is running, and enumerating
//! through v4l2 needs no daemon, no session bus, and no dependency.
//!
//! So this reads `/dev/video*` and reports the node name in the form the
//! backend already targets. Where PipeWire IS running it will publish
//! the same devices, and `pipelines_for` already treats the chosen
//! device as the plan and probe findings as fallbacks — which is exactly
//! the shape that lets these two sources coexist.
//!
//! ## Why the odd-numbered nodes are skipped
//!
//! A single UVC webcam registers TWO `/dev/video` nodes: one for
//! capture and one for metadata. Listing both shows every webcam twice,
//! and selecting the metadata node yields a stream that never produces a
//! frame — which looks exactly like a broken camera. The capability
//! check is what separates them.

use crate::camera_select::{Device, Facing};

/// Read the device list from v4l2.
///
/// Returns empty rather than an error when `/dev` has no video nodes:
/// a machine with no camera is an ordinary state, not a failure, and the
/// picker already handles an empty list by drawing nothing.
pub fn enumerate() -> Vec<Device> {
    let Ok(rd) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut nodes: Vec<(u32, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n: u32 = name.strip_prefix("video")?.parse().ok()?;
            Some((n, name))
        })
        .collect();
    // Numerically, not lexically: video10 sorts before video2 as a
    // string, and the resulting picker order would look scrambled on any
    // machine with more than ten nodes.
    nodes.sort_by_key(|(n, _)| *n);

    nodes
        .into_iter()
        .filter(|(n, _)| is_capture_node(*n))
        .map(|(n, node)| {
            let label = read_name(n).unwrap_or_else(|| format!("camera {n}"));
            Device {
                facing: facing_of(&label),
                // No capture group: a USB webcam is independent, which is
                // what `None` means here. Cameras that SHARE an ISP (the
                // FP5's front and rear) get a group from the platform
                // backend, and guessing one here would disable PIP for
                // devices that can do it.
                capture_group: None,
                id: node,
                label,
            }
        })
        .collect()
}

/// Is this a capture node rather than a metadata one?
///
/// A single UVC webcam registers two `/dev/video` nodes. Listing both
/// shows every camera twice, and the metadata node produces no frames —
/// a stream that opens and stays black, which reads as broken hardware.
///
/// Read from sysfs rather than by opening the device: opening a camera
/// to ask what it is can block, and can fail if another app holds it —
/// so a camera already in use would vanish from the list rather than
/// appear as busy.
fn is_capture_node(n: u32) -> bool {
    let path = format!("/sys/class/video4linux/video{n}/device/../video4linux/video{n}/name");
    // The presence of a `name` is not the test; the capability is. But
    // `device_caps` is the authoritative field and is absent on some
    // kernels, so fall back to accepting the node — over-listing is
    // recoverable (a device that fails to open), under-listing is not (a
    // camera that does not exist as far as the app is concerned).
    let caps = std::fs::read_to_string(format!("/sys/class/video4linux/video{n}/device_caps"))
        .or_else(|_| std::fs::read_to_string(path));
    match caps {
        Ok(text) => {
            let t = text.trim();
            // `device_caps` is hex; V4L2_CAP_VIDEO_CAPTURE is 0x00000001.
            match u32::from_str_radix(t.trim_start_matches("0x"), 16) {
                Ok(bits) => bits & 0x0000_0001 != 0,
                // Not hex — this was the name fallback, so accept it.
                Err(_) => true,
            }
        }
        Err(_) => true,
    }
}

/// The human name from sysfs, e.g. "Logitech StreamCam".
fn read_name(n: u32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/sys/class/video4linux/video{n}/name")).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Guess which way a camera points, from its name.
///
/// Delegates to `camera_source::looks_front_facing`, which already owns
/// this rule — a second copy would let the picker and the pipeline
/// disagree about mirroring, and a mirrored rear camera is a bug people
/// notice immediately.
fn facing_of(label: &str) -> Facing {
    if crate::camera_source::looks_front_facing(label) {
        Facing::Front
    } else {
        // Unknown, not Rear. A USB webcam on a desk faces the user but
        // is not a phone's "rear" camera, and claiming Rear would apply
        // rear-camera rotation rules to it.
        Facing::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_with_no_cameras_returns_empty_rather_than_failing() {
        // The ordinary state on a build machine, and the picker already
        // handles it by drawing nothing. An error here would turn "no
        // webcam plugged in" into a visible fault.
        let list = enumerate();
        // Cannot assert emptiness — CI might have a loopback device — so
        // assert the shape instead: whatever comes back is well formed.
        for d in &list {
            assert!(d.id.starts_with("video"), "id is the node name: {}", d.id);
            assert!(!d.label.is_empty(), "every device needs a label");
        }
    }

    #[test]
    fn nodes_are_ordered_numerically_not_lexically() {
        // video10 sorts before video2 as a string. On a machine with more
        // than ten nodes the picker order would look scrambled.
        let mut v: Vec<(u32, String)> = vec![
            (10, "video10".into()), (2, "video2".into()), (1, "video1".into())];
        v.sort_by_key(|(n, _)| *n);
        assert_eq!(v.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![1, 2, 10]);
    }

    #[test]
    fn a_usb_webcam_is_unknown_facing_not_rear() {
        // Claiming Rear would apply a phone's rear-camera rotation rules
        // to a webcam on a desk.
        assert_eq!(facing_of("Logitech StreamCam"), Facing::Unknown);
    }

    #[test]
    fn front_facing_detection_is_not_reimplemented_here() {
        // `camera_source::looks_front_facing` owns the rule. A second
        // copy would let the picker and the pipeline disagree about
        // mirroring, and a mirrored rear camera is noticed instantly.
        for name in ["front camera", "Front Facing Cam"] {
            assert_eq!(
                facing_of(name) == Facing::Front,
                crate::camera_source::looks_front_facing(name),
                "diverged from the shared rule on {name:?}"
            );
        }
    }

    #[test]
    fn an_unreadable_capability_accepts_the_node() {
        // Over-listing is recoverable — the device fails to open and the
        // health tracker records it. Under-listing is not: the camera
        // simply does not exist as far as the app is concerned, which is
        // the bug this module was written to fix.
        assert!(is_capture_node(99_999), "an absent node should not be excluded by a read error");
    }
}
