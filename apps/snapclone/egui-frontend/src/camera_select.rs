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
///
/// `id` is the **PipeWire node name**, which is what the backend targets with
/// `pipewiresrc target-object=<node>` — not a `/dev/videoN` path. Those still
/// appear in a settings file written by an older build, and `select` falls back
/// rather than failing when one no longer resolves.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub label: String,
    pub facing: Facing,
    /// Sources sharing a group cannot be live at the same time. `None` means
    /// freely concurrent — an independent USB webcam.
    ///
    /// This is `snapclone_videosrc::CaptureGroup`, and it is the reason PIP has
    /// to be checked rather than assumed: on the FP5 the front and rear cameras
    /// share one ISP, so `pipewiresrc` on the second one fails to negotiate. The
    /// Qt build offered PIP anyway and it silently did not work on device.
    pub capture_group: Option<u32>,
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
            if concurrent(f, r) {
                return Selection::Pip { main: r.id.clone(), inset: f.id.clone(),
                                        inset_mirrored: mirror_front };
            }
            // Two cameras that cannot run together. Falling through to the rear
            // sensor shows something rather than a black inset over a working
            // main view, which is what the Qt build did.
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

/// The camera facing the other way from `current`, if there is one.
///
/// What a double-tap on the viewfinder switches to. The FIRST device of
/// the other facing, in the session's order: the bridge's virtual node
/// ranks ahead of the raw sensor it holds (`camera_enumerate`), so the
/// flip lands on the one that opens. A device facing `Unknown` is never
/// the answer — a USB webcam is not the back of a phone.
pub fn other_facing<'a>(devices: &'a [Device], current: &str) -> Option<&'a Device> {
    let cur = devices.iter().find(|d| d.id == current)?;
    let want = match cur.facing {
        Facing::Front => Facing::Rear,
        Facing::Rear => Facing::Front,
        _ => return None,
    };
    devices.iter().find(|d| d.facing == want)
}

/// Is PIP offerable?
///
/// Two sensors is necessary and NOT sufficient. They must also be able to run
/// at once: on a phone whose front and rear cameras share an ISP they cannot,
/// and the second `pipewiresrc` fails to negotiate. The Qt build checked only
/// for two cameras, offered PIP, and it silently did nothing on the FP5.
///
/// Asked before showing the option rather than after choosing it — an entry
/// that fails when tapped teaches nothing.
pub fn pip_available(devices: &[Device]) -> bool {
    let (Some(f), Some(r)) = (devices.iter().find(|d| d.facing == Facing::Front),
                              devices.iter().find(|d| d.facing == Facing::Rear))
    else { return false };
    concurrent(f, r)
}

