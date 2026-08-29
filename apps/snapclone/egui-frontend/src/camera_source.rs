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

/// The ordered pipelines to try for a chosen camera, narrowest guess first.
///
/// `choose` above answers WHICH DEVICE; this answers HOW TO OPEN IT, and
/// they are different questions with different answers per platform. The
/// viewfinder needs both and until now had only the first: a `Device` with
/// a name and a path, and no notion that a phone wants pipewire/libcamera
/// tried before v4l2 while a desktop has only v4l2 and no libcamera nodes
/// at all. That gap is why `device_kit::source_plan` was hoisted out of
/// camera-bridge — this is the consumer it was hoisted FOR.
///
/// The chosen device's path becomes the override, so the plan leads with
/// the camera the person actually picked in settings rather than with
/// whatever the probe happened to enumerate first. Everything else the
/// probe found stays in the list behind it, because a saved preference
/// pointing at an unplugged webcam must degrade to "try the others"
/// rather than to nothing.
///
/// `front` cameras get a horizontal flip, matching the mirrored preview
/// the viewfinder already draws for them — a selfie preview that is
/// mirrored on screen and unmirrored in the pipeline would record the
/// opposite of what the person saw.
pub fn pipelines_for(choice: &CameraChoice, pw_nodes: &[String],
                     v4l2_list_output: &str) -> Vec<device_kit::source_plan::SourceCandidate> {
    let flip = if choice.front { Some("horizontal-flip") } else { None };
    device_kit::source_plan::plan(pw_nodes, v4l2_list_output, Some(&choice.device.path), flip)
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

    const DESKTOP_V4L2: &str = "\
Integrated Camera: Integrated C (usb-0000:00:14.0-8):\n\
\t/dev/video0\n\
\t/dev/video1\n";

    #[test]
    fn a_desktop_gets_a_v4l2_pipeline_where_it_used_to_get_nothing() {
        // The gap this closes: `choose` returned a device and the
        // viewfinder had no idea how to open it. A desktop has no
        // libcamera nodes at all, so a pipewire-only path — which is what
        // the phone-first code assumed — finds nothing on it.
        let c = choose(&[dev("0", "Integrated Camera")], &[], &fmt(1280, 720, 30), None)
            .expect("a camera");
        let plan = pipelines_for(&c, &[], DESKTOP_V4L2);
        assert!(!plan.is_empty(), "a desktop webcam produced no pipeline at all");
        assert_eq!(plan[0].kind, device_kit::source_plan::SourceKind::V4l2);
        assert!(plan[0].pipeline.contains("device=/dev/video0"), "{:?}", plan[0]);
    }

    #[test]
    fn the_chosen_device_leads_and_the_others_stay_as_fallbacks() {
        // A saved preference pointing at an unplugged webcam must degrade
        // to "try the others", not to nothing — so the choice leads the
        // list rather than replacing it.
        let c = choose(&[dev("1", "Second Camera")], &[], &fmt(640, 480, 30), None)
            .expect("a camera");
        let plan = pipelines_for(&c, &[], DESKTOP_V4L2);
        assert!(plan[0].pipeline.contains("device=/dev/video1"), "chosen first: {:?}", plan[0]);
        assert!(plan.len() > 1, "the probe's own findings remain as fallbacks: {plan:?}");
    }

    #[test]
    fn a_phone_prefers_libcamera_but_still_carries_v4l2() {
        // Same call, different machine: the libcamera node a phone
        // reports outranks v4l2, which stays behind it rather than being
        // dropped.
        let c = choose(&[dev("0", "Integrated Camera")], &[], &fmt(1280, 720, 30), None)
            .expect("a camera");
        let plan = pipelines_for(&c, &["libcamera_input_2_1".to_string()], DESKTOP_V4L2);
        let kinds: Vec<_> = plan.iter().map(|p| p.kind).collect();
        assert!(kinds.contains(&device_kit::source_plan::SourceKind::PipewireLibcamera),
                "the phone's own node is missing: {plan:?}");
        assert!(kinds.contains(&device_kit::source_plan::SourceKind::V4l2),
                "v4l2 was dropped rather than kept as a fallback: {plan:?}");
    }

    #[test]
    fn a_front_camera_flips_the_pipeline_to_match_the_mirrored_preview() {
        // The viewfinder already mirrors a selfie preview. If the pipeline
        // does not flip too, the recording is the opposite of what the
        // person watched themselves do.
        let front = choose(&[dev("2", "Integrated Front Camera")], &[], &fmt(640, 480, 30), None)
            .expect("a camera");
        assert!(front.front, "the name should read as front-facing");
        let plan = pipelines_for(&front, &[], DESKTOP_V4L2);
        assert!(plan[0].pipeline.contains("videoflip"), "{:?}", plan[0]);

        let back = choose(&[dev("0", "Integrated Camera")], &[], &fmt(640, 480, 30), None)
            .expect("a camera");
        let plan = pipelines_for(&back, &[], DESKTOP_V4L2);
        assert!(!plan[0].pipeline.contains("videoflip"), "a rear camera must not be flipped");
    }

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
