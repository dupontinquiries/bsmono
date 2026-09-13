//! Which decode backend to use — the DECISION, not the invocation, and
//! unproven scaffolding rather than a working feature.
//!
//! Everything actually decoding pixels in this crate today
//! (`decode::decode_range`) shells out to `ffmpeg`'s SOFTWARE decoder.
//! That is correct and fully proven (see `decode`'s own tests, including
//! cross-codec ones) but leaves real hardware decode — VA-API, NVDEC,
//! VideoToolbox — entirely unused, which matters for exactly the low-end
//! devices this whole `video` module is designed around: a phone's
//! hardware AV1 decoder is both faster and far lower power than its CPU
//! decoding the same stream in software.
//!
//! This module borrows the SHAPE of
//! `apps/snapclone/egui-frontend/src/video_backend.rs`'s encoder choice
//! — [`choose`] takes availability as INJECTED predicates rather than
//! probing, which is what makes the PRIORITY ORDER testable independently
//! of what happens to be installed on whatever machine runs the test
//! suite — because it is the right shape for this decision too, not
//! because any code is shared between them (video_backend.rs picks an
//! ENCODER for recording; this picks a DECODER for playback, and the two
//! have no logic in common beyond the pattern).
//!
//! **What this module does NOT do**: run `ffmpeg -hwaccel`, touch VA-API,
//! or upload a single pixel to a GPU texture. There is no hardware
//! decode path anywhere in this crate to select BETWEEN yet — `Backend`
//! has exactly one working variant (`Software`) and `choose` always
//! returns it until a hardware path exists and is proven the way
//! `decode`'s own byte-identical-seek tests prove the software path. The
//! other variants are typed placeholders for that future work, not a
//! claim that they run.

/// A decode backend, in priority order (software last: it always works,
/// so it is the correct final fallback rather than a rotating choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Backend {
    /// VA-API (`-hwaccel vaapi`), Linux Intel/AMD. UNIMPLEMENTED: no code
    /// path in `decode` uses this yet.
    Vaapi,
    /// NVDEC (`-hwaccel cuda`). UNIMPLEMENTED.
    Nvdec,
    /// VideoToolbox (`-hwaccel videotoolbox`), macOS/iOS. UNIMPLEMENTED.
    VideoToolbox,
    /// `ffmpeg`'s ordinary software decoder. The only backend this crate
    /// actually runs; see `decode::decode_range`.
    Software,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Vaapi => "vaapi",
            Backend::Nvdec => "nvdec",
            Backend::VideoToolbox => "videotoolbox",
            Backend::Software => "software",
        }
    }

    /// Every backend, best (most worth preferring, WHEN implemented)
    /// first. `Software` is always last and always available, so it is
    /// where every real device lands until a hardware path is built.
    pub fn priority_order() -> [Backend; 4] {
        [Backend::Vaapi, Backend::Nvdec, Backend::VideoToolbox, Backend::Software]
    }

    /// Is this backend's actual pixel path implemented in this crate?
    /// `choose` will never return a backend for which this is false — a
    /// caller cannot accidentally select unfinished work by, say, an
    /// availability predicate false-positiving on a hardware probe.
    fn implemented(self) -> bool {
        matches!(self, Backend::Software)
    }
}

/// Pick a backend given what the CALLER reports as available.
/// `hw_available` answers "does this device have working hardware decode
/// for this backend" for a non-`Software` variant; it is never consulted
/// for `Software`, which is always available by construction. Ignoring
/// `hw_available`'s answer for every backend but `Software` today is
/// intentional (see [`Backend::implemented`]) — this function's shape is
/// ready for a hardware backend the moment one exists and is proven; it
/// does not pretend one exists now.
pub fn choose(hw_available: &dyn Fn(Backend) -> bool) -> Backend {
    Backend::priority_order()
        .into_iter()
        .find(|b| b.implemented() && (*b == Backend::Software || hw_available(*b)))
        .unwrap_or(Backend::Software)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_is_chosen_regardless_of_what_hardware_reports_available() {
        // The core honesty property of this file: until a hardware
        // backend is IMPLEMENTED (not just detected), it must never be
        // returned, even if every hardware probe says yes.
        assert_eq!(choose(&|_| true), Backend::Software);
        assert_eq!(choose(&|_| false), Backend::Software);
    }

    #[test]
    fn priority_order_ends_with_software_and_lists_every_backend_once() {
        let order = Backend::priority_order();
        assert_eq!(order.last(), Some(&Backend::Software));
        let mut sorted = order.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), order.len(), "no backend listed twice");
    }

    #[test]
    fn only_software_is_implemented_today() {
        assert!(Backend::Software.implemented());
        assert!(!Backend::Vaapi.implemented());
        assert!(!Backend::Nvdec.implemented());
        assert!(!Backend::VideoToolbox.implemented());
    }
}
