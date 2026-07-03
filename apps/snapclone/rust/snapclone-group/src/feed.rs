//! feed.rs — the playback scheduler for the group "mini autoscrolling feed".
//!
//! # The UX this serves
//! Group video snaps POP UP AND PLAY IN ORDER on the main page with a short
//! delay between them — a mini auto-scrolling feed that feels close to a live
//! call, but trades the brittle held connection of RTC for robustness, fidelity,
//! and reachability. Each clip is a complete, reliably-delivered snap; this
//! scheduler decides WHAT plays NEXT and WHEN, so a burst of arrivals reads as a
//! conversation instead of a pile of notifications.
//!
//! # What this module is (and isn't)
//! This is the *brain*: pure scheduling logic, no Qt, no timers, no I/O. The Qt
//! feed widget is a thin renderer that (a) feeds clips in as they arrive, (b)
//! ticks this scheduler with the current time, and (c) plays whatever it says to
//! play. Keeping the policy here makes the tricky parts — ordering, the
//! inter-clip gap, late arrivals, seen-tracking, pause/resume — testable.
//!
//! # Ordering is the group DAG's job, not ours
//! Clips are scheduled in the group's CAUSAL TOTAL ORDER (`dag::total_order`,
//! sorted by `(lamport, author, id)`), NOT by wall-clock arrival. The caller
//! passes each clip's `order_key` (its index/rank in that causal order). This
//! scheduler only sequences and times; it trusts the DAG for "what comes before
//! what", so every replica plays the same sequence.

use std::collections::BTreeMap;

/// A clip queued for the feed. `order_key` is its rank in the group's causal
/// total order (lower = earlier). `clip_id` is the stable media/snap id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedClip {
    /// Rank in the group causal order (from dag::total_order position). The
    /// scheduler plays in ascending order_key. Ties shouldn't happen (the DAG's
    /// order is total) but if they do, clip_id breaks them deterministically.
    pub order_key: u64,
    /// Stable clip/snap id (what the renderer loads + plays).
    pub clip_id: String,
    /// Approx playback duration in ms (so the scheduler knows when the clip
    /// finishes and the inter-clip gap should start). 0 = unknown; the caller
    /// must then signal completion explicitly via `mark_finished`.
    pub duration_ms: u64,
}

/// What the feed should be doing right now, returned by `tick`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedState {
    /// Nothing to play (queue empty or all seen). The page can show idle/ambient.
    Idle,
    /// Play this clip now (it just became the active clip). Emitted once per
    /// clip when it starts.
    Play(String),
    /// A clip is currently playing; nothing to change. Carries the active id.
    Playing(String),
    /// In the inter-clip gap (the ~1s delay) before the next clip. Carries ms
    /// remaining so the UI can show a subtle countdown/transition if it wants.
    Gap { next_clip: String, ms_remaining: u64 },
    /// Paused (user interaction). Resumes via `resume`.
    Paused,
}

/// The feed scheduler. Time is injected (`now_ms`) — no internal clock — so it's
/// deterministic and testable. The caller ticks it; it never blocks.
#[derive(Debug, Clone)]
pub struct FeedScheduler {
    /// Inter-clip gap in ms (the "~1 second delay" that makes it read as a feed).
    gap_ms: u64,
    /// Unplayed clips keyed by (order_key, clip_id) so iteration is causal order.
    queue: BTreeMap<(u64, String), FeedClip>,
    /// Clips already played (seen), by clip_id — so re-adds don't replay and
    /// catch-up knows what to skip.
    seen: BTreeMap<String, ()>,
    /// The currently-playing clip + when it started (ms). None = not playing.
    active: Option<(String, u64)>,
    /// Active clip's duration (cached so finish-time = started + duration).
    active_duration_ms: u64,
    /// When the current inter-clip gap started (ms). None = not in a gap.
    gap_started_ms: Option<u64>,
    /// Paused flag (user interaction). While paused, tick returns Paused and
    /// time doesn't advance the active clip / gap (the caller passes fresh
    /// now_ms on resume and we rebase).
    paused: bool,
    /// Whether the feed is the focused page. When false, tick returns Idle and
    /// nothing autoplays (mirrors the camera lifecycle: don't run off-screen).
    focused: bool,
}

impl FeedScheduler {
    pub fn new(gap_ms: u64) -> Self {
        Self {
            gap_ms,
            queue: BTreeMap::new(),
            seen: BTreeMap::new(),
            active: None,
            active_duration_ms: 0,
            gap_started_ms: None,
            paused: false,
            focused: true,
        }
    }

