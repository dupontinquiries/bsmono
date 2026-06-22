//! radio.rs — "talk modes" for the Buzz radio / push-to-talk voice feature.
//!
//! This is the pure decision layer that answers ONE question every audio frame:
//! *should the local microphone be transmitting right now?* It does not touch
//! the mic, the codec, or the network — the caller drives those. It just owns
//! the talk-mode policy so the behaviour is testable in isolation (the rest of
//! Buzz — Opus encode, sealing to the peer/group, transport — is Qt/runtime
//! glue that can't be unit-tested here).
//!
//! ## The modes (see proposal_buzz_walkie_talkie.md §4)
//!
//!   • **Off**            — monitor only; never transmit (you can still hear).
//!   • **PushToTalk**     — transmit exactly while the talk control is held.
//!   • **VoiceActivated** — VOX: open when speech is detected, with squelch
//!                          hysteresis + hang-time so a pause mid-sentence
//!                          doesn't chop the transmission. Driven by the
//!                          `snapclone-audio` VAD likelihood the caller already
//!                          computes per frame.
//!   • **Continuous**     — always open while the channel is up (hands-free).
//!
//! ## Why hysteresis + hang + min-open
//!
//! A single threshold on a noisy VAD score "chatters" — it flips open/closed
//! many times a second right at the boundary, which sounds like the radio
//! stuttering. So VOX uses TWO thresholds (open > close) for hysteresis, a
//! `hang_ms` tail that keeps the channel open through short gaps between words,
//! and a `min_open_ms` floor so a single loud syllable still produces a clean
//! short burst rather than a click.
//!
//! ## Time
//!
//! The caller passes a monotonic millisecond clock (`now_ms`) into `update` —
//! no syscalls inside, which keeps the squelch timing deterministic and
//! testable. Pass any steady clock (e.g. a frame counter × frame-ms).

use std::os::raw::c_int;

/// How the local mic decides whether to transmit. `#[repr(C)]` so the C++ side
/// can pass it across the FFI as a plain int.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(C)]
pub enum TalkMode {
    Off = 0,
    PushToTalk = 1,
    VoiceActivated = 2,
    Continuous = 3,
}

impl TalkMode {
    /// Map a raw FFI int to a mode; unknown values fall back to Off (safe — the
    /// mic stays closed rather than transmitting unexpectedly).
    pub fn from_raw(v: c_int) -> TalkMode {
        match v {
            1 => TalkMode::PushToTalk,
            2 => TalkMode::VoiceActivated,
            3 => TalkMode::Continuous,
            _ => TalkMode::Off,
        }
    }
}

/// Squelch / VOX tuning. `open > close` gives the hysteresis band.
#[derive(Clone, Copy, Debug)]
pub struct SquelchConfig {
    /// VAD likelihood (0..1) at/above which a CLOSED gate OPENS.
    pub open_threshold: f32,
    /// VAD likelihood below which an OPEN gate may close (must be < open).
    pub close_threshold: f32,
    /// Keep transmitting this long after the likelihood last fell below
    /// `close_threshold`, so pauses between words don't clip the transmission.
    pub hang_ms: u64,
    /// Once opened, stay open at least this long (anti-stutter on a single
    /// loud syllable).
    pub min_open_ms: u64,
}

impl Default for SquelchConfig {
    fn default() -> Self {
        SquelchConfig {
            open_threshold: 0.60,
            close_threshold: 0.40,
            hang_ms: 800,
            min_open_ms: 250,
        }
    }
}

impl SquelchConfig {
    /// Keep the invariant open > close even if a caller mis-sets it (a degenerate
    /// band would make VOX chatter). Nudges close just under open if needed.
    fn sane(mut self) -> Self {
        if !(self.open_threshold > self.close_threshold) {
            self.close_threshold = (self.open_threshold - 0.1).max(0.0);
        }
        self
    }
}

/// Stateful transmit gate. Construct once per channel session; call `update`
/// once per audio frame with the current mode, PTT-button state, the latest VAD
/// likelihood, and a monotonic clock. Returns whether to transmit this frame.
pub struct TransmitGate {
    cfg: SquelchConfig,
    transmitting: bool,
    opened_at_ms: u64,
    last_voice_ms: u64,
}

impl TransmitGate {
    pub fn new(cfg: SquelchConfig) -> Self {
        TransmitGate {
            cfg: cfg.sane(),
            transmitting: false,
            opened_at_ms: 0,
            last_voice_ms: 0,
        }
    }

    /// Decide transmit on/off for this frame.
    ///
    /// * `mode`            — current talk mode.
    /// * `ptt_held`        — is the push-to-talk control held? (ignored unless PushToTalk)
    /// * `vad_likelihood`  — latest speech likelihood in [0,1] (ignored unless VoiceActivated)
    /// * `now_ms`          — monotonic clock in milliseconds.
    pub fn update(&mut self, mode: TalkMode, ptt_held: bool, vad_likelihood: f32, now_ms: u64) -> bool {
        let prev = self.transmitting;
        let next = match mode {
            TalkMode::Off => false,
            TalkMode::Continuous => true,
            TalkMode::PushToTalk => ptt_held,
            TalkMode::VoiceActivated => self.vox(prev, vad_likelihood, now_ms),
        };
        if next && !prev {
            // rising edge — start the open/hang timers
            self.opened_at_ms = now_ms;
            self.last_voice_ms = now_ms;
        }
        self.transmitting = next;
        next
    }

