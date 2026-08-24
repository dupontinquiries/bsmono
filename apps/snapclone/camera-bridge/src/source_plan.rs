//! source_plan — which camera source to try, in what order, as gst pipelines.
//!
//! The desktop-webcam gap (c11 field report): capture was pipewiresrc +
//! libcamera only — `probe_pw_nodes` greps pw-dump for `libcamera_input`
//! names, which exist on postmarketOS and never on a desktop, whose webcams
//! are v4l2. The Qt build never had the problem because Qt Multimedia
//! enumerates the platform backend itself. This module is that enumeration's
//! DECISION half: pure, fed by probe text, producing an ordered list of
//! pipeline candidates the capture thread tries until one plays.
//!
//! Order: an explicit operator override first (a `/dev/video*` path becomes a
//! v4l2 candidate, anything else a pipewire node); then libcamera pipewire
//! nodes (a phone that has them wants them — v4l2 on those kernels is the
//! raw sensor, wrong for a viewfinder); then v4l2 capture devices
//! (device-kit's parser already takes one node per device group, which is
//! what dodges uvcvideo's metadata-node trap, and classes loopback/Dummy as
//! VideoOutput so a virtual sink is never a capture candidate).
//!
//! An empty plan is an empty plan: the caller shows "no camera found" — a
//! fabricated test-source candidate here would turn a detection bug into a
//! convincing grey rectangle, which is the failure mode this port already
//! had once.

use device_kit::{parse_v4l2_list, DeviceKind};

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

/// The one sink shape both sources feed (matches `build_pipeline`'s tail —
/// the appsink name and caps the view thread expects).
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

pub fn pipewire_pipeline(node: &str, flip: Option<&str>) -> String {
    format!("pipewiresrc target-object=\"{node}\" ! {}", tail(flip))
}

pub fn v4l2_pipeline(path: &str, flip: Option<&str>) -> String {
    format!("v4l2src device={path} ! {}", tail(flip))
}

/// Build the ordered plan. `pw_nodes` from the existing pw-dump probe;
/// `v4l2_list_output` is `v4l2-ctl --list-devices` text (empty when the tool
/// is absent — a desktop without v4l2-utils simply contributes nothing).
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
            if ov.starts_with("/dev/") {
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

    /// The v4l2 pipelines PARSE against a real GStreamer with the real
    /// v4l2src element — parse does not open the device, so this runs on a
    /// camera-less machine and still catches syntax and missing-element
    /// errors, which a string assertion cannot. The pipewire pipeline is
    /// checked structurally only: this sandbox has no pipewiresrc plugin,
    /// and a parse test that skips when the plugin is missing would be a
    /// pass guaranteed by the absence of the thing under test.
    #[test]
    fn generated_v4l2_pipelines_parse_against_real_gstreamer() {
        gstreamer::init().expect("gst init");
        let p = plan(&["n".into()], DESKTOP, Some("/dev/video9"), Some("horizontal-flip"));
        let v4l2: Vec<_> = p.iter().filter(|c| c.kind == SourceKind::V4l2).collect();
        assert!(v4l2.len() >= 2, "the scan found v4l2 candidates to validate");
        for c in &v4l2 {
            gstreamer::parse::launch(&c.pipeline)
                .unwrap_or_else(|e| panic!("candidate {:?} does not parse: {e}", c.label));
        }
        let pw = p.iter().find(|c| c.kind == SourceKind::PipewireLibcamera).unwrap();
        assert!(pw.pipeline.starts_with("pipewiresrc target-object=\"n\""));
        assert!(pw.pipeline.contains("appsink name=sink"));
    }
}
