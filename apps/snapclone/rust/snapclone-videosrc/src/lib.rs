//! snapclone-videosrc — video source model + the simultaneity check.
//!
//! See `docs/PIP_AND_SCREENSHARE_DESIGN.md`. The key insight: a physical
//! camera, a USB webcam, a virtual-cam-bridge node, and a PipeWire screenshare
//! stream are ALL just PipeWire nodes the gst backend targets by name. The only
//! thing that differs operationally is **which of them can run at the same
//! time**.
//!
//! On the FP5 the front and rear cameras share one ISP, so they CANNOT both be
//! live — that's why Picture-in-Picture (rear + front selfie overlay) silently
//! fails on device. The PiP code never checked this; it just spun up a second
//! `pipewiresrc` and got a negotiation failure. This module models the
//! constraint as a **concurrent-capture group**: sources sharing exclusive
//! hardware share a group id, and at most one source per group may be live at
//! once. Independent USB webcams each get a unique group, so any number run
//! together; screenshare is its own group (always concurrent with cameras).
//!
//! `can_run_simultaneously` is the single function PiP / multi-cam / "screen +
//! selfie" all reduce to. It's pure decision logic over enumerated sources — no
//! I/O, no platform deps — so it's fully unit-testable here, unlike the Qt /
//! DBus / PipeWire glue that consumes it.

use std::collections::HashMap;

/// What kind of video source a node is. Drives UI labeling and default policy
/// (e.g. screenshare needs a persistent "sharing" indicator; cameras may mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A physical phone camera via libcamera (may be ISP-exclusive).
    PhysicalCamera,
    /// A desktop USB / UVC webcam — independent, freely concurrent.
    UsbWebcam,
    /// One of our camera-bridge virtual-cam-* nodes.
    VirtualCamera,
    /// A PipeWire screenshare stream from the xdg-desktop-portal ScreenCast API.
    ScreenCast,
}

impl SourceKind {
    /// Human label for the source picker.
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::PhysicalCamera => "Camera",
            SourceKind::UsbWebcam => "USB Webcam",
            SourceKind::VirtualCamera => "Virtual Camera",
            SourceKind::ScreenCast => "Screen Share",
        }
    }

    /// Screenshare is sensitive enough to always warrant a visible "live"
    /// indicator (it can capture other apps). Cameras already have the
    /// recording dot; this flags sources that need the extra affordance.
    pub fn needs_live_indicator(self) -> bool {
        matches!(self, SourceKind::ScreenCast)
    }
}

/// A concurrency group: sources sharing the SAME group id contend for exclusive
/// hardware and cannot be live together. A `None` group means "no exclusivity"
/// (this source can always run alongside anything, including others of its own
/// kind) — the common case for independent USB webcams and screenshare.
///
/// We use Option<u32> rather than a bare id so the "freely concurrent" case is
/// explicit and can't be confused with "group 0".
pub type CaptureGroup = Option<u32>;

/// An enumerated video source. `node` is the PipeWire node name the backend
/// targets via `pipewiresrc target-object=<node>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSource {
    pub node: String,
    pub display_name: String,
    pub kind: SourceKind,
    /// Sources sharing this group can't be live simultaneously. None = freely
    /// concurrent.
    pub capture_group: CaptureGroup,
}

impl VideoSource {
    pub fn new(node: &str, display_name: &str, kind: SourceKind, group: CaptureGroup) -> Self {
        Self {
            node: node.to_owned(),
            display_name: display_name.to_owned(),
            kind,
            capture_group: group,
        }
    }
}

/// Why a requested set of sources can't all run at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Two sources share an exclusive capture group (e.g. FP5 front+rear ISP).
    SharedExclusiveHardware {
        group: u32,
        a: String, // node a
        b: String, // node b
    },
}

impl Conflict {
    /// A user-facing explanation — this is what PiP should show instead of a
    /// silent black tile.
    pub fn user_message(&self) -> String {
        match self {
            Conflict::SharedExclusiveHardware { a, b, .. } => format!(
                "This device can't capture “{a}” and “{b}” at the same time — \
                 they share the same camera hardware."
            ),
        }
    }
}