    /// VOX sub-decision with hysteresis + hang + min-open.
    fn vox(&mut self, currently_open: bool, vad: f32, now_ms: u64) -> bool {
        if !currently_open {
            // Closed: only open on a clear rising crossing of the OPEN threshold.
            vad >= self.cfg.open_threshold
        } else {
            // Open: refresh the voice timer whenever we're above CLOSE.
            if vad >= self.cfg.close_threshold {
                self.last_voice_ms = now_ms;
            }
            let min_open_satisfied =
                now_ms.saturating_sub(self.opened_at_ms) >= self.cfg.min_open_ms;
            let hang_expired =
                now_ms.saturating_sub(self.last_voice_ms) >= self.cfg.hang_ms;
            // Stay open unless BOTH the minimum-open floor and the hang tail have
            // elapsed. (min-open guards a single syllable; hang guards pauses.)
            !(min_open_satisfied && hang_expired)
        }
    }

    pub fn is_transmitting(&self) -> bool {
        self.transmitting
    }

    /// Force the gate closed (e.g. the channel was torn down, or floor control
    /// revoked our turn). Resets the timers so the next open starts clean.
    pub fn reset(&mut self) {
        self.transmitting = false;
        self.opened_at_ms = 0;
        self.last_voice_ms = 0;
    }

    pub fn config(&self) -> &SquelchConfig {
        &self.cfg
    }
}

// ─── C FFI (opaque handle, mirrors the VAD detector pattern) ─────────────────

/// Create a transmit gate. Pass squelch params (or any ≤0 to use defaults for
/// that field is NOT supported — pass the four explicitly). Free with
/// `sc_radio_gate_free`.
///
/// # Safety
/// The returned pointer must be freed exactly once via `sc_radio_gate_free`.
#[no_mangle]
pub extern "C" fn sc_radio_gate_new(
    open_threshold: f32,
    close_threshold: f32,
    hang_ms: u32,
    min_open_ms: u32,
) -> *mut TransmitGate {
    let cfg = SquelchConfig {
        open_threshold,
        close_threshold,
        hang_ms: hang_ms as u64,
        min_open_ms: min_open_ms as u64,
    };
    Box::into_raw(Box::new(TransmitGate::new(cfg)))
}

/// Create a transmit gate with the default squelch tuning.
///
/// # Safety
/// The returned pointer must be freed exactly once via `sc_radio_gate_free`.
#[no_mangle]
pub extern "C" fn sc_radio_gate_new_default() -> *mut TransmitGate {
    Box::into_raw(Box::new(TransmitGate::new(SquelchConfig::default())))
}

/// Advance the gate one frame; returns 1 to transmit, 0 to stay silent.
/// `mode` is a `TalkMode` value (0=Off,1=PushToTalk,2=VoiceActivated,3=Continuous).
/// `ptt_held` is 0/1. Returns 0 on a null handle.
///
/// # Safety
/// `handle` must be a live pointer from `sc_radio_gate_new[_default]`.
#[no_mangle]
pub unsafe extern "C" fn sc_radio_gate_update(
    handle: *mut TransmitGate,
    mode: c_int,
    ptt_held: c_int,
    vad_likelihood: f32,
    now_ms: u64,
) -> c_int {
    if handle.is_null() {
        return 0;
    }
    let gate = &mut *handle;
    let on = gate.update(TalkMode::from_raw(mode), ptt_held != 0, vad_likelihood, now_ms);
    if on {
        1
    } else {
        0
    }
}

/// Force the gate closed.
///
/// # Safety
/// `handle` must be a live pointer from `sc_radio_gate_new[_default]`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_radio_gate_reset(handle: *mut TransmitGate) {
    if !handle.is_null() {
        (&mut *handle).reset();
    }
}

