//! Voice input via whisper-rs.
//!
//! Architecture:
//! - `recorder` runs a `cpal` input stream into a ring buffer.
//! - `transcriber` wraps `whisper-rs` for streaming + final transcription.
//! - `service` orchestrates them: start/stop, partial transcripts, audio level.

#[cfg(feature = "voice")]
pub mod hallucination;
#[cfg(feature = "voice")]
pub mod recorder;
#[cfg(feature = "voice")]
pub mod service;
#[cfg(feature = "voice")]
pub mod transcriber;

// The readiness FSM is pure state logic with no whisper dependency,
// so it's always available — the UI's timer-gating code references
// it regardless of whether the voice feature is compiled in.
pub mod readiness;

// Model-path resolution is also dependency-free (just std + directories
// crate), and putting it outside the voice feature gate means tests
// run in every build config — including --no-default-features.
pub mod model_path;

#[cfg(feature = "voice")]
pub use service::{VoiceService, VoiceServiceState, VOICE_AUTO_SHUTDOWN_SECS};

// Stubs when voice is disabled, so the rest of the codebase can compile.
#[cfg(not(feature = "voice"))]
pub mod service {
    use anyhow::Result;

    pub struct VoiceService;

    impl VoiceService {
        pub fn new() -> Result<Self> {
            anyhow::bail!("voice feature is disabled at compile time")
        }
        pub fn start_recording(&self) -> Result<()> {
            anyhow::bail!("voice feature is disabled at compile time")
        }
        pub fn stop_recording(&self) -> Result<String> {
            anyhow::bail!("voice feature is disabled at compile time")
        }
    }
}
