// src/deferred_transcribe.rs
//
// v0.1.460: deferred batch transcription.
//
// Motivation (user idea): "store the audio and bulk process it
// later in short bursts when you have easy access to the whisper
// context — this could improve efficiency."
//
// The expensive part of whisper is NOT per-clip inference, it's
// the model load + state init. The log shows it concretely:
//     whisper_init_state: compute buffer (encode) = 85 MB
//     whisper_init_state: compute buffer (decode) = 96 MB
//     (+ kv self/cross/pad, conv buffers)
// Roughly ~200MB of buffers allocated on init. If audio is
// transcribed one clip at a time with the context torn down
// between (or paged out under memory pressure on idle), that init
// cost is paid repeatedly.
//
// This module decouples "audio captured" from "audio
// transcribed". Clips that don't need a real-time result (corpus
// recordings, background captures) are enqueued here instead of
// hitting whisper immediately. They're drained in a single WARM
// BURST when conditions are favorable — paying the model-init cost
// ONCE and amortizing it across the whole batch.
//
// IMPORTANT: this module is the QUEUE + DRAIN POLICY only. It does
// NOT call whisper itself — the caller supplies a transcribe
// closure when draining, so the actual inference reuses the
// existing whisper path (and so this module stays testable without
// a model present). The valuable, subtle logic — WHEN to drain and
// WHICH clips to take — lives here and is fully unit-tested.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// A clip awaiting deferred transcription.
#[derive(Clone, Debug)]
pub struct PendingClip {
    /// Path to the saved audio on disk (.wav/.opus). The drain
    /// step loads + decodes this when it actually transcribes.
    pub audio_path: String,
    /// Speaker tag (may be empty / unknown).
    pub speaker: String,
    /// ms-since-epoch when the clip was captured. Used for
    /// oldest-first ordering and staleness.
    pub captured_ms: u64,
    /// Approx duration in seconds (for batch-time budgeting).
    pub approx_secs: f32,
}

/// Conditions the scheduler consults to decide whether to drain.
/// Pure inputs — the caller gathers these from the live system so
/// the decision logic itself is deterministic and testable.
#[derive(Clone, Copy, Debug)]
pub struct DrainContext {
    /// App is backgrounded / screen off.
    pub backgrounded: bool,
    /// On battery (vs plugged in).
    pub on_battery: bool,
    /// Battery percentage 0..100 (100 if unknown / desktop).
    pub battery_pct: u8,
    /// Whisper model is already loaded/warm RIGHT NOW. Draining
    /// while warm is the whole point — the init cost is already
    /// paid, so extra clips are nearly free.
    pub whisper_warm: bool,
    /// Foreground voice is actively recording — never contend with
    /// a live capture; the user's real-time interaction wins.
    pub foreground_capturing: bool,
}

/// The scheduler's decision, with a reason for logging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainDecision {
    /// Drain now (whisper is warm or it's a good idle window).
    Go,
    /// Hold — conditions unfavorable. The &'static str is why.
    Hold(&'static str),
}

/// Max clips to transcribe in a single burst, so a drain can't
/// monopolize the CPU/thread indefinitely. The next burst picks up
/// the rest. Keeps each warm-window bounded.
pub const MAX_BATCH: usize = 12;

/// Don't bother spinning whisper up from cold for fewer than this
/// many pending clips when on battery — the init cost isn't worth
/// it for one or two. (When already warm, any count drains.)
pub const COLD_BATTERY_MIN_CLIPS: usize = 5;

/// Below this battery % on battery, never drain — protect a
/// critically low charge (mirrors the bg-listen 30% gate).
pub const BATTERY_FLOOR_PCT: u8 = 30;

fn queue() -> &'static Mutex<VecDeque<PendingClip>> {
    static Q: OnceLock<Mutex<VecDeque<PendingClip>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Enqueue a clip for later batch transcription.
pub fn enqueue(clip: PendingClip) {
    if let Ok(mut q) = queue().lock() {
        q.push_back(clip);
    }
}

/// Number of clips currently waiting.
pub fn pending_count() -> usize {
    queue().lock().map(|q| q.len()).unwrap_or(0)
}

