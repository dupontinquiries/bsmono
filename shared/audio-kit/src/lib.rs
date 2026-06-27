//! audio-kit — shared DSP primitives.
//!
//! The leaf audio building blocks the apps kept re-implementing: signal level
//! (RMS / peak / dB), the Goertzel magnitude plus a Goertzel-based narrow-band
//! spectrogram, fundamental-frequency (pitch) estimation, analysis windows, and
//! tone / noise generators. A structural-clone scan put the single largest
//! duplication in the repo here — bo-suite's `audio_features` was copied verbatim
//! across bo-mux and bo-controller, and `rms` / `goertzel` / pitch estimation
//! recur in voiceterm and snapclone-audio.
//!
//! audio-kit holds the PRIMITIVES; each app's strategies are consumers:
//!   * rusty-board (bo-suite): the tone/drone cancellers, spectral masker, and
//!     backup beeper build on [`goertzel`](goertzel::goertzel) / [`pitch`] / [`gen`].
//!   * voiceterm / web2qml: speaker separation + voice features build on
//!     [`level::rms`] / [`goertzel`] / [`window`].
//!   * logscope: voice reactions (level/voicing gating of an in-flight generation)
//!     build on [`level::rms`] / [`pitch`].
//!   * unendlich: game-audio synthesis builds on [`gen`].
//!
//! Everything is a pure function or a small state struct over `&[f32]` /
//! `f32`. Zero-dependency (std + f32 math) — builds offline and links into every
//! target. The opinionated, stateful processors (a streaming canceller, a
//! diarizer) stay in the apps; this crate is the math beneath them.

pub mod gen;
pub mod goertzel;
pub mod level;
pub mod pitch;
pub mod window;

pub use gen::{Noise, Tone};
pub use goertzel::{goertzel, spectrogram, spectrogram_at_freqs, SpectrogramConfig};
pub use level::{db_to_lin, lin_to_db, peak, rms};
pub use pitch::estimate_fundamental;
pub use window::Window;
