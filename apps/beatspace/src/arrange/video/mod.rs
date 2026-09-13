//! video — AV1 clips on the timeline as separable picture and sound, with
//! three playback fidelities (`feel::Mode`) instead of one.
//!
//! * `cache_policy` — what to decode and hold, purely from playhead
//!   position, velocity, and a byte budget. This is Preview mode's whole
//!   story, and Feel mode's base layer.
//! * `bucket` — content importance (`Bucket`) resolved at three confidence
//!   levels: a verified profiled segment, a guess borrowed from nearby
//!   segments, or a neutral fallback that IS `cache_policy`'s plain
//!   distance falloff. Segments are keyed by SOURCE, never by timeline
//!   clip, so Split/Trim/Move/Duplicate can never desync them — see the
//!   module's own doc for why that split matters.
//! * `profile` — the cheap analyzer (motion, cut detection, a named-crude
//!   skin-tone proxy for "maybe a face") that produces verified segments,
//!   and the freeze/fingerprint/verify machinery that catches a profile
//!   going stale against a re-encoded source or a tuned analyzer.
//! * `feel` — combines a `BucketField` resolution with a `cache_policy`
//!   desired set to bend the falloff toward important content under the
//!   SAME byte budget (a reallocation, never an addition), and the
//!   `Preview`/`Render`/`Feel` mode enum.
//! * `probe` — real file metadata and exact keyframe positions via
//!   `ffprobe`, tested against a real AV1+Opus fixture ffmpeg encodes on
//!   the fly (guarded: skips cleanly on a build without libaom/libopus).
//! * `decode` (not yet built) — the real pixel path: ffmpeg-backed,
//!   GOP-aware using `probe::keyframes`, scaled per tier by ffmpeg's own
//!   scaler rather than decode-then-downsample.
//!
//! Nothing here has been wired into the egui view yet, and `decode` in
//! particular needs a real display to prove "realtime" rather than just
//! "correct" once it exists.

pub mod bucket;
pub mod cache_policy;
pub mod feel;
pub mod probe;
pub mod profile;