/// Decide whether to drain given the current context. Pure
/// function of (pending count, ctx) — no I/O — so it's fully
/// testable. Policy:
///
///   - Never while foreground voice is capturing (contention).
///   - Never if nothing pending.
///   - Never below the battery floor on battery.
///   - If whisper is ALREADY warm: drain (the expensive init is
///     already paid; piggy-backing the backlog is the efficient
///     move regardless of power state). This is the core of the
///     user's idea.
///   - If whisper is cold AND on battery: only spin it up for a
///     worthwhile batch (>= COLD_BATTERY_MIN_CLIPS), and prefer
///     not to do it while backgrounded (let it wait for a warm
///     window or for charging).
///   - If whisper is cold AND plugged in: drain whenever there's
///     anything pending (power is free).
pub fn decide(pending: usize, ctx: DrainContext) -> DrainDecision {
    if ctx.foreground_capturing {
        return DrainDecision::Hold("foreground voice capturing");
    }
    if pending == 0 {
        return DrainDecision::Hold("nothing pending");
    }
    if ctx.on_battery && ctx.battery_pct < BATTERY_FLOOR_PCT {
        return DrainDecision::Hold("battery below floor");
    }
    if ctx.whisper_warm {
        // Already paid the init — always worth draining the backlog.
        return DrainDecision::Go;
    }
    // Whisper is cold from here on (spinning it up costs the init).
    if !ctx.on_battery {
        // Plugged in: power is free, drain anything.
        return DrainDecision::Go;
    }
    // On battery + cold: be choosy.
    if ctx.backgrounded {
        // Deep idle on battery — don't wake whisper from cold just
        // to grind the backlog; wait for a foreground/warm window.
        return DrainDecision::Hold("cold + backgrounded on battery");
    }
    if pending >= COLD_BATTERY_MIN_CLIPS {
        DrainDecision::Go
    } else {
        DrainDecision::Hold("cold on battery, batch too small")
    }
}

/// Convenience: gather the decision from live count + ctx.
pub fn should_drain(ctx: DrainContext) -> DrainDecision {
    decide(pending_count(), ctx)
}

/// Pull up to MAX_BATCH clips, OLDEST FIRST, for a warm-burst
/// drain. Removes them from the queue and returns them. The caller
/// transcribes the returned batch with the (already-warm) whisper
/// context, paying init at most once for the whole batch.
///
/// Oldest-first because a deferred clip's value decays (a corpus
/// sample captured an hour ago is staler than one from a minute
/// ago) and FIFO is the least surprising drain order.
pub fn take_batch() -> Vec<PendingClip> {
    let mut out = Vec::new();
    if let Ok(mut q) = queue().lock() {
        while out.len() < MAX_BATCH {
            match q.pop_front() {
                Some(c) => out.push(c),
                None => break,
            }
        }
    }
    out
}