/// THE check PiP was missing: can every source in `chosen` capture at the same
/// time? Returns Ok(()) if yes, or the first blocking Conflict if not.
///
/// Algorithm: group the chosen sources by their (Some) capture group; if any
/// group holds more than one chosen source, that's a conflict. None-group
/// sources never conflict.
pub fn can_run_simultaneously(chosen: &[VideoSource]) -> Result<(), Conflict> {
    let mut seen: HashMap<u32, &str> = HashMap::new();
    for src in chosen {
        if let Some(g) = src.capture_group {
            if let Some(prev) = seen.insert(g, &src.node) {
                // Two sources in the same exclusive group → conflict.
                return Err(Conflict::SharedExclusiveHardware {
                    group: g,
                    a: prev.to_string(),
                    b: src.node.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Convenience for the PiP decision specifically: given a main source and a
/// candidate PiP source, may they run together?
pub fn pip_allowed(main: &VideoSource, pip: &VideoSource) -> Result<(), Conflict> {
    can_run_simultaneously(&[main.clone(), pip.clone()])
}

/// Given all enumerated sources and a `main` selection, return the subset that
/// could serve as a SECOND simultaneous source (for offering PiP / multi-cam
/// options in the UI). Excludes the main itself and anything that conflicts.
pub fn compatible_secondaries<'a>(
    all: &'a [VideoSource],
    main: &VideoSource,
) -> Vec<&'a VideoSource> {
    all.iter()
        .filter(|s| s.node != main.node)
        .filter(|s| pip_allowed(main, s).is_ok())
        .collect()
}

// ─── pw-dump node classification ──────────────────────────────────────────────
//
// The C++ CameraRegistry historically only matched the FP5 bridge's
// `virtual-cam-*` nodes and raw `libcamera_input.*` nodes, so a plain desktop
// USB/UVC webcam — which PipeWire publishes as a generic Video/Source node with
// api.v4l2 — never appeared in the camera list (confirmed on hardware: plugging
// a USB webcam in on desktop did nothing). This classifier is the tested
// decision the registry needs: given the fields pw-dump reports for a node
// (node name, media.class, the api/factory string), decide whether it's a
// usable camera source and which SourceKind + capture group it gets.
//
// Capture-group assignment: V4L2/USB webcams are independent devices, so each
// gets its OWN group (freely concurrent → simultaneous capture works on
// desktop, unlike the FP5's shared-ISP front/rear). We derive a stable group id
// from the node id so two enumerations of the same device agree.

/// A node as seen in pw-dump: just the fields we classify on. The C++ side
/// fills these from the parsed JSON/props.
#[derive(Debug, Clone)]
pub struct PwNode {
    /// Numeric PipeWire object id (used to derive a stable capture group).
    pub object_id: u32,
    pub node_name: String,
    /// PipeWire `media.class`, e.g. "Video/Source", "Stream/Output/Video".
    pub media_class: String,
    /// The device API/factory hint, e.g. "v4l2", "libcamera", "pipewiresrc".
    /// Lowercased by the caller is fine; we compare case-insensitively.
    pub api: String,
    /// Human description (`node.description`), used for the display name.
    pub description: String,
}

/// Decide whether a pw-dump node is a video source we can use, and classify it.
/// Returns None for nodes that aren't usable cameras/sources (audio, sinks,
/// monitors, etc.). This is purely the FP5-unrelated additions — the registry
/// keeps its existing libcamera/virtual-cam handling and uses this for the
/// generic nodes it used to ignore.
pub fn classify_pw_node(n: &PwNode) -> Option<VideoSource> {
    let media = n.media_class.to_ascii_lowercase();
    let api = n.api.to_ascii_lowercase();
    let name = n.node_name.to_ascii_lowercase();

    // Only video SOURCES (a webcam) or video OUTPUT streams (a screencast).
    let is_video_source = media == "video/source";
    let is_video_stream = media == "stream/output/video";
    if !is_video_source && !is_video_stream {
        return None;
    }

    // The FP5 bridge + raw libcamera nodes are handled by the existing registry
    // path; don't double-classify them here.
    if name.starts_with("virtual-cam-") || name.starts_with("libcamera_input.") {
        return None;
    }

    let display = if n.description.is_empty() {
        n.node_name.clone()
    } else {
        n.description.clone()
    };

    if is_video_stream {
        // A screencast portal stream. Its own (None) group → always concurrent.
        return Some(VideoSource::new(
            &n.node_name,
            &display,
            SourceKind::ScreenCast,
            None,
        ));
    }

    // is_video_source: a real capture device. V4L2/USB → independent webcam,
    // each its own exclusive group so two webcams can coexist but the same
    // device can't be opened twice. libcamera-api sources that slipped past the
    // name filter are still physical cameras.
    let kind = if api.contains("v4l2") || name.contains("usb") || name.contains("uvc") {
        SourceKind::UsbWebcam
    } else if api.contains("libcamera") {
        SourceKind::PhysicalCamera
    } else {
        // Unknown video source API — treat as a USB-style independent webcam
        // (the safe assumption: give it its own group rather than sharing one).
        SourceKind::UsbWebcam
    };

    // Stable per-device exclusive group derived from the object id, offset so it
    // never collides with the FP5 ISP group (0) the registry uses for
    // front/rear. Each distinct device id → a distinct group → independent.
    let group = Some(1000 + n.object_id);
    Some(VideoSource::new(&n.node_name, &display, kind, group))
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    fn node(id: u32, name: &str, class: &str, api: &str, desc: &str) -> PwNode {
        PwNode {
            object_id: id,
            node_name: name.to_owned(),
            media_class: class.to_owned(),
            api: api.to_owned(),
            description: desc.to_owned(),
        }
    }

    #[test]
    fn usb_webcam_is_recognized() {
        // The case that failed on hardware: a plugged-in USB webcam.
        let n = node(42, "v4l2_input.usb-0bda_1234", "Video/Source", "v4l2", "HD Webcam C920");
        let s = classify_pw_node(&n).expect("USB webcam should classify");
        assert_eq!(s.kind, SourceKind::UsbWebcam);
        assert_eq!(s.display_name, "HD Webcam C920");
        assert!(s.capture_group.is_some(), "gets its own exclusive group");
        assert_ne!(s.capture_group, Some(0), "must not collide with FP5 ISP group 0");
    }

    #[test]
    fn two_usb_webcams_are_independent() {
        let a = classify_pw_node(&node(10, "v4l2_a", "Video/Source", "v4l2", "Cam A")).unwrap();
        let b = classify_pw_node(&node(11, "v4l2_b", "Video/Source", "v4l2", "Cam B")).unwrap();
        // Different object ids → different groups → can run simultaneously.
        assert_ne!(a.capture_group, b.capture_group);
        assert!(can_run_simultaneously(&[a, b]).is_ok());
    }

    #[test]
    fn screencast_stream_is_recognized() {
        let n = node(7, "screencast-portal-0", "Stream/Output/Video", "pipewiresrc", "Screen Share");
        let s = classify_pw_node(&n).unwrap();
        assert_eq!(s.kind, SourceKind::ScreenCast);
        assert!(s.capture_group.is_none(), "screencast is freely concurrent");
    }

    #[test]
    fn fp5_nodes_are_left_to_the_registry() {
        // The existing registry path owns these; the classifier must skip them
        // so we don't double-list or mis-group the shared-ISP cameras.
        assert!(classify_pw_node(&node(1, "virtual-cam-rear", "Video/Source", "libcamera", "Rear")).is_none());
        assert!(classify_pw_node(&node(2, "libcamera_input._base_soc_0", "Video/Source", "libcamera", "x")).is_none());
    }

    #[test]
    fn non_video_nodes_are_ignored() {
        assert!(classify_pw_node(&node(3, "alsa_output.0", "Audio/Sink", "alsa", "Speakers")).is_none());
        assert!(classify_pw_node(&node(4, "v4l2_output.0", "Video/Sink", "v4l2", "loopback")).is_none());
    }

    #[test]
    fn unknown_video_source_api_defaults_to_independent_webcam() {
        let s = classify_pw_node(&node(9, "some_capture", "Video/Source", "weirdapi", "Mystery Cam")).unwrap();
        assert_eq!(s.kind, SourceKind::UsbWebcam);
        assert!(s.capture_group.is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_order_bgr_is_no_swap() {
        // BGRx/BGRA = bytes B,G,R,(x/A) = exactly Qt Format_RGB32 LE layout.
        assert_eq!(classify_pixel_order("BGRx"), Some(PixelOrder::NoSwap));
        assert_eq!(classify_pixel_order("BGRA"), Some(PixelOrder::NoSwap));
        assert_eq!(classify_pixel_order("bgrx"), Some(PixelOrder::NoSwap)); // case-insensitive
    }

    #[test]
    fn pixel_order_rgb_needs_swap() {
        // RGBx/RGBA = bytes R,G,B,(x/A): R and B reversed vs BGRx → rgbSwapped.
        assert_eq!(classify_pixel_order("RGBx"), Some(PixelOrder::SwapRb));
        assert_eq!(classify_pixel_order("RGBA"), Some(PixelOrder::SwapRb));
    }

    #[test]
    fn pixel_order_alpha_first_not_cheap_fixable() {
        // Pad/alpha byte FIRST: components are rotated, not mirrored — a simple
        // R/B swap leaves them wrong. The old code WRONGLY treated ARGB as a
        // swap; this guards that regression.
        for f in ["xRGB", "ARGB", "xBGR", "ABGR"] {
            assert_eq!(classify_pixel_order(f), Some(PixelOrder::NotCheapFixable),
                       "format {f} must not be classified as a cheap R/B swap");
        }
    }

    #[test]
    fn pixel_order_unknown_is_none() {
        // YUV / unknown names → None (caller keeps its current setting).
        assert_eq!(classify_pixel_order("NV12"), None);
        assert_eq!(classify_pixel_order("I420"), None);
        assert_eq!(classify_pixel_order(""), None);
        assert_eq!(classify_pixel_order("string"), None); // the old parser bug's value
    }

    // FP5: front + rear share ISP group 0. A USB webcam (group None) and a
    // screenshare (group None) are independent.
    fn fp5_rear() -> VideoSource {
        VideoSource::new("virtual-cam-rear", "Rear Camera", SourceKind::VirtualCamera, Some(0))
    }
    fn fp5_front() -> VideoSource {
        VideoSource::new("virtual-cam-front", "Front Camera", SourceKind::VirtualCamera, Some(0))
    }
    fn usb_a() -> VideoSource {
        VideoSource::new("v4l2-usb-a", "Logitech C920", SourceKind::UsbWebcam, Some(10))
    }
    fn usb_b() -> VideoSource {
        VideoSource::new("v4l2-usb-b", "Razer Kiyo", SourceKind::UsbWebcam, Some(11))
    }
    fn screen() -> VideoSource {
        VideoSource::new("screencast-0", "Screen Share", SourceKind::ScreenCast, None)
    }

    #[test]
    fn fp5_front_and_rear_conflict() {
        // The headline PiP bug: rear + front cannot run together on the FP5.
        let res = pip_allowed(&fp5_rear(), &fp5_front());
        assert!(res.is_err());
        match res.unwrap_err() {
            Conflict::SharedExclusiveHardware { group, .. } => assert_eq!(group, 0),
        }
    }

    #[test]
    fn fp5_conflict_has_useful_message() {
        let msg = pip_allowed(&fp5_rear(), &fp5_front())
            .unwrap_err()
            .user_message();
        assert!(msg.contains("same camera hardware"));
        // Not a silent failure — there's an actual reason to show the user.
        assert!(!msg.is_empty());
    }

    #[test]
    fn independent_usb_webcams_run_together() {
        // Desktop webcam array: two independent UVC devices CAN run at once,
        // so PiP / multi-cam works where it failed on mobile.
        assert!(can_run_simultaneously(&[usb_a(), usb_b()]).is_ok());
    }

    #[test]
    fn screenshare_plus_camera_ok() {
        // "Screen + selfie": screenshare (no group) alongside any one camera.
        assert!(can_run_simultaneously(&[screen(), fp5_front()]).is_ok());
        assert!(can_run_simultaneously(&[screen(), usb_a()]).is_ok());
    }

    #[test]
    fn three_independent_sources_ok() {
        // Two independent webcams + a screenshare: all concurrent.
        assert!(can_run_simultaneously(&[usb_a(), usb_b(), screen()]).is_ok());
    }

    #[test]
    fn mixed_with_one_exclusive_pair_conflicts() {
        // A valid trio plus a second ISP-group camera trips the check.
        let set = [usb_a(), screen(), fp5_rear(), fp5_front()];
        assert!(can_run_simultaneously(&set).is_err());
    }

    #[test]
    fn compatible_secondaries_excludes_isp_sibling() {
        // On the FP5, if main = rear, the front is NOT a compatible secondary
        // (so PiP wouldn't be offered), but a plugged-in USB webcam WOULD be.
        let all = vec![fp5_rear(), fp5_front(), usb_a(), screen()];
        let main = fp5_rear();
        let secs = compatible_secondaries(&all, &main);
        let nodes: Vec<&str> = secs.iter().map(|s| s.node.as_str()).collect();
        assert!(!nodes.contains(&"virtual-cam-rear")); // not itself
        assert!(!nodes.contains(&"virtual-cam-front")); // ISP sibling excluded
        assert!(nodes.contains(&"v4l2-usb-a")); // independent webcam OK
        assert!(nodes.contains(&"screencast-0")); // screenshare OK
    }

    #[test]
    fn single_source_always_ok() {
        assert!(can_run_simultaneously(&[fp5_rear()]).is_ok());
        assert!(can_run_simultaneously(&[]).is_ok());
    }

    #[test]
    fn source_kind_labels_and_indicator() {
        assert_eq!(SourceKind::ScreenCast.label(), "Screen Share");
        assert!(SourceKind::ScreenCast.needs_live_indicator());
        assert!(!SourceKind::PhysicalCamera.needs_live_indicator());
    }
}

// ─── frame geometry: stride / buffer-size math for tiling + GPU import ────────
//
// The single most common live-preview corruption ("tiling" / a sheared,
// diagonally-walking image) comes from a ROW STRIDE mismatch: the camera ISP or
// the GL downloader pads each row up to an alignment boundary (16/32/64/128
// bytes), so the real bytes-per-row is GST_ROUND_UP(width×bpp, align), NOT the
// tight width×bpp the naive reader assumes. Read at the wrong stride and every
// row starts a few bytes off from the last → the picture skews.
//
// This module is the geometry both fixes need, lifted out of the C++ so it can
// be TESTED (the inline version in GstLaunchBackend never was):
//   * `padded_stride` / `frame_size` — what to EXPECT for a given alignment, so
//     the reader can pass the correct bytesPerLine instead of guessing.
//   * `infer_stride` — given an OBSERVED buffer size that doesn't match the
//     tight size, work out which alignment the producer actually used (this is
//     the "Stride candidates:" diagnostic, made deterministic and testable).
//   * A GPU texture-import path (DMA-BUF / Vulkan on Mesa) needs the exact same
//     numbers to describe the imported image's layout to the RHI.

/// Bytes per pixel for the packed formats the preview pipeline uses.
pub const BGRX_BPP: u32 = 4;

/// How a packed-32 GStreamer format maps onto Qt's `Format_RGB32` reader, whose
/// little-endian byte layout is [B, G, R, x]. The preview pipeline always
/// negotiates BGRx, so `NoSwap` is the expected case; the others are the
/// defensive fallback for when something upstream silently delivers a different
/// byte order.
///
/// GStreamer format names list bytes in MEMORY order, left to right (e.g. RGBx
/// is bytes R,G,B,x). The decision is purely "where are R and B?":
///   * R at byte2, B at byte0  → already correct           → `NoSwap`
///   * R at byte0, B at byte2  → reversed, rgbSwapped fixes → `SwapRb`
///   * pad/alpha byte FIRST (xRGB/ARGB/xBGR/ABGR) → a simple R/B swap CANNOT
///     fix it (the components are rotated, not mirrored)   → `NotCheapFixable`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOrder {
    NoSwap,
    SwapRb,
    NotCheapFixable,
}

/// Classify a GStreamer packed-32 format name (case-insensitive). Unknown names
/// return `None` so the caller can leave its current setting untouched.
pub fn classify_pixel_order(format: &str) -> Option<PixelOrder> {
    match format.to_ascii_uppercase().as_str() {
        "BGRX" | "BGRA" => Some(PixelOrder::NoSwap),
        "RGBX" | "RGBA" => Some(PixelOrder::SwapRb),
        "XRGB" | "ARGB" | "XBGR" | "ABGR" => Some(PixelOrder::NotCheapFixable),
        _ => None,
    }
}

/// Round `v` up to the next multiple of `align` (align must be > 0).
pub fn round_up(v: u32, align: u32) -> u32 {
    if align <= 1 {
        return v;
    }
    ((v + align - 1) / align) * align
}

/// The padded row stride (bytes) for `width` pixels at `bpp` bytes/pixel,
/// rounded up to `align_bytes`. With align_bytes <= 1 this is the tight stride.
pub fn padded_stride(width: u32, bpp: u32, align_bytes: u32) -> u32 {
    round_up(width * bpp, align_bytes)
}

/// Total bytes for one frame at the given padded stride.
pub fn frame_size(width: u32, height: u32, bpp: u32, align_bytes: u32) -> u64 {
    padded_stride(width, bpp, align_bytes) as u64 * height as u64
}

/// The tight (unpadded) frame size — what a naive reader assumes.
pub fn tight_frame_size(width: u32, height: u32, bpp: u32) -> u64 {
    (width * bpp) as u64 * height as u64
}

/// A candidate explanation for an observed buffer size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrideCandidate {
    pub align_bytes: u32,
    pub row_stride: u32,
    pub frame_size: u64,
}

/// Given the negotiated width/height/bpp and an OBSERVED total buffer size that
/// doesn't equal the tight size, return the alignment candidates whose implied
/// frame size divides the observed buffer cleanly (i.e. the buffer is an exact
/// whole number of such frames). Ordered smallest-alignment first. This is the
/// deterministic core of the old inline "Stride candidates:" heuristic.
///
/// `observed_total` is however many bytes have accumulated (one or more frames).
pub fn infer_stride(
    width: u32,
    height: u32,
    bpp: u32,
    observed_total: u64,
) -> Vec<StrideCandidate> {
    let tight = tight_frame_size(width, height, bpp);
    let mut out = Vec::new();
    for &align in &[16u32, 32, 64, 128, 256] {
        let fs = frame_size(width, height, bpp, align);
        if fs == tight {
            continue; // this alignment implies no padding — not the culprit
        }
        if fs > 0 && observed_total % fs == 0 {
            out.push(StrideCandidate {
                align_bytes: align,
                row_stride: padded_stride(width, bpp, align),
                frame_size: fs,
            });
        }
    }
    out
}

/// The single best-guess stride for an observed buffer (the smallest alignment
/// that divides it cleanly), or None if the tight stride already fits or nothing
/// matches. Useful for auto-recovery rather than just diagnostics.
pub fn best_stride_guess(
    width: u32,
    height: u32,
    bpp: u32,
    observed_total: u64,
) -> Option<StrideCandidate> {
    let tight = tight_frame_size(width, height, bpp);
    if tight > 0 && observed_total % tight == 0 {
        return None; // tight stride already explains it — no padding
    }
    infer_stride(width, height, bpp, observed_total).into_iter().next()
}

/// Round a requested capture WIDTH down to a multiple of `align_px` so that the
/// tight stride (width×bpp) is already a multiple of `align_px×bpp` — i.e. the
/// producer's padded stride equals the tight stride and no tiling is possible.
/// This is the "avoid padding entirely" strategy (vs. handling it at read time).
pub fn align_width_down(width: u32, align_px: u32) -> u32 {
    if align_px <= 1 {
        return width;
    }
    (width / align_px) * align_px
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn tight_1280_has_no_padding() {
        // 1280×4 = 5120, already a multiple of 16/32/64/128/256 → no padding,
        // which matches the device log (bytes-per-frame 3686400 = 1280*720*4).
        assert_eq!(padded_stride(1280, BGRX_BPP, 64), 5120);
        assert_eq!(frame_size(1280, 720, BGRX_BPP, 64), 3_686_400);
        assert_eq!(tight_frame_size(1280, 720, BGRX_BPP), 3_686_400);
        // No candidates because every alignment equals tight.
        assert!(infer_stride(1280, 720, BGRX_BPP, 3_686_400).is_empty());
        assert!(best_stride_guess(1280, 720, BGRX_BPP, 3_686_400).is_none());
    }

    #[test]
    fn odd_width_pads_and_tiles_without_correct_stride() {
        // A width whose tight row isn't 64-aligned: 1284×4 = 5136. Round up to
        // 64 → 5184 (a 48-byte pad per row). This is exactly the tiling setup.
        let w = 1284;
        assert_eq!(padded_stride(w, BGRX_BPP, 64), 5184);
        let tight = tight_frame_size(w, 720, BGRX_BPP); // 3_697_920
        let padded = frame_size(w, 720, BGRX_BPP, 64); // 3_732_480
        assert_ne!(tight, padded, "padding present → naive reader tiles");
    }

    #[test]
    fn infer_recovers_the_alignment_from_a_bloated_buffer() {
        // Producer used 64-byte alignment on a 1284-wide frame; the reader
        // assumed tight and let the buffer bloat to 3 frames. infer_stride
        // should surface align=64 as a clean divisor.
        let w = 1284;
        let padded = frame_size(w, 720, BGRX_BPP, 64);
        let observed = padded * 3; // three real frames accumulated
        let cands = infer_stride(w, 720, BGRX_BPP, observed);
        assert!(cands.iter().any(|c| c.align_bytes == 64 && c.row_stride == 5184),
                "should identify the 64-byte alignment, got {cands:?}");
        // best guess picks the smallest clean alignment.
        let best = best_stride_guess(w, 720, BGRX_BPP, observed).unwrap();
        assert_eq!(best.frame_size, padded);
    }

    #[test]
    fn align_width_down_kills_padding() {
        // Rounding 1284 down to a multiple of 16 → 1280, whose tight stride is
        // already 64-aligned → no padding, no tiling.
        let aligned = align_width_down(1284, 16);
        assert_eq!(aligned, 1280);
        assert!(infer_stride(aligned, 720, BGRX_BPP,
                             tight_frame_size(aligned, 720, BGRX_BPP)).is_empty());
    }

    #[test]
    fn round_up_basics() {
        assert_eq!(round_up(5136, 64), 5184);
        assert_eq!(round_up(5120, 64), 5120);
        assert_eq!(round_up(1, 16), 16);
        assert_eq!(round_up(100, 1), 100); // align<=1 is a no-op
    }
}

// ─── preview frame sanity check ──────────────────────────────────────────────
//
// A defensive guard for the UI boundary: before painting a preview frame, the
// app can ask "is this QImage's byte layout self-consistent?" A frame whose
// total byte count doesn't match width × bytesPerLine, or whose bytesPerLine is
// below the tight minimum (width×bpp), is malformed — painting it produces the
// tiling/repeat/tint artifacts. Catching it here turns "preview looks weird"
// into a precise log line. Pure arithmetic; the C++ side reads the QImage's
// sizeInBytes()/bytesPerLine()/width()/height() and calls this.

/// Why a preview frame failed validation (Ok = fine).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameValidity {
    Ok,
    /// bytesPerLine is smaller than width×bpp — rows can't fit; guaranteed skew.
    StrideTooSmall { stride: u32, min: u32 },
    /// total bytes != bytesPerLine × height — truncated or oversized buffer.
    SizeMismatch { total: u64, expected: u64 },
    /// zero/absurd geometry.
    BadGeometry,
}

impl FrameValidity {
    pub fn is_ok(self) -> bool {
        matches!(self, FrameValidity::Ok)
    }
}

/// Validate a preview frame's declared layout. `total_bytes` is the buffer size,
/// `stride` is bytesPerLine, `width`/`height` the dimensions, `bpp` bytes/pixel.
pub fn validate_preview_frame(
    width: u32,
    height: u32,
    bpp: u32,
    stride: u32,
    total_bytes: u64,
) -> FrameValidity {
    if width == 0 || height == 0 || bpp == 0 {
        return FrameValidity::BadGeometry;
    }
    let min_stride = width * bpp;
    if stride < min_stride {
        return FrameValidity::StrideTooSmall {
            stride,
            min: min_stride,
        };
    }
    let expected = stride as u64 * height as u64;
    // Allow the buffer to be >= expected (some allocators over-allocate), but a
    // buffer SMALLER than one frame's worth is definitely truncated.
    if total_bytes < expected {
        return FrameValidity::SizeMismatch { total: total_bytes, expected };
    }
    FrameValidity::Ok
}

#[cfg(test)]
mod frame_validity_tests {
    use super::*;

    #[test]
    fn well_formed_frame_is_ok() {
        // 1280×720 BGRx, tight stride, exact size.
        assert!(validate_preview_frame(1280, 720, 4, 5120, 3_686_400).is_ok());
        // Over-allocated buffer is still ok (>= expected).
        assert!(validate_preview_frame(1280, 720, 4, 5120, 4_000_000).is_ok());
        // Padded stride, matching size.
        assert!(validate_preview_frame(1284, 720, 4, 5184, 3_732_480).is_ok());
    }

    #[test]
    fn stride_below_tight_is_caught() {
        // The repeat-near-bottom signature: stride < width×bpp.
        match validate_preview_frame(1280, 720, 4, 5000, 3_600_000) {
            FrameValidity::StrideTooSmall { stride, min } => {
                assert_eq!(stride, 5000);
                assert_eq!(min, 5120);
            }
            other => panic!("expected StrideTooSmall, got {other:?}"),
        }
    }

    #[test]
    fn truncated_buffer_is_caught() {
        // Buffer smaller than stride×height → truncated.
        match validate_preview_frame(1280, 720, 4, 5120, 3_000_000) {
            FrameValidity::SizeMismatch { total, expected } => {
                assert_eq!(total, 3_000_000);
                assert_eq!(expected, 3_686_400);
            }
            other => panic!("expected SizeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn zero_geometry_is_bad() {
        assert_eq!(validate_preview_frame(0, 720, 4, 0, 0), FrameValidity::BadGeometry);
        assert_eq!(validate_preview_frame(1280, 0, 4, 5120, 0), FrameValidity::BadGeometry);
    }
}

// ─── preview frame-processing gate ───────────────────────────────────────────
//
// CameraWidget::shouldProcessFrames() decides, per frame, whether to process +
// display it. It's pure boolean logic but lives inline in the GUI and is only
// observable on-device by backgrounding the app and reading a log line. Extract
// it so it's TESTED and so a hardware diagnostic harness can replay a sequence
// of state transitions and assert the gate matches expectation.
//
// The device log shows "Frame processing PAUSED (visible: true, appActive:
// false)" — the preview freezing purely because the app lost ACTIVE state while
// still fully visible. On a desktop/Plasma compositor app-active flips false for
// transient reasons (notifications, focus changes), so a naive
// `require app_active` freezes the live preview spuriously. `should_process_with_
// debounce` models a fix: a brief inactive blip does NOT pause if the widget is
// still visible and the screen is on.

/// Inputs to the per-frame processing gate (mirror of the C++ member flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameGate {
    pub app_active: bool,
    pub widget_visible: bool,
    pub screen_off: bool,
    pub scan_mode: bool,
}

/// Why the gate decided as it did — lets the harness/log say WHAT paused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateReason {
    ProcessVisible,   // normal: visible + active + screen on
    ProcessScanMode,  // scanning needs frames even when offscreen
    PauseAppInactive, // app not foreground
    PauseScreenOff,   // display dark
    PauseNotVisible,  // widget offscreen (and not scanning)
}

impl GateReason {
    pub fn processes(self) -> bool {
        matches!(self, GateReason::ProcessVisible | GateReason::ProcessScanMode)
    }
}

/// Exact mirror of CameraWidget::shouldProcessFrames() precedence:
///   1. app inactive  → pause
///   2. screen off    → pause (beats scan mode)
///   3. scan mode     → process (even if offscreen)
///   4. else          → process iff widget visible
pub fn frame_gate(g: FrameGate) -> GateReason {
    if !g.app_active {
        return GateReason::PauseAppInactive;
    }
    if g.screen_off {
        return GateReason::PauseScreenOff;
    }
    if g.scan_mode {
        return GateReason::ProcessScanMode;
    }
    if g.widget_visible {
        GateReason::ProcessVisible
    } else {
        GateReason::PauseNotVisible
    }
}

/// Debounce-aware gate for the transient-inactive freeze. A short inactive blip
/// (app_active just went false, within `grace_ms` of the last active moment)
/// should NOT pause the preview if the widget is still visible and the screen is
/// on — this is the desktop "lost focus to a notification" case where freezing
/// the viewfinder is wrong. Screen-off and not-visible still pause immediately.
///
/// `ms_since_active` is how long app_active has been false (0 if currently true).
pub fn frame_gate_debounced(g: FrameGate, ms_since_active: u64, grace_ms: u64) -> GateReason {
    if !g.app_active {
        // Screen-off and offscreen are real pauses regardless of grace.
        if g.screen_off {
            return GateReason::PauseScreenOff;
        }
        if !g.widget_visible && !g.scan_mode {
            return GateReason::PauseNotVisible;
        }
        // Visible + screen on + only-just-inactive → keep processing through the
        // blip; only pause once we've been inactive past the grace window.
        if ms_since_active <= grace_ms {
            return if g.scan_mode {
                GateReason::ProcessScanMode
            } else {
                GateReason::ProcessVisible
            };
        }
        return GateReason::PauseAppInactive;
    }
    // Active → identical to the plain gate.
    frame_gate(g)
}

#[cfg(test)]
mod frame_gate_tests {
    use super::*;

    fn g(active: bool, visible: bool, screen_off: bool, scan: bool) -> FrameGate {
        FrameGate { app_active: active, widget_visible: visible, screen_off, scan_mode: scan }
    }

    #[test]
    fn normal_visible_active_processes() {
        assert_eq!(frame_gate(g(true, true, false, false)), GateReason::ProcessVisible);
    }

    #[test]
    fn inactive_pauses_even_if_visible() {
        // This is the device-log case: visible=true, app_active=false → paused.
        assert_eq!(frame_gate(g(false, true, false, false)), GateReason::PauseAppInactive);
    }

    #[test]
    fn screen_off_beats_scan_mode() {
        // Screen-off must win over scan mode (battery: nothing to scan in the dark).
        assert_eq!(frame_gate(g(true, true, true, true)), GateReason::PauseScreenOff);
    }

    #[test]
    fn scan_mode_processes_offscreen() {
        // QR scanning works even when the camera widget itself is offscreen.
        assert_eq!(frame_gate(g(true, false, false, true)), GateReason::ProcessScanMode);
    }

    #[test]
    fn offscreen_not_scanning_pauses() {
        assert_eq!(frame_gate(g(true, false, false, false)), GateReason::PauseNotVisible);
    }

    #[test]
    fn debounce_rides_through_brief_inactive_blip() {
        // Visible, screen on, app_active just flipped false 50ms ago, grace 400ms
        // → keep processing (don't freeze the viewfinder on a transient focus loss).
        assert_eq!(
            frame_gate_debounced(g(false, true, false, false), 50, 400),
            GateReason::ProcessVisible
        );
    }

    #[test]
    fn debounce_pauses_after_grace_expires() {
        // Inactive longer than the grace window → genuinely backgrounded, pause.
        assert_eq!(
            frame_gate_debounced(g(false, true, false, false), 800, 400),
            GateReason::PauseAppInactive
        );
    }

    #[test]
    fn debounce_still_pauses_immediately_on_screen_off() {
        assert_eq!(
            frame_gate_debounced(g(false, true, true, false), 10, 400),
            GateReason::PauseScreenOff
        );
    }

    #[test]
    fn debounce_offscreen_pauses_immediately() {
        assert_eq!(
            frame_gate_debounced(g(false, false, false, false), 10, 400),
            GateReason::PauseNotVisible
        );
    }
}


//
// Every received snap's file extension is chosen from its magic bytes (the
// sender doesn't transmit a filename). Wrong extension = the OS/players reject
// it and the viewer's codec routing misfires — e.g. a VVC clip mislabeled
// .webm is unplayable (webm by spec can't carry H.266). This is the pure
// byte-sniff that decides it, extracted from SnapRouter::detectMediaExtension so
// it can be TESTED (the C++ copy never was). The C++ should call this (via FFI)
// or stay byte-for-byte in sync with it.

/// The container/format a buffer's magic bytes indicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaContainer {
    WebM,      // EBML, webm doctype, VP8/VP9/AV1 — our normal output
    Matroska,  // EBML, matroska doctype OR a non-webm codec (VVC/HEVC) → .mkv
    Mp4,       // ftyp box
    Mov,       // QuickTime moov/mdat
    Jpeg,
    Png,
    /// Video bytes whose container we couldn't identify.
    UnknownVideo,
    /// Non-video bytes we couldn't identify (default to image).
    UnknownOther,
}

impl MediaContainer {
    /// The file extension (with dot) for this container.
    pub fn extension(self) -> &'static str {
        match self {
            MediaContainer::WebM => ".webm",
            MediaContainer::Matroska => ".mkv",
            MediaContainer::Mp4 => ".mp4",
            MediaContainer::Mov => ".mov",
            MediaContainer::Jpeg => ".jpg",
            MediaContainer::Png => ".png",
            MediaContainer::UnknownVideo => ".bin",
            MediaContainer::UnknownOther => ".jpg",
        }
    }
}

/// True if the caller told us these bytes are video (used only to pick the
/// fallback when no magic matches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaHint {
    Video,
    Image,
}

