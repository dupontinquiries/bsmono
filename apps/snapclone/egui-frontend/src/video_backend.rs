//! Which video encoder to use — the decision, not the invocation.
//!
//! C14_TODO §15 step 2, split so the part that needs no gstreamer and no
//! ffmpeg can be tested here. Choosing a backend is a pure function of what is
//! installed; running one is not.
//!
//! # Ported from the Qt chain, order and reasons intact
//!
//! `VideoEncoderBackend.cpp`'s `allBackendsInPriorityOrder()` is the reference,
//! and the order carries reasoning that is not obvious from the names:
//!
//! * **ffmpeg's libsvtav1 before gstreamer's `svtav1enc`.** The Qt comment
//!   gives three reasons: ffmpeg's wrapper tracks upstream SVT-AV1 more
//!   aggressively, it emits accurate `frame=NNN ... time=` progress on stderr,
//!   and hardware paths (vaapi, qsv) are reachable behind it. Its cost is that
//!   ffmpeg is "not always present on minimal mobile" — which is exactly why
//!   the gstreamer path stays as the next choice rather than being dropped.
//! * **VP9 and VP8 after AV1**, as fallbacks for a device whose gstreamer has
//!   no AV1 element at all.
//! * **VVC last and opt-in.** The Qt comment: "Never the default; never
//!   displaces AV1." Kept behind the same environment variable so that stays
//!   true.
//!
//! Reimplementing that order from the names alone would have got it backwards —
//! gstreamer looks like the obvious first choice on a phone. This is the second
//! time on this feature that reading the Qt tree changed the answer; see §15.
//!
//! # Availability is injected
//!
//! [`choose`] takes two predicates rather than probing. That is what makes the
//! ORDER testable — which matters more than the probes, because a wrong probe
//! fails loudly on a device and a wrong order silently picks the worse encoder
//! forever.

/// A backend, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Backend {
    /// ffmpeg CLI driving libsvtav1. First choice; see the module note.
    FfmpegSvtAv1,
    /// gstreamer `svtav1enc`. Same codec, fewer knobs, but present on mobile
    /// images that ship no ffmpeg.
    GstSvtAv1,
    /// gstreamer `vp9enc`. Not AV1, but universally decodable.
    GstVp9,
    /// gstreamer `vp8enc`. Last resort that still produces a playable file.
    GstVp8,
    /// ffmpeg with libvvenc. Opt-in only, never chosen by default.
    FfmpegVvc,
}

impl Backend {
    /// The name to log, matching the Qt backend's own strings so a bug report
    /// from either front end reads the same.
    pub fn name(self) -> &'static str {
        match self {
            Backend::FfmpegSvtAv1 => "ffmpeg-libsvtav1",
            Backend::GstSvtAv1 => "gst-svtav1enc",
            Backend::GstVp9 => "gst-vp9enc",
            Backend::GstVp8 => "gst-vp8enc",
            Backend::FfmpegVvc => "ffmpeg-vvc",
        }
    }

    /// Is this AV1? Used to decide whether `snapclone_av1policy`'s plan
    /// applies — a VP8 fallback must not be handed AV1 CRF numbers, because
    /// VP9's and AV1's quality scales are not the same range and 32 means
    /// different things in each.
    pub fn is_av1(self) -> bool {
        matches!(self, Backend::FfmpegSvtAv1 | Backend::GstSvtAv1)
    }

    /// Every backend, best first. The order IS the ported knowledge.
    pub fn priority_order() -> [Backend; 5] {
        [
            Backend::FfmpegSvtAv1,
            Backend::GstSvtAv1,
            Backend::GstVp9,
            Backend::GstVp8,
            Backend::FfmpegVvc,
        ]
    }

    /// What has to exist for this backend to run.
    ///
    /// Returned rather than probed inline so a caller can report WHAT is
    /// missing. "No encoder available" sends someone looking at the app;
    /// "svtav1enc element missing" sends them to their gstreamer packages,
    /// which is where the problem is.
    pub fn requires(self) -> Requirement {
        match self {
            Backend::FfmpegSvtAv1 => Requirement::FfmpegEncoder("libsvtav1"),
            Backend::GstSvtAv1 => Requirement::GstElement("svtav1enc"),
            Backend::GstVp9 => Requirement::GstElement("vp9enc"),
            Backend::GstVp8 => Requirement::GstElement("vp8enc"),
            Backend::FfmpegVvc => Requirement::FfmpegEncoder("libvvenc"),
        }
    }
}