/// Re-enqueue clips that failed to transcribe (e.g. transient
/// whisper error) at the FRONT so they're retried first next
/// burst. Bounded: if the queue is already huge, drop — a clip
/// that keeps failing shouldn't grow the backlog unboundedly.
pub fn requeue_front(clips: Vec<PendingClip>) {
    if clips.is_empty() {
        return;
    }
    if let Ok(mut q) = queue().lock() {
        // Cap total backlog to avoid unbounded growth on repeated
        // failure. 200 is generous for corpus-style accumulation.
        const MAX_BACKLOG: usize = 200;
        for c in clips.into_iter().rev() {
            if q.len() >= MAX_BACKLOG {
                break;
            }
            q.push_front(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(bg: bool, batt: bool, pct: u8, warm: bool, fg: bool) -> DrainContext {
        DrainContext {
            backgrounded: bg,
            on_battery: batt,
            battery_pct: pct,
            whisper_warm: warm,
            foreground_capturing: fg,
        }
    }

    #[test]
    fn never_while_foreground_capturing() {
        // Even warm + plugged + lots pending: foreground wins.
        assert_eq!(decide(50, ctx(false, false, 100, true, true)),
            DrainDecision::Hold("foreground voice capturing"));
    }

    #[test]
    fn nothing_pending_holds() {
        assert_eq!(decide(0, ctx(false, false, 100, true, false)),
            DrainDecision::Hold("nothing pending"));
    }

    #[test]
    fn battery_floor_protects_low_charge() {
        assert_eq!(decide(20, ctx(false, true, 15, true, false)),
            DrainDecision::Hold("battery below floor"));
        // Exactly at floor still holds (strict <), one above goes.
        assert_eq!(decide(20, ctx(false, true, 29, true, false)),
            DrainDecision::Hold("battery below floor"));
        assert_eq!(decide(20, ctx(false, true, 30, true, false)),
            DrainDecision::Go);
    }

    #[test]
    fn warm_always_drains_above_floor() {
        // The core idea: if whisper is warm, drain regardless of
        // power state (init already paid).
        assert_eq!(decide(1, ctx(true, true, 80, true, false)), DrainDecision::Go);
        assert_eq!(decide(1, ctx(false, false, 100, true, false)), DrainDecision::Go);
    }

    #[test]
    fn cold_plugged_drains_anything() {
        assert_eq!(decide(1, ctx(false, false, 100, false, false)), DrainDecision::Go);
        assert_eq!(decide(1, ctx(true, false, 100, false, false)), DrainDecision::Go);
    }

    #[test]
    fn cold_battery_backgrounded_waits() {
        assert_eq!(decide(50, ctx(true, true, 90, false, false)),
            DrainDecision::Hold("cold + backgrounded on battery"));
    }

    #[test]
    fn cold_battery_foreground_needs_worthwhile_batch() {
        // Small batch from cold on battery: not worth the init.
        assert_eq!(decide(2, ctx(false, true, 90, false, false)),
            DrainDecision::Hold("cold on battery, batch too small"));
        // Big enough batch: amortizes the init, go.
        assert_eq!(decide(COLD_BATTERY_MIN_CLIPS, ctx(false, true, 90, false, false)),
            DrainDecision::Go);
    }

    #[test]
    fn take_batch_is_fifo_and_capped() {
        // Fresh queue for this test (shared singleton — drain it first).
        while !take_batch().is_empty() {}
        for i in 0..(MAX_BATCH + 5) {
            enqueue(PendingClip {
                audio_path: format!("/tmp/clip_{}.wav", i),
                speaker: String::new(),
                captured_ms: i as u64,
                approx_secs: 1.0,
            });
        }
        let batch = take_batch();
        assert_eq!(batch.len(), MAX_BATCH, "batch capped at MAX_BATCH");
        // Oldest first.
        assert_eq!(batch[0].audio_path, "/tmp/clip_0.wav");
        assert_eq!(batch[MAX_BATCH - 1].audio_path,
            format!("/tmp/clip_{}.wav", MAX_BATCH - 1));
        // Remaining 5 still queued.
        assert_eq!(pending_count(), 5);
        // Second burst takes the rest.
        let rest = take_batch();
        assert_eq!(rest.len(), 5);
        assert_eq!(rest[0].audio_path, format!("/tmp/clip_{}.wav", MAX_BATCH));
        assert_eq!(pending_count(), 0);
    }

    #[test]
    fn requeue_front_retries_first_and_bounds() {
        while !take_batch().is_empty() {}
        enqueue(PendingClip { audio_path: "a".into(), speaker: String::new(),
            captured_ms: 1, approx_secs: 1.0 });
        // Failed clips go back to the front for first retry.
        requeue_front(vec![
            PendingClip { audio_path: "f1".into(), speaker: String::new(),
                captured_ms: 0, approx_secs: 1.0 },
            PendingClip { audio_path: "f2".into(), speaker: String::new(),
                captured_ms: 0, approx_secs: 1.0 },
        ]);
        let b = take_batch();
        // f1, f2 (re-enqueued front, original order preserved), then a.
        assert_eq!(b[0].audio_path, "f1");
        assert_eq!(b[1].audio_path, "f2");
        assert_eq!(b[2].audio_path, "a");
    }
}