/// Can these two run at the same time?
///
/// Mirrors `snapclone_videosrc::can_run_simultaneously` for the two-source case.
/// `None` is freely concurrent; equal `Some` ids share exclusive hardware.
pub fn concurrent(a: &Device, b: &Device) -> bool {
    match (a.capture_group, b.capture_group) {
        (Some(x), Some(y)) => x != y,
        _ => true,
    }
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

/// Build a [`Device`] from a PipeWire node's properties.
///
/// The backend enumerates nodes and hands over their property dictionary; this
/// turns one into a device or rejects it. Written against the property NAMES
/// rather than a binding type, so it is testable with no PipeWire running and
/// so a portal-provided node (which arrives with the same properties through a
/// different route) needs no second code path.
///
/// ## Which nodes are cameras
///
/// `media.class` must contain `Video/Source`. A sink is not a camera, and
/// neither is `Stream/Output/Video` — that is another application's output,
/// and offering it as a camera is how a video call ends up showing itself.
pub fn device_from_node(props: &[(&str, &str)]) -> Option<Device> {
    let get = |k: &str| props.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);

    let class = get("media.class")?;
    if !class.contains("Video/Source") { return None; }

    // node.name is the handle `pipewiresrc target-object` takes. Without it the
    // device cannot be opened, so a node lacking one is not offerable however
    // well it is described.
    let id = get("node.name")?;
    if id.is_empty() { return None; }

    let label = get("node.description").or_else(|| get("node.nick")).unwrap_or(id);

    // PipeWire may state the facing outright; otherwise fall back to the label,
    // which is a guess and returns Unknown when it cannot tell.
    // Location arrives in THREE spellings and GNOME Snapshot's `aperture` crate
    // matches all of them: "Front"|"front"|"0" and "Back"|"back"|"1". My first
    // version took lowercase only, so a capitalised or numeric location — which
    // is what libcamera actually reports on some pipelines — fell through to
    // guessing from the label. That is a silent downgrade from a stated fact to
    // an inference, and it un-mirrors a selfie.
    let facing = match get("api.libcamera.location").or_else(|| get("device.location")) {
        Some(v) => match v.trim() {
            "Front" | "front" | "0" => Facing::Front,
            "Back" | "back" | "rear" | "Rear" | "1" => Facing::Rear,
            // A location we do not recognise is NOT a reason to trust the
            // label: the platform stated something and we failed to read it,
            // which is worth being unknown about rather than guessing over.
            _ => Facing::Unknown,
        },
        None => facing_from_label(label),
    };

    // Sensors behind one ISP are grouped by their DEVICE id: libcamera exposes
    // both phone cameras under one device, which is exactly the exclusivity
    // that breaks PIP. A USB webcam is its own device and so its own group,
    // which is the behaviour we want without special-casing it.
    let capture_group = get("device.id").and_then(|d| d.parse::<u32>().ok());

    Some(Device { id: id.to_string(), label: label.to_string(), facing, capture_group })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, label: &str, facing: Facing) -> Device {
        Device { id: id.into(), label: label.into(), facing, capture_group: None }
    }

    #[test]
    fn the_other_facing_is_the_first_of_that_facing_and_never_unknown() {
        let ds = vec![dev("virtual-cam-front", "bridge front", Facing::Front),
                      dev("raw-rear", "sensor", Facing::Rear),
                      dev("virtual-cam-rear", "bridge rear", Facing::Rear),
                      dev("usb", "webcam", Facing::Unknown)];
        assert_eq!(other_facing(&ds, "virtual-cam-front").map(|d| d.id.as_str()), Some("raw-rear"));
        assert_eq!(other_facing(&ds, "raw-rear").map(|d| d.id.as_str()), Some("virtual-cam-front"));
        assert!(other_facing(&ds, "usb").is_none(), "a webcam has no other side");
        assert!(other_facing(&ds, "nope").is_none());
        let one = vec![dev("front", "f", Facing::Front)];
        assert!(other_facing(&one, "front").is_none(), "one camera flips to nothing");
    }

    fn shared(id: &str, label: &str, facing: Facing, group: u32) -> Device {
        Device { id: id.into(), label: label.into(), facing, capture_group: Some(group) }
    }

    /// A laptop-ish pair: two independent nodes, freely concurrent.
    fn phone() -> Vec<Device> {
        vec![dev("/dev/video0", "rear camera", Facing::Rear),
             dev("/dev/video2", "front camera", Facing::Front)]
    }

    /// An FP5: both sensors behind one ISP.
    fn fp5() -> Vec<Device> {
        vec![shared("camera-rear", "rear camera", Facing::Rear, 1),
             shared("camera-front", "front camera", Facing::Front, 1)]
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
    fn pip_is_refused_when_the_two_sensors_share_an_isp() {
        // The FP5 case the Qt build got wrong: it checked for two cameras,
        // offered PIP, and the second pipewiresrc failed to negotiate — so the
        // feature silently did nothing on device.
        assert!(pip_available(&phone()), "independent nodes can pair");
        assert!(!pip_available(&fp5()), "one ISP cannot serve both");
    }

    #[test]
    fn pip_requested_on_an_isp_shared_phone_shows_the_rear_camera() {
        // Something rather than a black inset over a working main view.
        match select(&fp5(), PIP_ID, true) {
            Selection::Single { id, mirrored } => {
                assert_eq!(id, "camera-rear");
                assert!(!mirrored);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_independent_webcams_are_concurrent_even_in_the_same_group_position() {
        // None means freely concurrent, and must not be confused with group 0 —
        // which is why the type is Option<u32> rather than a bare id.
        let a = dev("usb-a", "cam", Facing::Unknown);
        let b = dev("usb-b", "cam", Facing::Unknown);
        assert!(concurrent(&a, &b));
        let g0a = shared("a", "cam", Facing::Unknown, 0);
        let g0b = shared("b", "cam", Facing::Unknown, 0);
        assert!(!concurrent(&g0a, &g0b), "group 0 is a real group");
    }

    #[test]
    fn a_grouped_and_an_ungrouped_source_can_run_together() {
        // A phone camera plus a screenshare stream, which is the common case
        // for "share my screen and my face".
        let cam = shared("camera-front", "front", Facing::Front, 1);
        let usb = dev("usb-webcam", "webcam", Facing::Unknown);
        assert!(concurrent(&cam, &usb));
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
    fn a_pipewire_camera_node_becomes_a_device() {
        let d = device_from_node(&[
            ("media.class", "Video/Source"),
            ("node.name", "camera-rear"),
            ("node.description", "Rear Camera"),
            ("api.libcamera.location", "back"),
            ("device.id", "42"),
        ]).expect("a camera");
        assert_eq!(d.id, "camera-rear");
        assert_eq!(d.label, "Rear Camera");
        assert_eq!(d.facing, Facing::Rear);
        assert_eq!(d.capture_group, Some(42));
    }

    #[test]
    fn another_applications_video_output_is_not_a_camera() {
        // Stream/Output/Video is somebody else's output, and offering it as a
        // camera is how a video call ends up showing itself.
        assert!(device_from_node(&[
            ("media.class", "Stream/Output/Video"),
            ("node.name", "some-app"),
        ]).is_none());
        assert!(device_from_node(&[
            ("media.class", "Audio/Source"),
            ("node.name", "mic"),
        ]).is_none());
    }

    #[test]
    fn a_node_without_a_name_cannot_be_opened_so_it_is_not_offered() {
        // node.name is the handle pipewiresrc target-object takes.
        assert!(device_from_node(&[
            ("media.class", "Video/Source"),
            ("node.description", "Nameless Camera"),
        ]).is_none());
    }

    #[test]
    fn every_spelling_of_location_is_read() {
        // GNOME Snapshot's aperture crate matches "Front"|"front"|"0" and
        // "Back"|"back"|"1" — libcamera reports capitalised and numeric forms
        // on some pipelines. Taking lowercase only silently downgrades a stated
        // fact to a label guess, which un-mirrors a selfie.
        for (v, want) in [("front", Facing::Front), ("Front", Facing::Front), ("0", Facing::Front),
                          ("back", Facing::Rear), ("Back", Facing::Rear), ("1", Facing::Rear)] {
            let d = device_from_node(&[("media.class", "Video/Source"),
                ("node.name", "cam"), ("api.libcamera.location", v)]).unwrap();
            assert_eq!(d.facing, want, "location {v:?}");
        }
    }

    #[test]
    fn an_unrecognised_location_is_unknown_rather_than_a_label_guess() {
        // The platform stated something and we failed to read it. Falling back
        // to the label would paper over a real gap in this match.
        let d = device_from_node(&[("media.class", "Video/Source"),
            ("node.name", "cam"), ("node.description", "Front Camera"),
            ("api.libcamera.location", "external")]).unwrap();
        assert_eq!(d.facing, Facing::Unknown, "guessed from the label instead");
    }

    #[test]
    fn the_facing_pipewire_states_beats_the_label_guess() {
        // A node described "Main Camera" while libcamera says front is a front
        // camera; trusting the label would un-mirror a selfie.
        let d = device_from_node(&[
            ("media.class", "Video/Source"),
            ("node.name", "cam"),
            ("node.description", "Main Camera"),
            ("api.libcamera.location", "front"),
        ]).unwrap();
        assert_eq!(d.facing, Facing::Front, "label says rear, libcamera says front");
    }

    #[test]
    fn two_sensors_on_one_libcamera_device_share_a_capture_group() {
        // This is where the FP5 constraint comes from — no special case, just
        // the device id both sensors report.
        let front = device_from_node(&[("media.class", "Video/Source"),
            ("node.name", "f"), ("api.libcamera.location", "front"), ("device.id", "7")]).unwrap();
        let rear = device_from_node(&[("media.class", "Video/Source"),
            ("node.name", "r"), ("api.libcamera.location", "back"), ("device.id", "7")]).unwrap();
        assert!(!concurrent(&front, &rear));
        assert!(!pip_available(&[front, rear]));
    }

    #[test]
    fn a_usb_webcam_gets_its_own_group_without_a_special_case() {
        let a = device_from_node(&[("media.class", "Video/Source"),
            ("node.name", "usb0"), ("device.id", "11")]).unwrap();
        let b = device_from_node(&[("media.class", "Video/Source"),
            ("node.name", "usb1"), ("device.id", "12")]).unwrap();
        assert!(concurrent(&a, &b));
    }

    #[test]
    fn a_node_falls_back_through_description_then_nick_then_name() {
        let n = device_from_node(&[("media.class", "Video/Source"), ("node.name", "cam0")]).unwrap();
        assert_eq!(n.label, "cam0", "the name is the last resort, not an empty label");
        let k = device_from_node(&[("media.class", "Video/Source"),
            ("node.name", "cam0"), ("node.nick", "Webcam")]).unwrap();
        assert_eq!(k.label, "Webcam");
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