/// What a backend needs present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// A gstreamer element of this name.
    GstElement(&'static str),
    /// An ffmpeg build listing this encoder.
    ///
    /// The ENCODER, not the binary. `d2x-job`'s codec preflight exists because
    /// checking for ffmpeg and checking for an encoder inside it are different
    /// questions with different failures — a stripped ffmpeg passes the first
    /// and dies at the second, an hour into a job.
    FfmpegEncoder(&'static str),
}

/// Environment variable that lets VVC be considered at all.
pub const VVC_OPT_IN: &str = "SNAPCLONE_ENABLE_VVC";

/// Pick a backend given what is installed.
///
/// `gst_element` and `ffmpeg_encoder` answer "is this available". `vvc_opt_in`
/// is passed separately rather than read from the environment here, so the rule
/// that VVC never wins by accident is testable.
pub fn choose(
    gst_element: &dyn Fn(&str) -> bool,
    ffmpeg_encoder: &dyn Fn(&str) -> bool,
    vvc_opt_in: bool,
) -> Option<Backend> {
    Backend::priority_order().into_iter().find(|b| {
        if *b == Backend::FfmpegVvc && !vvc_opt_in {
            return false;
        }
        match b.requires() {
            Requirement::GstElement(n) => gst_element(n),
            Requirement::FfmpegEncoder(n) => ffmpeg_encoder(n),
        }
    })
}

/// Everything that was tried and why it was not usable.
///
/// For the log line when [`choose`] returns `None`. A recording that silently
/// does not start is the worst outcome here — the user has pressed a shutter
/// and believes something happened.
pub fn unavailable_report(
    gst_element: &dyn Fn(&str) -> bool,
    ffmpeg_encoder: &dyn Fn(&str) -> bool,
    vvc_opt_in: bool,
) -> Vec<String> {
    Backend::priority_order()
        .into_iter()
        .filter_map(|b| {
            if b == Backend::FfmpegVvc && !vvc_opt_in {
                return Some(format!("{}: not enabled ({VVC_OPT_IN})", b.name()));
            }
            let (kind, what, have) = match b.requires() {
                Requirement::GstElement(n) => ("gstreamer element", n, gst_element(n)),
                Requirement::FfmpegEncoder(n) => ("ffmpeg encoder", n, ffmpeg_encoder(n)),
            };
            (!have).then(|| format!("{}: {kind} `{what}` missing", b.name()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }
    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn ffmpeg_av1_beats_gstreamer_av1() {
        // The ported order, and the one a rewrite would get backwards:
        // gstreamer looks like the obvious first choice on a phone. The Qt
        // comment gives the reasons — better upstream tracking, real progress
        // output, reachable hardware paths.
        let both = |n: &str| n == "svtav1enc" || n == "libsvtav1";
        assert_eq!(choose(&both, &both, false), Some(Backend::FfmpegSvtAv1));
    }

    #[test]
    fn gstreamer_av1_is_used_when_ffmpeg_has_no_av1() {
        // The case the Qt comment names: ffmpeg is "not always present on
        // minimal mobile", which is why the gst path stays rather than being
        // dropped once ffmpeg was preferred.
        let gst = |n: &str| n == "svtav1enc";
        assert_eq!(choose(&gst, &none, false), Some(Backend::GstSvtAv1));
    }

    #[test]
    fn vp9_then_vp8_when_there_is_no_av1_at_all() {
        // Both steps, because a chain that fell straight to VP8 would pass a
        // test that only checked "something was chosen".
        let vp9 = |n: &str| n == "vp9enc" || n == "vp8enc";
        assert_eq!(choose(&vp9, &none, false), Some(Backend::GstVp9));
        let vp8 = |n: &str| n == "vp8enc";
        assert_eq!(choose(&vp8, &none, false), Some(Backend::GstVp8));
    }

    #[test]
    fn vvc_never_wins_unless_it_is_opted_into() {
        // The Qt comment is explicit: "Never the default; never displaces
        // AV1." Two directions — it must not be picked when everything is
        // available, and it must not be picked when it is the ONLY thing
        // available, because that is where an accidental default would hide.
        assert_eq!(choose(&all, &all, true), Some(Backend::FfmpegSvtAv1),
                   "VVC displaced AV1 with the opt-in on");
        let only_vvc = |n: &str| n == "libvvenc";
        assert_eq!(choose(&none, &only_vvc, false), None,
                   "VVC was chosen without the opt-in");
        assert_eq!(choose(&none, &only_vvc, true), Some(Backend::FfmpegVvc));
    }

    #[test]
    fn nothing_available_is_none_and_says_what_was_missing() {
        // A recording that silently does not start is the worst outcome: the
        // user pressed a shutter and believes something happened.
        assert_eq!(choose(&none, &none, false), None);
        let r = unavailable_report(&none, &none, false);
        assert_eq!(r.len(), 5, "not every backend was reported: {r:?}");
        assert!(r.iter().any(|l| l.contains("libsvtav1")));
        assert!(r.iter().any(|l| l.contains("svtav1enc")));
        // And the VVC line explains the variable rather than looking like a
        // missing package.
        assert!(r.iter().any(|l| l.contains(VVC_OPT_IN)), "{r:?}");
    }

    #[test]
    fn only_the_av1_backends_take_an_av1_plan() {
        // VP8/VP9 and AV1 do not share a quality scale — CRF 32 means
        // different things — so handing a VP8 fallback the AV1 plan's numbers
        // produces a file at the wrong quality with no error.
        assert!(Backend::FfmpegSvtAv1.is_av1());
        assert!(Backend::GstSvtAv1.is_av1());
        assert!(!Backend::GstVp9.is_av1());
        assert!(!Backend::GstVp8.is_av1());
        assert!(!Backend::FfmpegVvc.is_av1(), "VVC is not AV1");
    }

    #[test]
    fn the_logged_names_match_the_qt_backends() {
        // So a bug report from either front end reads the same. Diverging here
        // costs nothing until someone greps a log for a name that only one of
        // the two apps uses.
        assert_eq!(Backend::FfmpegSvtAv1.name(), "ffmpeg-libsvtav1");
        assert_eq!(Backend::GstSvtAv1.name(), "gst-svtav1enc");
        assert_eq!(Backend::GstVp9.name(), "gst-vp9enc");
        assert_eq!(Backend::GstVp8.name(), "gst-vp8enc");
    }
}
