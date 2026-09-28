//! source_plan — which camera source to try, in what order, as gst pipelines.
//!
//! The desktop-webcam gap (c11 field report, first fixed in
//! `apps/snapclone/camera-bridge/src/source_plan.rs`): capture was
//! pipewiresrc + libcamera only — probing pw-dump for `libcamera_input`
//! names, which exist on postmarketOS and never on a desktop, whose webcams
//! are v4l2. The Qt build never had the problem because Qt Multimedia
//! enumerates the platform backend itself.
//!
//! Moved here from camera-bridge because it is exactly this crate's job —
//! the version-agnostic DECISION half of device selection, fed by probe
//! text, producing an ordered list of pipeline candidates the capture
//! thread tries until one plays — and camera-bridge was the only consumer
//! of a piece of logic that `apps/snapclone/egui-frontend`'s own
//! `camera_source.rs` needs just as much: it currently only picks a
//! *device*, with no notion that a phone wants pipewire/libcamera tried
//! first and a desktop has only v4l2. Two copies of this table would have
//! let a phone and a desktop desync on which candidate order is right,
//! exactly the failure `pick_video_format`'s docs already warn against for
//! format negotiation.
//!
//! Order: an explicit operator override first (a `/dev/video*` path becomes
//! a v4l2 candidate, anything else a pipewire node); then libcamera
//! pipewire nodes (a phone that has them wants them — v4l2 on those
//! kernels is the raw sensor, wrong for a viewfinder); then v4l2 capture
//! devices ([`crate::parse_v4l2_list`] already takes one node per device
//! group, which is what dodges uvcvideo's metadata-node trap, and classes
//! loopback/Dummy as `VideoOutput` so a virtual sink is never a capture
//! candidate).
//!
//! An empty plan is an empty plan: the caller shows "no camera found" — a
//! fabricated test-source candidate here would turn a detection bug into a
//! convincing grey rectangle, which is the failure mode this port already
//! had once.
//!
//! This module builds pipeline STRINGS and does not link gstreamer — it is
//! pure text over probe output, same std-only shape as the rest of this
//! crate. A caller that wants to confirm the strings actually parse against
//! a real GStreamer (camera-bridge does, in its own tests) links gstreamer
//! itself; that dependency does not belong in a crate several UI-less
//! consumers share.

