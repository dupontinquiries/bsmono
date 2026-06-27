// src/queue_policy.rs
//
// The playback-queue policy layer: a small, pure, fully-testable
// abstraction over "what should happen when the user adds a track,
// given how they want it added and what's currently playing."
//
// Historically `enqueue_or_play` hard-coded a single behaviour (if
// something is playing, append; else play now). That conflated *intent*
// with *mechanism* and left no room for the richer behaviours the UI
// wants. This module makes the intent a first-class value — `EnqueueMode`
// — and turns the (mode, current-state) pair into an explicit ordered
// list of `QueueAction`s the caller then executes against the real
// queue / player. No Qt, no I/O, no globals: just data in, plan out, so
// every mode is unit-testable in isolation.
//
// The four modes mirror the ways a listener actually wants to add audio:
//
//   * Stack       — append after everything already queued. The classic
//                   "add to queue"; current playback is untouched.
//
//   * Replace     — clear the queue and play this now. "Forget all that,
//                   play this instead."
//
//   * Synchronize — start this track *together with* the current one,
//                   time-aligned from t=0, as an overlaid layer (think
//                   two stems meant to be heard as one). The current
//                   track keeps playing; the new one is added as a
//                   synchronized layer rather than queued behind.
//
//   * MidgapLayer — drop this in *underneath* the current track on a
//                   separate layer, at the current playhead position
//                   (fill the "mid gap"), without interrupting the top
//                   track. A bed/ambience under what's already playing.
//
// Stack and Replace operate on the linear queue. Synchronize and
// MidgapLayer operate on the *layer* system (the same multi-layer mixer
// the soundscape engine uses) — they don't touch the linear queue order,
// they add a concurrent layer. Representing all four as one plan type
// keeps the call site uniform.

/// How the user wants an added track to behave relative to current
/// playback. The default is `Stack` (the least-surprising "add to queue").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueMode {
    Stack,
    Replace,
    Synchronize,
    MidgapLayer,
}

impl Default for EnqueueMode {
    fn default() -> Self { EnqueueMode::Stack }
}

impl EnqueueMode {
    /// Stable string tag for persistence / the Qt bridge. Kept in sync
    /// with `from_tag`.
    pub fn as_tag(self) -> &'static str {
        match self {
            EnqueueMode::Stack       => "stack",
            EnqueueMode::Replace     => "replace",
            EnqueueMode::Synchronize => "sync",
            EnqueueMode::MidgapLayer => "midgap",
        }
    }

    /// Parse a tag back to a mode. Unknown / empty tags fall back to the
    /// default (`Stack`) rather than erroring — a bad persisted value
    /// should degrade to the safe behaviour, not break enqueue.
    pub fn from_tag(s: &str) -> Self {
        match s {
            "replace" => EnqueueMode::Replace,
            "sync"    => EnqueueMode::Synchronize,
            "midgap"  => EnqueueMode::MidgapLayer,
            _          => EnqueueMode::Stack,
        }
    }

    /// Human-facing label for menus / toasts.
    pub fn label(self) -> &'static str {
        match self {
            EnqueueMode::Stack       => "Add to queue",
            EnqueueMode::Replace     => "Play now (replace queue)",
            EnqueueMode::Synchronize => "Play synced (overlay)",
            EnqueueMode::MidgapLayer => "Play underneath (layer)",
        }
    }

    /// Does this mode use the concurrent-layer system rather than the
    /// linear queue? (Synchronize / MidgapLayer.) Useful for the UI to
    /// decide whether to show layer controls.
    pub fn is_layered(self) -> bool {
        matches!(self, EnqueueMode::Synchronize | EnqueueMode::MidgapLayer)
    }
}

