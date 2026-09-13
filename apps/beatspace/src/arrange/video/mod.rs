//! video — AV1 clips on the timeline as separable picture and sound, with
//! three playback fidelities (`feel::Mode`) instead of one.
//!
//! * `cache_policy` — what to decode and hold, purely from playhead
//!   position, velocity, and a byte budget. This is Preview mode's whole
//!   story, and Feel mode's base layer.
//! * `bucket` — content importance (`Bucket`) resolved at three confidence
//!   levels: a verified profiled segment, a guess borrowed from nearby
//!   segments (decayed by distance and by crossing a clip boundary), or a
//!   neutral fallback that IS `cache_policy`'s plain distance falloff.
//! * `profile` (planned) — the cheap analyzer that produces verified
//!   segments, and the freeze/fingerprint/verify machinery that catches a
//!   profile going stale against a re-encoded source or a tuned analyzer.
//! * `feel` (planned) — combines a `BucketField` resolution with
//!   `cache_policy::Shells` to bend the falloff by importance under a
//!   fixed byte budget, and the `Preview`/`Render`/`Feel` mode switch.
//! * `decode` (planned) — the real pixel path: ffmpeg-backed, GOP-aware,
//!   scaled per tier by ffmpeg's own scaler rather than decode-then-
//!   downsample.
//!
//! Nothing here has been wired into the egui view yet, and `decode` in
//! particular needs a real display to prove "realtime" rather than just
//! "correct" — see the module's own doc for exactly what has and hasn't
//! been run.

pub mod bucket;
pub mod cache_policy;
pub mod profile;