const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

fn contains_subslice(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

fn index_of(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Identify the media container from a buffer's leading bytes. Mirrors the
/// SnapRouter logic exactly, including the VVC/HEVC-in-matroska distinction.
pub fn sniff_container(bytes: &[u8], hint: MediaHint) -> MediaContainer {
    // EBML — webm and mkv share the same 4-byte magic, so look deeper.
    if bytes.len() >= 4 && bytes[0..4] == EBML_MAGIC {
        // Scan a bounded prefix (header/Tracks live near the start).
        let scan = bytes.len().min(65536);
        let head = &bytes[..scan];
        // A non-webm codec (VVC/HEVC) means it must be .mkv.
        if contains_subslice(head, b"V_MPEGH/ISO/VVC")
            || contains_subslice(head, b"V_MPEGH/ISO/HEVC")
        {
            return MediaContainer::Matroska;
        }
        // Otherwise distinguish by DocType: an explicit "matroska" before/without
        // "webm" → mkv; else our normal webm.
        let dt = index_of(head, b"matroska");
        let wd = index_of(head, b"webm");
        match (dt, wd) {
            (Some(d), Some(w)) if d < w => MediaContainer::Matroska,
            (Some(_), None) => MediaContainer::Matroska,
            _ => MediaContainer::WebM,
        }
    } else if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        MediaContainer::Mp4
    } else if bytes.len() >= 8 && (&bytes[4..8] == b"moov" || &bytes[4..8] == b"mdat") {
        MediaContainer::Mov
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        MediaContainer::Jpeg
    } else if bytes.len() >= 8 && bytes[0] == 0x89 && &bytes[1..4] == b"PNG" {
        MediaContainer::Png
    } else {
        match hint {
            MediaHint::Video => MediaContainer::UnknownVideo,
            MediaHint::Image => MediaContainer::UnknownOther,
        }
    }
}

/// Convenience: the extension string directly.
pub fn sniff_extension(bytes: &[u8], hint: MediaHint) -> &'static str {
    sniff_container(bytes, hint).extension()
}