use crate::{parse_v4l2_list, DeviceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    PipewireLibcamera,
    V4l2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceCandidate {
    pub kind: SourceKind,
    /// Human-readable, for the log line and the picker.
    pub label: String,
    pub pipeline: String,
}

/// The one sink shape both sources feed (matches the capture thread's
/// expected appsink name and caps).
fn tail(flip_method: Option<&str>) -> String {
    let flip = match flip_method {
        Some(m) => format!("videoflip method={m} ! "),
        None => String::new(),
    };
    format!(
        "videoconvert ! {flip}video/x-raw,format=RGBA ! \
         appsink name=sink max-buffers=2 drop=true sync=false"
    )
}

/// Rewrite a candidate's appsink tail into an fdsink one, for a capture host
/// that runs the pipeline as a `gst-launch-1.0` subprocess instead of linking
/// gstreamer.
///
/// Lives HERE and not in the consumer because [`tail`] above is the only
/// producer of the suffix being replaced — a consumer doing string surgery on
/// it would break silently the day `tail` changes, while this function fails
/// its own test in the same commit.
///
/// The width/height are forced (with a `videoscale` ahead of the caps) so the
/// reader on the other end of the fd knows every frame is exactly
/// `w * h * 4` bytes of RGBA — raw video over a pipe has no in-band framing,
/// so a fixed size IS the framing. `None` when the pipeline does not end in
/// the appsink tail this module wrote, which means it is not one of ours.
pub fn to_fdsink(pipeline: &str, width: u32, height: u32) -> Option<String> {
    // Must mirror `tail` exactly; the round-trip test holds them together.
    let appsink = "videoconvert ! ";
    let idx = pipeline.rfind(appsink)?;
    if !pipeline.contains("appsink name=sink") {
        return None;
    }
    let head = &pipeline[..idx];
    // The flip element sits between videoconvert and the caps in `tail`, so
    // carry it over rather than losing a front camera's mirror on the way
    // from preview to capture.
    let flip = if pipeline.contains("videoflip method=horizontal-flip") {
        "videoflip method=horizontal-flip ! "
    } else {
        ""
    };
    Some(format!(
        "{head}videoconvert ! {flip}videoscale ! \
         video/x-raw,format=RGBA,width={width},height={height} ! \
         fdsink fd=1"
    ))
}

pub fn pipewire_pipeline(node: &str, flip: Option<&str>) -> String {
    format!("pipewiresrc target-object=\"{node}\" ! {}", tail(flip))
}

pub fn v4l2_pipeline(path: &str, flip: Option<&str>) -> String {
    format!("v4l2src device={path} ! {}", tail(flip))
}

/// Any GStreamer source element, with the same tail — `videotestsrc`, a
/// `filesrc ! decodebin`, whatever a developer wants to point the app at.
/// Reached through an override of the form `gst:<source>`; it is how a
/// machine with no camera runs the real `gst-launch-1.0` path end to end,
/// which a test pattern painted in-process cannot exercise.
pub fn custom_pipeline(source: &str, flip: Option<&str>) -> String {
    format!("{} ! {}", source.trim(), tail(flip))
}

/// Build the ordered plan. `pw_nodes` from a pw-dump probe (libcamera
/// input nodes only); `v4l2_list_output` is `v4l2-ctl --list-devices`
/// text (empty when the tool is absent — a desktop without v4l2-utils
/// simply contributes nothing, not an error).
pub fn plan(
    pw_nodes: &[String],
    v4l2_list_output: &str,
    override_source: Option<&str>,
    flip: Option<&str>,
) -> Vec<SourceCandidate> {
    let mut out = Vec::new();
    if let Some(ov) = override_source {
        if !ov.trim().is_empty() {
            let ov = ov.trim();
            if let Some(src) = ov.strip_prefix("gst:") {
                out.push(SourceCandidate {
                    kind: SourceKind::V4l2,
                    label: format!("override gst {}", src.split_whitespace().next().unwrap_or(src)),
                    pipeline: custom_pipeline(src, flip),
                });
            } else if ov.starts_with("/dev/") {
                out.push(SourceCandidate {
                    kind: SourceKind::V4l2,
                    label: format!("override {ov}"),
                    pipeline: v4l2_pipeline(ov, flip),
                });
            } else {
                out.push(SourceCandidate {
                    kind: SourceKind::PipewireLibcamera,
                    label: format!("override {ov}"),
                    pipeline: pipewire_pipeline(ov, flip),
                });
            }
        }
    }
    for n in pw_nodes {
        out.push(SourceCandidate {
            kind: SourceKind::PipewireLibcamera,
            label: format!("libcamera {n}"),
            pipeline: pipewire_pipeline(n, flip),
        });
    }
    for d in parse_v4l2_list(v4l2_list_output) {
        if d.kind != DeviceKind::VideoCapture {
            continue; // loopbacks/virtual outputs are sinks, not cameras
        }
        out.push(SourceCandidate {
            kind: SourceKind::V4l2,
            label: format!("{} ({})", d.name, d.path),
            pipeline: v4l2_pipeline(&d.path, flip),
        });
    }
    // The override may duplicate a discovered device; trying it twice is
    // harmless but noisy, so exact-pipeline duplicates collapse to the first.
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.pipeline.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gst_override_becomes_a_custom_source_with_the_same_tail() {
        // `gst:videotestsrc pattern=ball` is how a machine with no camera
        // runs the real gst-launch path. It gets the same tail as a device
        // candidate, so `to_fdsink` retargets it like any other.
        let got = plan(&[], "", Some("gst:videotestsrc pattern=ball"), None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "override gst videotestsrc");
        assert!(got[0].pipeline.starts_with("videotestsrc pattern=ball ! videoconvert !"), "{}", got[0].pipeline);
        let fd = to_fdsink(&got[0].pipeline, 64, 48).expect("retargetable");
        assert!(fd.ends_with("fdsink fd=1"));
        assert!(fd.contains("width=64,height=48"));
        // A mirrored front preview carries its flip through too.
        let flipped = plan(&[], "", Some("gst:videotestsrc"), Some("horizontal-flip"));
        assert!(to_fdsink(&flipped[0].pipeline, 8, 8).unwrap().contains("horizontal-flip"));
    }

    // A real desktop's v4l2-ctl shape: a uvc webcam whose metadata node is
    // the second path (must not be picked), plus a v4l2loopback sink (must
    // not be a candidate at all).
    const DESKTOP: &str = "\
Integrated Camera: Integrated C (usb-0000:00:14.0-8):\n\
\t/dev/video0\n\
\t/dev/video1\n\
\n\
Dummy video device (0x0000) (platform:v4l2loopback-000):\n\
\t/dev/video10\n";

    #[test]
    fn every_pipeline_this_module_writes_can_be_retargeted_to_an_fd() {
        // The round-trip that keeps `to_fdsink` honest: it does string
        // surgery on `tail`'s output, so it must succeed on everything both
        // builders produce — plain and flipped — or the two functions have
        // drifted apart.
        for p in [
            v4l2_pipeline("/dev/video0", None),
            v4l2_pipeline("/dev/video0", Some("horizontal-flip")),
            pipewire_pipeline("libcamera_input_2_1", None),
            pipewire_pipeline("libcamera_input_2_1", Some("horizontal-flip")),
        ] {
            let fd = to_fdsink(&p, 640, 480)
                .unwrap_or_else(|| panic!("not retargetable: {p}"));
            assert!(fd.contains("fdsink fd=1"), "{fd}");
            assert!(!fd.contains("appsink"), "the appsink survived: {fd}");
            assert!(fd.contains("width=640,height=480"),
                    "the fd reader needs a fixed frame size: {fd}");
            // The source half is untouched.
            let head = p.split(" ! ").next().unwrap();
            assert!(fd.starts_with(head), "the source was rewritten: {fd}");
        }
    }

    #[test]
    fn a_front_camera_keeps_its_mirror_through_the_retarget() {
        let p = v4l2_pipeline("/dev/video2", Some("horizontal-flip"));
        let fd = to_fdsink(&p, 320, 240).unwrap();
        assert!(fd.contains("videoflip method=horizontal-flip"),
                "the selfie mirror was dropped on the capture path: {fd}");
        let plain = to_fdsink(&v4l2_pipeline("/dev/video0", None), 320, 240).unwrap();
        assert!(!plain.contains("videoflip"), "a rear camera grew a flip: {plain}");
    }

    #[test]
    fn a_foreign_pipeline_is_refused_rather_than_mangled() {
        assert_eq!(to_fdsink("videotestsrc ! autovideosink", 640, 480), None);
    }

    #[test]
    fn a_desktop_webcam_yields_one_v4l2_candidate_on_the_capture_node() {
        let p = plan(&[], DESKTOP, None, None);
        assert_eq!(p.len(), 1, "{p:#?}");
        assert_eq!(p[0].kind, SourceKind::V4l2);
        assert!(p[0].pipeline.contains("device=/dev/video0"), "{}", p[0].pipeline);
        assert!(!p[0].pipeline.contains("video1"), "metadata node leaked in");
    }

    #[test]
    fn a_phone_prefers_its_libcamera_nodes_and_keeps_v4l2_as_fallback() {
        let nodes = vec!["libcamera_input_2_1".to_string()];
        let p = plan(&nodes, DESKTOP, None, Some("rotate-180"));
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].kind, SourceKind::PipewireLibcamera);
        assert_eq!(p[1].kind, SourceKind::V4l2);
        assert!(p[0].pipeline.contains("videoflip method=rotate-180"));
    }

    #[test]
    fn an_override_goes_first_and_a_dev_path_becomes_v4l2() {
        let p = plan(&["node_a".into()], DESKTOP, Some("/dev/video2"), None);
        assert_eq!(p[0].kind, SourceKind::V4l2);
        assert!(p[0].pipeline.contains("device=/dev/video2"));
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn no_sources_means_an_empty_plan_not_a_fabricated_one() {
        assert!(plan(&[], "", None, None).is_empty());
    }

    #[test]
    fn a_v4l2_candidates_label_and_pipeline_agree_on_the_path() {
        // Regression guard for the exact bug the module header names: if the
        // label ever named one path and the pipeline opened another (e.g. a
        // metadata node leaking into the pipeline while the label still read
        // the capture node), this would still pass every other test here.
        let p = plan(&[], DESKTOP, None, None);
        for c in &p {
            if c.kind != SourceKind::V4l2 { continue; }
            let path_in_label = c.label.rsplit('(').next().unwrap_or("")
                .trim_end_matches(')');
            assert!(c.pipeline.contains(&format!("device={path_in_label}")),
                "label {:?} and pipeline {:?} disagree on the device path",
                c.label, c.pipeline);
        }
    }
}
