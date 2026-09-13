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
//!   SAME byte budget (a reallocation, never an addition); the
//!   `Preview`/`Render`/`Feel` mode enum; and `Strictness` — three named
//!   levels (not a slider) for how much a hint is trusted to act and how
//!   much cache churn it may cause, because that is a hardware-class
//!   decision. `device_prefs::DevicePrefs::bucket_strictness` is where a
//!   device's choice is kept; `HotkeyAction` is the (not yet bound —
//!   there is no video timeline in the view to bind it to) stricter/
//!   looser/reset logic a keyboard handler will eventually call.
//! * `probe` — real file metadata and exact keyframe positions via
//!   `ffprobe`, tested against real fixtures ffmpeg encodes on the fly
//!   (guarded: skips cleanly on a build without the needed encoders).
//! * `decode` — the real pixel path: `ffmpeg`-backed, GOP-aware using
//!   `probe::keyframes`, scaled per tier by `ffmpeg`'s own scaler rather
//!   than decode-then-downsample. Its seek-and-discard correctness is
//!   PROVEN byte-identical to decoding from frame zero, for both AV1 and
//!   H.264-with-B-frames (a codec where decode order and display order
//!   genuinely diverge) — not assumed for either.
//! * `source` — `import()`: a video file into the arrangement as
//!   separable picture (`VideoAsset`, no pixels decoded yet) and sound
//!   (a plain audio `Sample`, the same type any other import produces).
//! * `backend` — WHICH decoder to run pixels through, in the shape of
//!   `apps/snapclone`'s encoder-choice module (availability injected, so
//!   the priority order is testable without probing real hardware). Only
//!   `Backend::Software` is actually implemented — the others are typed
//!   placeholders for VA-API/NVDEC/VideoToolbox, not a claim any of them
//!   run; see the module's own doc for why decode has no software
//!   fallback the way `gl_playback` does for render.
//! * `gl_playback` (feature `gpu-playback`) — the DISPLAY half proven for
//!   real: a decoded frame uploaded as an actual GPU texture, sampled by
//!   an actual fragment shader, rendered to an offscreen framebuffer, and
//!   read back pixel-exact — headless, via surfaceless EGL, so it runs
//!   identically on real hardware or on Mesa's software rasterizer where
//!   no GPU exists. Proven against both a synthetic pattern and a real
//!   frame decoded from an AV1 fixture by `decode::decode_range`. This is
//!   NOT part of the fleet's wgpu-version table (`C18_TODO.md`) — it is
//!   plain OpenGL/EGL via `glow`, chosen to match `eframe`'s own `glow`
//!   backend (see this crate's `Cargo.toml` comment on why `app` forces
//!   glow over wgpu), and has nothing to do with the `gpu` feature's
//!   candle/wgpu training backend either. Three separate GPU-adjacent
//!   things in this one crate; none of them are each other.
//!
//! Nothing here has been wired into the egui view yet — no clip on the
//! timeline plays a `VideoAsset`, no frame drawn by `gl_playback` has
//! reached an actual window. The missing piece is entirely integration:
//! a `SourceRef::Video` variant on `arrange::model::Clip`, a monitor
//! panel in `arrangepage.rs` that drives `cache_policy`/`feel` from the
//! playhead and calls `decode` + `gl_playback` for the resulting frames,
//! and — per the fleet's own beatspace precedent (`0ac14413`, "workspace
//! on desktop vs tabs on mobile") — a layout that is a real panel on a
//! desktop-width screen and a tab on a phone-width one, not a shrunk copy
//! of the same panel.

pub mod backend;
pub mod bucket;
pub mod cache_policy;
pub mod decode;
pub mod feel;
#[cfg(feature = "gpu-playback")]
pub mod gl_playback;
pub mod probe;
pub mod profile;
pub mod source;
