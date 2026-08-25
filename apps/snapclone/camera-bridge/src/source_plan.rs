//! source_plan — re-exported from `device-kit`.
//!
//! This module used to hold the desktop-webcam-gap fix directly (pw-dump
//! probing for libcamera nodes falls back to v4l2 candidates on a desktop,
//! which has none). Moved into `device-kit` so `apps/snapclone/egui-frontend`'s
//! `camera_source.rs` — which only picks a *device*, with no notion that a
//! phone wants pipewire/libcamera tried first and a desktop only has v4l2 —
//! can share the exact same candidate ordering instead of re-deriving it and
//! risking the two disagreeing about which pipeline to try first.
//!
//! What stays here, and can only stay here: the one test that links a real
//! GStreamer to confirm the generated pipeline strings actually PARSE
//! (`gstreamer::parse::launch`) — device-kit is deliberately std-only with
//! zero dependencies, so a parse-against-real-gstreamer test cannot live
//! there without dragging that dependency into every other consumer
//! (bo-suite, voiceterm, logscope, beatspace) that only wants the pure
//! candidate-ordering logic.

pub use device_kit::source_plan::{pipewire_pipeline, plan, v4l2_pipeline, SourceCandidate, SourceKind};

#[cfg(test)]
mod tests {
    use super::*;

    // Same fixture as device-kit's own tests, kept local so this file is
    // self-contained rather than reaching into a sibling crate's test module.
    const DESKTOP: &str = "\
Integrated Camera: Integrated C (usb-0000:00:14.0-8):\n\
\t/dev/video0\n\
\t/dev/video1\n\
\n\
Dummy video device (0x0000) (platform:v4l2loopback-000):\n\
\t/dev/video10\n";

    /// The v4l2 pipelines PARSE against a real GStreamer with the real
    /// v4l2src element — parse does not open the device, so this runs on a
    /// camera-less machine and still catches syntax and missing-element
    /// errors, which a string assertion cannot. The pipewire pipeline is
    /// checked structurally only: a sandbox without the pipewiresrc plugin
    /// makes a parse test that skips when the plugin is missing a pass
    /// guaranteed by the absence of the thing under test.
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
