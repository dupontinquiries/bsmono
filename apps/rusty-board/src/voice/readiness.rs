//! Voice readiness state machine.
//!
//! The whisper pipeline is expensive in two ways:
//!   1. The model sitting in RAM (~50-200MB) — addressed by the
//!      auto-shutdown timer in `service.rs`.
//!   2. The high-frequency polling timers (30Hz) the UI runs to
//!      pump audio levels and partial transcripts — these run
//!      forever even when voice is idle, burning CPU wakeups.
//!
//! This FSM coordinates BOTH. It tracks a small set of "proximity"
//! signals and decides, every tick, what readiness state voice
//! should be in:
//!
//! ```text
//!   Asleep ──(proximity)──► Warm ──(record start)──► Active
//!     ▲                       │                         │
//!     └────(idle timeout)─────┘◄───(record stop)────────┘
//! ```
//!
//! - **Asleep**: transcriber may be released; UI polls slowly
//!   (250ms). This is the resting state when the user shows no
//!   sign of wanting voice.
//! - **Warm**: transcriber loaded and ready, UI polls at a medium
//!   rate (100ms). Entered when a proximity signal suggests voice
//!   might be used in the next few seconds (e.g. the keyboard just
//!   appeared, or the user tapped near the voice key).
//! - **Active**: recording in progress; UI polls fast (33ms) for
//!   smooth level meter + partial transcripts.
//!
//! The proximity heuristic deliberately errs toward Warm when
//! unsure, because a cold-start model load (1-2s) on the first tap
//! is a worse user experience than a few seconds of extra RAM.
//! But it returns to Asleep aggressively once those signals lapse,
//! so steady-state idle keyboards don't hold the model or poll fast.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

/// Readiness state. Ordered by "hotness" so we can compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Asleep,
    Warm,
    Active,
}

impl Readiness {
    fn from_u8(v: u8) -> Readiness {
        match v {
            2 => Readiness::Active,
            1 => Readiness::Warm,
            _ => Readiness::Asleep,
        }
    }
    fn as_u8(self) -> u8 {
        match self {
            Readiness::Asleep => 0,
            Readiness::Warm => 1,
            Readiness::Active => 2,
        }
    }

    /// UI polling cadence in milliseconds for this state. The voice
    /// level / partial timers reschedule themselves at this rate.
    ///
    /// Asleep is the steady state when the keyboard is visible but
    /// voice isn't being used. There's nothing to update at the
    /// pump level (no level meter, no partial transcript), and
    /// transitions out of Asleep are event-driven via
    /// signal_proximity / set_recording which write the FSM state
    /// atomically. So the pump can sleep aggressively here — the
    /// only cost is that the NEXT pump tick after a transition has
    /// up to one IDLE interval of latency before noticing the new
    /// state. At 2 seconds, a focus event that bumps to Warm will
    /// be picked up well before the user finishes reaching for
    /// the mic key, and the level meter still starts updating
    /// promptly once recording begins (since set_recording bumps
    /// to Active, and Active uses 33ms cadence).
    pub fn poll_interval_ms(self) -> u64 {
        match self {
            Readiness::Asleep => 2000,
            Readiness::Warm => 100,
            Readiness::Active => 33,
        }
    }
}

/// How long a single proximity signal keeps the FSM Warm before it
/// lapses back toward Asleep (assuming no new signals). 8 seconds
/// is long enough that a user reaching for the voice key after
/// reading a sentence still finds it warm, but short enough that an
/// idle keyboard returns to Asleep promptly.
const WARM_LINGER_SECS: u64 = 8;

/// Process-wide FSM. Lock-free: state in an AtomicU8, last-signal
/// timestamp in an AtomicU64 (epoch seconds).
pub struct VoiceReadinessFsm {
    state: AtomicU8,
    /// Epoch seconds of the most recent proximity signal.
    last_signal: AtomicU64,
    /// Whether recording is currently active. Set by the service
    /// bridge on record start/stop; forces Active state.
    recording: std::sync::atomic::AtomicBool,
}

impl VoiceReadinessFsm {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(0), // Asleep
            last_signal: AtomicU64::new(0),
            recording: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Register a "voice might be used soon" signal. Examples:
    ///   - keyboard just became visible
    ///   - the user tapped a key adjacent to the voice key
    ///   - the microterm opened
    ///   - the user enabled voice in settings
    ///
    /// Bumps the FSM to at least Warm and resets the linger clock.
    pub fn signal_proximity(&self) {
        self.last_signal.store(now_secs(), Ordering::Relaxed);
        // Only promote to Warm if we're currently Asleep. Don't
        // demote Active.
        let cur = Readiness::from_u8(self.state.load(Ordering::Relaxed));
        if cur == Readiness::Asleep {
            self.state.store(Readiness::Warm.as_u8(), Ordering::Relaxed);
        }
    }

    /// Record start/stop. Recording forces Active; stopping returns
    /// to Warm (so the next utterance starts fast) and resets the
    /// linger clock.
    pub fn set_recording(&self, recording: bool) {
        self.recording.store(recording, Ordering::Relaxed);
        if recording {
            self.state.store(Readiness::Active.as_u8(), Ordering::Relaxed);
        } else {
            self.state.store(Readiness::Warm.as_u8(), Ordering::Relaxed);
            self.last_signal.store(now_secs(), Ordering::Relaxed);
        }
    }

    /// Tick the FSM: recompute the state based on elapsed time since
    /// the last signal. Called by the coordinator timer (slow — once
    /// per second is plenty since transitions are second-scale).
    /// Returns the (possibly changed) current state.
    pub fn tick(&self) -> Readiness {
        // Recording always wins.
        if self.recording.load(Ordering::Relaxed) {
            self.state.store(Readiness::Active.as_u8(), Ordering::Relaxed);
            return Readiness::Active;
        }
        let cur = Readiness::from_u8(self.state.load(Ordering::Relaxed));
        let idle = now_secs().saturating_sub(self.last_signal.load(Ordering::Relaxed));
        let next = match cur {
            Readiness::Active => {
                // Not recording but state says Active — shouldn't
                // happen, but recover to Warm.
                Readiness::Warm
            }
            Readiness::Warm => {
                if idle >= WARM_LINGER_SECS {
                    Readiness::Asleep
                } else {
                    Readiness::Warm
                }
            }
            Readiness::Asleep => Readiness::Asleep,
        };
        if next != cur {
            self.state.store(next.as_u8(), Ordering::Relaxed);
        }
        next
    }

    pub fn current(&self) -> Readiness {
        Readiness::from_u8(self.state.load(Ordering::Relaxed))
    }
}

/// Current epoch seconds.
fn now_secs() -> u64 {
        core_kit::time::now_secs()
    }

/// Process-wide singleton.
pub fn global() -> &'static VoiceReadinessFsm {
    static FSM: VoiceReadinessFsm = VoiceReadinessFsm::new();
    &FSM
}