/// Free a transmit gate.
///
/// # Safety
/// `handle` must be a pointer from `sc_radio_gate_new[_default]` not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_radio_gate_free(handle: *mut TransmitGate) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_never_transmits() {
        let mut g = TransmitGate::new(SquelchConfig::default());
        assert!(!g.update(TalkMode::Off, true, 1.0, 0));
        assert!(!g.update(TalkMode::Off, true, 1.0, 1000));
    }

    #[test]
    fn continuous_always_transmits() {
        let mut g = TransmitGate::new(SquelchConfig::default());
        assert!(g.update(TalkMode::Continuous, false, 0.0, 0));
        assert!(g.update(TalkMode::Continuous, false, 0.0, 1000));
    }

    #[test]
    fn ptt_follows_button() {
        let mut g = TransmitGate::new(SquelchConfig::default());
        assert!(!g.update(TalkMode::PushToTalk, false, 0.9, 0));
        assert!(g.update(TalkMode::PushToTalk, true, 0.0, 20));
        assert!(g.update(TalkMode::PushToTalk, true, 0.0, 40));
        assert!(!g.update(TalkMode::PushToTalk, false, 0.9, 60));
    }

    #[test]
    fn vox_opens_above_open_threshold() {
        let mut g = TransmitGate::new(SquelchConfig::default()); // open 0.6
        assert!(!g.update(TalkMode::VoiceActivated, false, 0.50, 0)); // below open
        assert!(g.update(TalkMode::VoiceActivated, false, 0.70, 20)); // crosses open
    }

    #[test]
    fn vox_hysteresis_mid_band_does_not_open() {
        // A score between close (0.4) and open (0.6) must NOT open a closed gate.
        let mut g = TransmitGate::new(SquelchConfig::default());
        assert!(!g.update(TalkMode::VoiceActivated, false, 0.50, 0));
        assert!(!g.update(TalkMode::VoiceActivated, false, 0.55, 20));
    }

    #[test]
    fn vox_hang_keeps_open_through_short_pause() {
        let cfg = SquelchConfig { open_threshold: 0.6, close_threshold: 0.4, hang_ms: 800, min_open_ms: 100 };
        let mut g = TransmitGate::new(cfg);
        assert!(g.update(TalkMode::VoiceActivated, false, 0.9, 0));     // open
        // go quiet, but within hang → still open
        assert!(g.update(TalkMode::VoiceActivated, false, 0.1, 300));
        assert!(g.update(TalkMode::VoiceActivated, false, 0.1, 700));
        // past min_open AND past hang since last voice (last voice was t=0) → close
        assert!(!g.update(TalkMode::VoiceActivated, false, 0.1, 900));
    }

    #[test]
    fn vox_voice_during_hang_refreshes_and_stays_open() {
        let cfg = SquelchConfig { open_threshold: 0.6, close_threshold: 0.4, hang_ms: 500, min_open_ms: 100 };
        let mut g = TransmitGate::new(cfg);
        assert!(g.update(TalkMode::VoiceActivated, false, 0.9, 0));
        assert!(g.update(TalkMode::VoiceActivated, false, 0.1, 300));  // quiet, within hang
        assert!(g.update(TalkMode::VoiceActivated, false, 0.9, 450));  // speaks again → refresh
        assert!(g.update(TalkMode::VoiceActivated, false, 0.1, 800));  // 350ms since refresh < 500 hang
        assert!(!g.update(TalkMode::VoiceActivated, false, 0.1, 1000)); // 550ms since refresh → close
    }

    #[test]
    fn vox_min_open_prevents_instant_close() {
        // A single loud frame then silence: min_open keeps it open briefly even
        // though hang would otherwise be irrelevant (hang small here).
        let cfg = SquelchConfig { open_threshold: 0.6, close_threshold: 0.4, hang_ms: 0, min_open_ms: 200 };
        let mut g = TransmitGate::new(cfg);
        assert!(g.update(TalkMode::VoiceActivated, false, 0.9, 0));   // open
        assert!(g.update(TalkMode::VoiceActivated, false, 0.0, 100)); // 100ms < min_open → stay open
        assert!(!g.update(TalkMode::VoiceActivated, false, 0.0, 250)); // past min_open + hang → close
    }

    #[test]
    fn degenerate_band_is_sanitized() {
        // close >= open would chatter; constructor must fix it.
        let cfg = SquelchConfig { open_threshold: 0.5, close_threshold: 0.9, hang_ms: 100, min_open_ms: 50 };
        let g = TransmitGate::new(cfg);
        assert!(g.config().open_threshold > g.config().close_threshold);
    }

    #[test]
    fn reset_closes_and_clears() {
        let mut g = TransmitGate::new(SquelchConfig::default());
        assert!(g.update(TalkMode::Continuous, false, 0.0, 0));
        g.reset();
        assert!(!g.is_transmitting());
    }

    #[test]
    fn ffi_roundtrip() {
        unsafe {
            let h = sc_radio_gate_new_default();
            assert!(!h.is_null());
            // PushToTalk held → 1
            assert_eq!(sc_radio_gate_update(h, TalkMode::PushToTalk as c_int, 1, 0.0, 0), 1);
            assert_eq!(sc_radio_gate_update(h, TalkMode::PushToTalk as c_int, 0, 0.0, 20), 0);
            // null handle → 0, no crash
            assert_eq!(sc_radio_gate_update(std::ptr::null_mut(), 3, 1, 1.0, 0), 0);
            sc_radio_gate_reset(h);
            sc_radio_gate_free(h);
        }
    }

    #[test]
    fn from_raw_unknown_is_off() {
        assert_eq!(TalkMode::from_raw(99), TalkMode::Off);
        assert_eq!(TalkMode::from_raw(-1), TalkMode::Off);
        assert_eq!(TalkMode::from_raw(2), TalkMode::VoiceActivated);
    }
}