    /// Add (or update) a clip. Late arrivals are inserted in CAUSAL ORDER — if a
    /// clip with a lower order_key than the currently-queued ones arrives while
    /// we're mid-feed, it slots into its correct position and will play in order
    /// *if it hasn't been passed yet*. A clip whose id is already seen is
    /// ignored (no replay). Returns true if it was newly queued.
    pub fn add_clip(&mut self, clip: FeedClip) -> bool {
        if self.seen.contains_key(&clip.clip_id) {
            return false; // already played — don't resurrect
        }
        let key = (clip.order_key, clip.clip_id.clone());
        // If the same clip_id is already queued (possibly under a different
        // order_key after a re-derivation), remove the stale entry first.
        let existing: Vec<(u64, String)> = self
            .queue
            .keys()
            .filter(|(_, id)| id == &clip.clip_id)
            .cloned()
            .collect();
        for k in existing {
            self.queue.remove(&k);
        }
        self.queue.insert(key, clip);
        true
    }

    /// Mark the active clip finished (used when duration_ms was 0/unknown and
    /// the renderer signals end-of-playback itself). Starts the inter-clip gap.
    pub fn mark_finished(&mut self, now_ms: u64) {
        if let Some((id, _)) = self.active.take() {
            self.seen.insert(id, ());
            self.gap_started_ms = Some(now_ms);
        }
    }

    /// Pause (user scrolled/tapped). The active clip and gap freeze.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resume from pause. `now_ms` rebases timing so the remaining gap/clip
    /// continues from here rather than jumping (avoids skipping a clip because
    /// real time elapsed while paused).
    pub fn resume(&mut self, now_ms: u64) {
        if !self.paused {
            return;
        }
        self.paused = false;
        // Rebase: if a clip was active, treat it as (re)started now; if in a gap,
        // restart the gap from now. This keeps the "feel" — a paused feed picks
        // up where it left off, not where the wall clock ran to.
        if let Some((id, _)) = self.active.take() {
            self.active = Some((id, now_ms));
        } else if self.gap_started_ms.is_some() {
            self.gap_started_ms = Some(now_ms);
        }
    }

    /// Focus/unfocus the feed page. Unfocused → tick is Idle, no autoplay.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The core: given the current time, decide what the feed should do. Pure —
    /// call it on a timer tick or whenever clips/time change.
    pub fn tick(&mut self, now_ms: u64) -> FeedState {
        if !self.focused {
            return FeedState::Idle;
        }
        if self.paused {
            return FeedState::Paused;
        }

        // 1. A clip is active — has it finished (only when duration is known)?
        if let Some((id, started)) = self.active.clone() {
            if self.active_duration_ms == 0 {
                // Unknown duration — wait for mark_finished(); keep playing.
                return FeedState::Playing(id);
            }
            if now_ms.saturating_sub(started) < self.active_duration_ms {
                return FeedState::Playing(id);
            }
            // Finished: retire it and begin the gap.
            self.active = None;
            self.seen.insert(id, ());
            self.gap_started_ms = Some(now_ms);
        }

        // 2. In the inter-clip gap?
        if let Some(gap_start) = self.gap_started_ms {
            let elapsed = now_ms.saturating_sub(gap_start);
            if elapsed < self.gap_ms {
                // Still gapping — report time remaining + the upcoming clip.
                if let Some(next) = self.peek_next_id() {
                    return FeedState::Gap {
                        next_clip: next,
                        ms_remaining: self.gap_ms - elapsed,
                    };
                }
                // Gap but nothing queued behind it — fall through to Idle.
                self.gap_started_ms = None;
                return FeedState::Idle;
            }
            // Gap elapsed — clear it and start the next clip below.
            self.gap_started_ms = None;
        }

        // 3. Nothing active, no pending gap — start the next queued clip.
        if let Some((key, clip)) = self.pop_next() {
            let _ = key;
            self.active = Some((clip.clip_id.clone(), now_ms));
            self.active_duration_ms = clip.duration_ms;
            return FeedState::Play(clip.clip_id);
        }

        FeedState::Idle
    }