/// A single concrete step the caller executes. The policy never performs
/// these; it only decides the sequence. Keeping them coarse (one action
/// per real operation the controller already supports) means the call
/// site is a thin `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    /// Clear the entire linear queue for the active device.
    ClearQueue,
    /// Append the track to the end of the linear queue.
    PushBack(i64),
    /// Insert the track at a specific queue position (0 = play next).
    InsertAt(i64, i64),
    /// Begin playback of the track immediately on the active device,
    /// replacing whatever the *top* layer is currently playing.
    PlayNow(i64),
    /// Add the track as a concurrent layer that starts time-aligned from
    /// the beginning (synchronized overlay). bool = start_paused: if the
    /// top track is itself paused we mirror that so they stay in lockstep.
    AddSyncLayer(i64, bool),
    /// Add the track as a concurrent layer positioned *underneath* the
    /// current top track, entering at the current playhead (mid-gap bed).
    /// The i64 is the start offset in milliseconds to align the bed with
    /// the top track's current position (0 if unknown / not playing).
    AddUnderlayLayer(i64, i64),
}

/// Everything the policy needs to know about the current player state to
/// produce a plan. Deliberately tiny + Copy so it's trivial to build at
/// the call site and to construct in tests.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackState {
    /// Is the top layer actively playing right now?
    pub is_playing: bool,
    /// Is the top layer paused (loaded but not advancing)?
    pub is_paused: bool,
    /// The top track's current playhead in milliseconds (for aligning a
    /// mid-gap underlay). 0 if unknown.
    pub position_ms: i64,
}

impl PlaybackState {
    /// Nothing is loaded / playing — a clean slate.
    pub fn idle() -> Self {
        Self { is_playing: false, is_paused: false, position_ms: 0 }
    }
    /// True iff there is *something* on the top layer (playing or paused)
    /// that a new track could relate to.
    pub fn has_top(self) -> bool { self.is_playing || self.is_paused }
}