#[cfg(test)]
mod container_tests {
    use super::*;

    // Build a tiny EBML buffer: magic + optional doctype + optional codec id.
    fn ebml(doctype: Option<&[u8]>, codec: Option<&[u8]>) -> Vec<u8> {
        let mut v = EBML_MAGIC.to_vec();
        v.extend_from_slice(&[0x42, 0x82]); // a fake DocType element id
        if let Some(d) = doctype {
            v.extend_from_slice(d);
        }
        v.extend_from_slice(&[0x86, 0x00]); // filler
        if let Some(c) = codec {
            v.extend_from_slice(c);
        }
        v.extend_from_slice(&[0u8; 16]); // tail
        v
    }

    #[test]
    fn webm_doctype_is_webm() {
        let b = ebml(Some(b"webm"), None);
        assert_eq!(sniff_container(&b, MediaHint::Video), MediaContainer::WebM);
        assert_eq!(sniff_extension(&b, MediaHint::Video), ".webm");
    }

    #[test]
    fn matroska_doctype_is_mkv() {
        let b = ebml(Some(b"matroska"), None);
        assert_eq!(sniff_container(&b, MediaHint::Video), MediaContainer::Matroska);
    }

    #[test]
    fn vvc_codec_forces_mkv_even_without_doctype() {
        // The VVC case the whole last round was about: a matroska carrying H.266
        // must be .mkv, never .webm.
        let b = ebml(Some(b"webm"), Some(b"V_MPEGH/ISO/VVC"));
        assert_eq!(sniff_container(&b, MediaHint::Video), MediaContainer::Matroska,
                   "VVC track overrides even a webm-looking doctype");
        assert_eq!(sniff_extension(&b, MediaHint::Video), ".mkv");
    }