    /// Number of clips waiting to play (for a "N new" badge).
    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }

    /// Whether a given clip has been played.
    pub fn has_seen(&self, clip_id: &str) -> bool {
        self.seen.contains_key(clip_id)
    }

    /// The id of the next clip that would play, without dequeuing it.
    pub fn peek_next_id(&self) -> Option<String> {
        self.queue.keys().next().map(|(_, id)| id.clone())
    }

    fn pop_next(&mut self) -> Option<((u64, String), FeedClip)> {
        let key = self.queue.keys().next().cloned()?;
        let clip = self.queue.remove(&key)?;
        Some((key, clip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(order: u64, id: &str, dur: u64) -> FeedClip {
        FeedClip { order_key: order, clip_id: id.to_string(), duration_ms: dur }
    }

    #[test]
    fn plays_clips_in_causal_order_with_gap() {
        let mut s = FeedScheduler::new(1000); // 1s gap
        s.add_clip(clip(2, "b", 500));
        s.add_clip(clip(1, "a", 500));        // added 2nd but lower order_key
        // t=0: starts the causally-first clip (a), not the first-added.
        assert_eq!(s.tick(0), FeedState::Play("a".into()));
        // mid-clip
        assert_eq!(s.tick(250), FeedState::Playing("a".into()));
        // a finishes at 500 → gap begins, b is next
        match s.tick(500) {
            FeedState::Gap { next_clip, ms_remaining } => {
                assert_eq!(next_clip, "b");
                assert_eq!(ms_remaining, 1000);
            }
            other => panic!("expected Gap, got {:?}", other),
        }
        // still gapping at 500+999
        assert!(matches!(s.tick(1499), FeedState::Gap { .. }));
        // gap done at 500+1000 → b plays
        assert_eq!(s.tick(1500), FeedState::Play("b".into()));
        // b finishes → nothing left
        assert_eq!(s.tick(2000), FeedState::Idle);
        assert!(s.has_seen("a") && s.has_seen("b"));
    }

    #[test]
    fn late_arrival_inserts_in_order_if_not_passed() {
        let mut s = FeedScheduler::new(1000);
        s.add_clip(clip(1, "a", 500));
        s.add_clip(clip(3, "c", 500));
        assert_eq!(s.tick(0), FeedState::Play("a".into()));
        // While 'a' plays, a causally-earlier-than-c clip 'b' (order 2) arrives.
        assert!(s.add_clip(clip(2, "b", 500)));
        // a finishes → gap → next should be b (order 2), not c.
        let _ = s.tick(500);
        assert_eq!(s.peek_next_id(), Some("b".into()));
        let _ = s.tick(1500); // gap done → b plays
        // after b, c.
        let _ = s.tick(2000); // b finishes → gap
        assert_eq!(s.peek_next_id(), Some("c".into()));
    }

    #[test]
    fn seen_clip_is_not_replayed() {
        let mut s = FeedScheduler::new(0);
        s.add_clip(clip(1, "a", 100));
        assert_eq!(s.tick(0), FeedState::Play("a".into()));
        s.tick(100); // finishes → seen
        assert!(s.has_seen("a"));
        // Re-adding the same clip id is ignored.
        assert!(!s.add_clip(clip(1, "a", 100)));
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn unknown_duration_waits_for_mark_finished() {
        let mut s = FeedScheduler::new(500);
        s.add_clip(clip(1, "a", 0)); // unknown duration
        assert_eq!(s.tick(0), FeedState::Play("a".into()));
        // Without a known duration it keeps Playing regardless of time…
        assert_eq!(s.tick(99999), FeedState::Playing("a".into()));
        // …until the renderer signals completion.
        s.mark_finished(99999);
        assert!(s.has_seen("a"));
        // now in gap
        assert!(matches!(s.tick(99999), FeedState::Idle | FeedState::Gap { .. }));
    }

    #[test]
    fn pause_freezes_and_resume_rebases() {
        let mut s = FeedScheduler::new(1000);
        s.add_clip(clip(1, "a", 500));
        s.add_clip(clip(2, "b", 500));
        assert_eq!(s.tick(0), FeedState::Play("a".into()));
        s.pause();
        // While paused, time passing does NOT advance/skip.
        assert_eq!(s.tick(10_000), FeedState::Paused);
        s.resume(10_000);
        // 'a' rebased to start at 10_000 — still playing right after resume.
        assert_eq!(s.tick(10_100), FeedState::Playing("a".into()));
        // finishes 500ms after the rebased start, not from the original t=0.
        assert!(matches!(s.tick(10_500), FeedState::Gap { .. }));
    }

    #[test]
    fn unfocused_is_idle_no_autoplay() {
        let mut s = FeedScheduler::new(1000);
        s.add_clip(clip(1, "a", 500));
        s.set_focused(false);
        assert_eq!(s.tick(0), FeedState::Idle);
        // nothing consumed — still queued for when we return.
        assert_eq!(s.pending_count(), 1);
        s.set_focused(true);
        assert_eq!(s.tick(0), FeedState::Play("a".into()));
    }

    #[test]
    fn empty_queue_is_idle() {
        let mut s = FeedScheduler::new(1000);
        assert_eq!(s.tick(0), FeedState::Idle);
        assert_eq!(s.pending_count(), 0);
        assert_eq!(s.peek_next_id(), None);
    }
}