/// Turn (mode, track, current state) into the ordered list of actions to
/// execute. This is the heart of the module — every behavioural decision
/// lives here and nowhere else.
///
/// Invariants the plans uphold:
///   * `Stack` never interrupts current playback.
///   * `Replace` always ends with the new track playing and an otherwise
///     empty queue.
///   * The layered modes (`Synchronize`, `MidgapLayer`) never touch the
///     linear queue and never stop the top track.
///   * When nothing is playing, *every* mode degrades to "just play it"
///     — there's no current track to stack behind, sync with, or layer
///     under, so the only sensible result is immediate playback. This
///     keeps a first tap on a fresh device behaving intuitively no matter
///     which mode is selected.
pub fn plan(mode: EnqueueMode, track_id: i64, state: PlaybackState) -> Vec<QueueAction> {
    // Universal idle fallback: with no top track, all modes collapse to
    // PlayNow. (Replace also implies an empty queue, but on a fresh device
    // the queue is already empty; we still clear for determinism.)
    if !state.has_top() {
        return match mode {
            EnqueueMode::Replace => vec![QueueAction::ClearQueue, QueueAction::PlayNow(track_id)],
            _                    => vec![QueueAction::PlayNow(track_id)],
        };
    }

    match mode {
        EnqueueMode::Stack => {
            // Something is playing/paused → just append; don't disturb it.
            vec![QueueAction::PushBack(track_id)]
        }
        EnqueueMode::Replace => {
            // Wipe the queue and take over the top layer immediately.
            vec![QueueAction::ClearQueue, QueueAction::PlayNow(track_id)]
        }
        EnqueueMode::Synchronize => {
            // Overlay, time-aligned from 0. Mirror the top layer's
            // paused-ness so the two stay locked together.
            vec![QueueAction::AddSyncLayer(track_id, state.is_paused)]
        }
        EnqueueMode::MidgapLayer => {
            // Bed underneath, entering at the current playhead so it lines
            // up with what's already audible.
            vec![QueueAction::AddUnderlayLayer(track_id, state.position_ms.max(0))]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_round_trips_all_modes() {
        for m in [EnqueueMode::Stack, EnqueueMode::Replace,
                  EnqueueMode::Synchronize, EnqueueMode::MidgapLayer] {
            assert_eq!(EnqueueMode::from_tag(m.as_tag()), m);
        }
    }

    #[test]
    fn unknown_tag_falls_back_to_stack() {
        assert_eq!(EnqueueMode::from_tag(""), EnqueueMode::Stack);
        assert_eq!(EnqueueMode::from_tag("garbage"), EnqueueMode::Stack);
        assert_eq!(EnqueueMode::default(), EnqueueMode::Stack);
    }

    #[test]
    fn layered_flag_only_for_sync_and_midgap() {
        assert!(!EnqueueMode::Stack.is_layered());
        assert!(!EnqueueMode::Replace.is_layered());
        assert!(EnqueueMode::Synchronize.is_layered());
        assert!(EnqueueMode::MidgapLayer.is_layered());
    }

    #[test]
    fn idle_makes_every_mode_play_now() {
        let idle = PlaybackState::idle();
        for m in [EnqueueMode::Stack, EnqueueMode::Synchronize,
                  EnqueueMode::MidgapLayer] {
            assert_eq!(plan(m, 7, idle), vec![QueueAction::PlayNow(7)],
                "{:?} on idle should just play", m);
        }
        // Replace on idle clears first (determinism) then plays.
        assert_eq!(plan(EnqueueMode::Replace, 7, idle),
                   vec![QueueAction::ClearQueue, QueueAction::PlayNow(7)]);
    }

    #[test]
    fn stack_appends_without_interrupting() {
        let playing = PlaybackState { is_playing: true, is_paused: false, position_ms: 5000 };
        assert_eq!(plan(EnqueueMode::Stack, 42, playing),
                   vec![QueueAction::PushBack(42)]);
    }

    #[test]
    fn replace_clears_then_plays() {
        let playing = PlaybackState { is_playing: true, is_paused: false, position_ms: 5000 };
        assert_eq!(plan(EnqueueMode::Replace, 42, playing),
                   vec![QueueAction::ClearQueue, QueueAction::PlayNow(42)]);
    }

    #[test]
    fn sync_mirrors_paused_state() {
        let playing = PlaybackState { is_playing: true, is_paused: false, position_ms: 0 };
        assert_eq!(plan(EnqueueMode::Synchronize, 9, playing),
                   vec![QueueAction::AddSyncLayer(9, false)]);
        let paused = PlaybackState { is_playing: false, is_paused: true, position_ms: 0 };
        assert_eq!(plan(EnqueueMode::Synchronize, 9, paused),
                   vec![QueueAction::AddSyncLayer(9, true)]);
    }

    #[test]
    fn midgap_aligns_to_playhead() {
        let playing = PlaybackState { is_playing: true, is_paused: false, position_ms: 12345 };
        assert_eq!(plan(EnqueueMode::MidgapLayer, 9, playing),
                   vec![QueueAction::AddUnderlayLayer(9, 12345)]);
        // Negative/garbage position clamps to 0.
        let weird = PlaybackState { is_playing: true, is_paused: false, position_ms: -50 };
        assert_eq!(plan(EnqueueMode::MidgapLayer, 9, weird),
                   vec![QueueAction::AddUnderlayLayer(9, 0)]);
    }

    #[test]
    fn layered_modes_never_clear_or_push_queue() {
        let playing = PlaybackState { is_playing: true, is_paused: false, position_ms: 100 };
        for m in [EnqueueMode::Synchronize, EnqueueMode::MidgapLayer] {
            let actions = plan(m, 1, playing);
            assert!(!actions.iter().any(|a| matches!(a,
                QueueAction::ClearQueue | QueueAction::PushBack(_) | QueueAction::InsertAt(_, _))),
                "{:?} must not touch the linear queue", m);
            assert!(!actions.iter().any(|a| matches!(a, QueueAction::PlayNow(_))),
                "{:?} must not stop the top track", m);
        }
    }
}
