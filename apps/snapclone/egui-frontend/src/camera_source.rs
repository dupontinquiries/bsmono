//! camera_source — pick a real camera, instead of drawing a rectangle.
//!
//! `shared/device-kit`'s own description names *"snapclone camera-bridge"* as
//! its first consumer, and no port had ever depended on it. The viewfinder was
//! drawing a placeholder while the crate written for exactly this sat unused —
//! the same pattern as the graph reaching for empty slices while `nifty-core`
//! was already a dependency.
//!
//! This is the selection layer: which device, at which format. Capture itself
//! stays in the host, because a UI crate that opens `/dev/video0` cannot be
//! tested and cannot run on a machine without one.
//!
//! ## What device-kit already decides, and why not to re-decide it
//!
//! `pick_video_format` restricts to a matching fourcc *if any match*, then takes
//! the closest area, then prefers fps at or below what was asked. Those are real
//! decisions — asking for 1080p30 on a camera that only does 720p60 should give
//! 720p, not fail — and duplicating them in the frontend would let the two drift
//! into disagreeing about which camera the user is looking through.

pub use device_kit::{Device, DeviceKind, VideoFormat};

/// What the viewfinder needs to know about its input.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraChoice {
    pub device: Device,
    pub format: VideoFormat,
    /// True for a front/selfie camera, which the viewfinder mirrors. Derived
    /// from the name because that is the only signal V4L2 gives — there is no
    /// facing flag, and every phone-shaped device labels it.
    pub front: bool,
}

/// Names that mark a front-facing camera. Matched case-insensitively as
/// substrings, because the full name is usually a vendor string with the useful
/// word buried in it.
const FRONT_MARKERS: [&str; 4] = ["front", "user", "selfie", "facetime"];

pub fn looks_front_facing(name: &str) -> bool {
    let n = name.to_lowercase();
    FRONT_MARKERS.iter().any(|m| n.contains(m))
}

/// Choose a camera and a format.
///
/// `prefer` is a saved device name from settings. Returns `None` when there is
/// no camera at all — which the caller must show as "no camera" rather than an
/// empty preview, since a black rectangle and a missing device look identical.
pub fn choose(devices: &[Device], supported: &[VideoFormat],
              desired: &VideoFormat, prefer: Option<&str>) -> Option<CameraChoice> {
    let device = device_kit::pick_device(devices, DeviceKind::VideoCapture, prefer)?.clone();
    // Fall back to the desired format only when the device reports none: a
    // camera with no advertised formats still usually works at its default, and
    // refusing to open it would be worse than trying.
    let format = device_kit::pick_video_format(supported, desired)
        .unwrap_or_else(|| desired.clone());
    let front = looks_front_facing(&device.name);
    Some(CameraChoice { device, format, front })
}

/// A one-line description for the settings row: "HD Webcam — 1280×720 @30".
pub fn describe(c: &CameraChoice) -> String {
    format!("{} — {}×{} @{}", c.device.name, c.format.width, c.format.height, c.format.fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str) -> Device {
        Device::new(DeviceKind::VideoCapture, id, name, &format!("/dev/video{id}"))
    }

    fn fmt(w: u32, h: u32, fps: u32) -> VideoFormat { VideoFormat::new(w, h, fps, "YUYV") }

    #[test]
    fn no_camera_is_reported_rather_than_shown_as_an_empty_preview() {
        // A black rectangle and a missing device look identical, which is the
        // placeholder failure in its original form.
        assert!(choose(&[], &[fmt(1280, 720, 30)], &fmt(1280, 720, 30), None).is_none());
    }

    #[test]
    fn a_saved_device_name_wins_over_the_first_camera() {
        let devs = vec![dev("0", "Integrated Webcam"), dev("1", "HD Pro C920")];
        let c = choose(&devs, &[fmt(1280, 720, 30)], &fmt(1280, 720, 30), Some("c920"))
            .expect("a camera");
        assert_eq!(c.device.id, "1", "the preference is honoured case-insensitively");
        let d = choose(&devs, &[fmt(1280, 720, 30)], &fmt(1280, 720, 30), None).unwrap();
        assert_eq!(d.device.id, "0", "and without one, the first");
    }

    #[test]
    fn a_preference_that_no_longer_matches_still_gives_a_camera() {
        // Unplugging the preferred webcam must not leave the app with no
        // preview at all — device-kit falls back to the first of the kind.
        let devs = vec![dev("0", "Integrated Webcam")];
        let c = choose(&devs, &[fmt(640, 480, 30)], &fmt(640, 480, 30), Some("c920"));
        assert!(c.is_some(), "falls back rather than failing");
    }

    #[test]
    fn the_format_comes_from_device_kits_negotiation() {
        // Asking for 1080p30 on a camera that only does 720p60 should give
        // 720p, not fail. Re-deciding this in the frontend would let the two
        // drift into disagreeing about what the user is looking through.
        let devs = vec![dev("0", "Cam")];
        let supported = vec![fmt(1280, 720, 60), fmt(320, 240, 30)];
        let c = choose(&devs, &supported, &fmt(1920, 1080, 30), None).unwrap();
        assert_eq!((c.format.width, c.format.height), (1280, 720));
    }

    #[test]
    fn a_camera_advertising_no_formats_is_still_opened() {
        // It usually works at its default, and refusing would be worse than
        // trying.
        let devs = vec![dev("0", "Cam")];
        let want = fmt(640, 480, 30);
        let c = choose(&devs, &[], &want, None).unwrap();
        assert_eq!(c.format, want);
    }

    #[test]
    fn front_facing_is_read_from_the_name_because_v4l2_has_no_flag() {
        assert!(looks_front_facing("Integrated User Facing Camera"));
        assert!(looks_front_facing("FaceTime HD"));
        assert!(looks_front_facing("front camera"));
        assert!(!looks_front_facing("HD Pro Webcam C920"));
        // and the choice carries it, so the viewfinder mirrors the right one
        let c = choose(&[dev("0", "Front Camera")], &[fmt(640, 480, 30)],
                       &fmt(640, 480, 30), None).unwrap();
        assert!(c.front);
    }

    #[test]
    fn the_settings_row_reads_like_a_camera_not_a_struct() {
        let c = choose(&[dev("0", "HD Webcam")], &[fmt(1280, 720, 30)],
                       &fmt(1280, 720, 30), None).unwrap();
        assert_eq!(describe(&c), "HD Webcam — 1280×720 @30");
    }
}