    #[test]
    fn hevc_codec_forces_mkv() {
        let b = ebml(None, Some(b"V_MPEGH/ISO/HEVC"));
        assert_eq!(sniff_container(&b, MediaHint::Video), MediaContainer::Matroska);
    }

    #[test]
    fn plain_ebml_defaults_to_webm() {
        // No doctype, no exotic codec → our normal webmmux output.
        let b = ebml(None, None);
        assert_eq!(sniff_container(&b, MediaHint::Video), MediaContainer::WebM);
    }

    #[test]
    fn mp4_ftyp() {
        let mut b = vec![0x00, 0x00, 0x00, 0x18];
        b.extend_from_slice(b"ftypmp42");
        b.extend_from_slice(&[0u8; 8]);
        assert_eq!(sniff_container(&b, MediaHint::Video), MediaContainer::Mp4);
        assert_eq!(sniff_extension(&b, MediaHint::Video), ".mp4");
    }

    #[test]
    fn mov_moov() {
        let mut b = vec![0x00, 0x00, 0x00, 0x10];
        b.extend_from_slice(b"moov");
        b.extend_from_slice(&[0u8; 8]);
        assert_eq!(sniff_container(&b, MediaHint::Video), MediaContainer::Mov);
    }

    #[test]
    fn jpeg_and_png() {
        assert_eq!(sniff_container(&[0xFF, 0xD8, 0xFF, 0x00], MediaHint::Image),
                   MediaContainer::Jpeg);
        let mut png = vec![0x89];
        png.extend_from_slice(b"PNG");
        png.extend_from_slice(&[0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(sniff_container(&png, MediaHint::Image), MediaContainer::Png);
    }

    #[test]
    fn unknown_falls_back_to_hint() {
        assert_eq!(sniff_container(&[0, 1, 2, 3, 4], MediaHint::Video),
                   MediaContainer::UnknownVideo);
        assert_eq!(sniff_extension(&[0, 1, 2, 3, 4], MediaHint::Video), ".bin");
        assert_eq!(sniff_container(&[0, 1, 2, 3, 4], MediaHint::Image),
                   MediaContainer::UnknownOther);
        assert_eq!(sniff_extension(&[0, 1, 2, 3, 4], MediaHint::Image), ".jpg");
    }

    #[test]
    fn short_buffers_dont_panic() {
        for len in 0..4 {
            let b = vec![0x1A; len];
            let _ = sniff_container(&b, MediaHint::Video); // must not panic
        }
        // 0xFF 0xD8 without the third byte shouldn't claim jpeg.
        assert_ne!(sniff_container(&[0xFF, 0xD8], MediaHint::Image), MediaContainer::Jpeg);
    }
}
