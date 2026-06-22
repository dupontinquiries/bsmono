//! v0.1.662: assist-bubble surfacing primitive — the SCHEDULER (pure core).
//!
//! One shared channel for active assists (session_auto proposals, recall
//! re-entry, "why so slow", stuck-cadence nudge), so they never become the
//! "thrown in because there was space" furniture the user called out. The
//! principle: a bubble must earn its place by a workflow MOMENT, be
//! TRANSIENT, and never NAG.
//!
//! This scheduler is the pure guarantee of "never nags": given a stream of
//! candidate assists (each a typed kind, fired only on detected state), it
//! decides Show vs Suppress under a policy — max concurrent, per-kind
//! cooldown, global cooldown, and user snooze ("don't show this kind"). The
//! QML overlay (transient bottom-corner, auto + manual dismiss, voice-
//! pickable action) is the wiring layer; this decides WHEN it may appear.

use std::collections::{HashMap, HashSet};

/// Why a candidate was suppressed (for logging / tuning, never user-facing
/// as jargon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suppress {
    /// the user snoozed this kind ("don't show this kind")
    Snoozed,
    /// too soon since the last bubble of the same kind
    PerKindCooldown,
    /// too soon since ANY bubble (global anti-nag floor)
    GlobalCooldown,
    /// the visible channel is already full
    MaxConcurrent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Show,
    Suppress(Suppress),
}

/// Anti-nag policy. Defaults are conservative: at most one bubble visible,
/// the same kind no more than every 5 minutes, and any bubble no more than
/// every 20 seconds.
#[derive(Debug, Clone)]
pub struct BubblePolicy {
    pub max_concurrent: usize,
    pub per_kind_cooldown_secs: u64,
    pub global_cooldown_secs: u64,
}

impl Default for BubblePolicy {
    fn default() -> Self {
        BubblePolicy { max_concurrent: 1, per_kind_cooldown_secs: 300, global_cooldown_secs: 20 }
    }
}

/// Scheduler state. One per session. `now` is unix seconds, supplied by the
/// caller (keeps this pure + testable).
#[derive(Debug, Clone, Default)]
pub struct BubbleScheduler {
    policy: BubblePolicy,
    last_shown_per_kind: HashMap<String, u64>,
    last_shown_any: u64,
    /// kinds currently on screen (awaiting dismiss).
    visible: Vec<String>,
    /// kinds the user snoozed for this session.
    snoozed: HashSet<String>,
    /// whether we've shown anything yet (so the first bubble isn't blocked
    /// by global cooldown vs a zero timestamp).
    shown_any: bool,
}

impl BubbleScheduler {
    pub fn new(policy: BubblePolicy) -> Self {
        BubbleScheduler { policy, ..Default::default() }
    }
    pub fn with_defaults() -> Self { Self::new(BubblePolicy::default()) }

    /// Offer a candidate assist of `kind` at time `now`. Returns whether it
    /// may surface; on Show, records it as visible (call `dismiss` when the
    /// bubble closes).
    pub fn offer(&mut self, kind: &str, now: u64) -> Decision {
        if self.snoozed.contains(kind) {
            return Decision::Suppress(Suppress::Snoozed);
        }
        if self.visible.len() >= self.policy.max_concurrent {
            return Decision::Suppress(Suppress::MaxConcurrent);
        }
        if self.shown_any
            && now.saturating_sub(self.last_shown_any) < self.policy.global_cooldown_secs {
            return Decision::Suppress(Suppress::GlobalCooldown);
        }
        if let Some(&last) = self.last_shown_per_kind.get(kind) {
            if now.saturating_sub(last) < self.policy.per_kind_cooldown_secs {
                return Decision::Suppress(Suppress::PerKindCooldown);
            }
        }
        // accept
        self.last_shown_per_kind.insert(kind.to_string(), now);
        self.last_shown_any = now;
        self.shown_any = true;
        self.visible.push(kind.to_string());
        Decision::Show
    }

    /// A bubble of `kind` was dismissed (auto-timeout or user close). Frees a
    /// concurrent slot. Does NOT reset the cooldown (so dismiss-then-retrigger
    /// can't nag).
    pub fn dismiss(&mut self, kind: &str) {
        if let Some(pos) = self.visible.iter().position(|k| k == kind) {
            self.visible.remove(pos);
        }
    }

    /// User chose "don't show this kind" — suppress it for the session.
    pub fn snooze(&mut self, kind: &str) {
        self.snoozed.insert(kind.to_string());
        self.dismiss(kind);
    }
    pub fn unsnooze(&mut self, kind: &str) { self.snoozed.remove(kind); }
    pub fn is_snoozed(&self, kind: &str) -> bool { self.snoozed.contains(kind) }
    pub fn visible_count(&self) -> usize { self.visible.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_offer_shows() {
        let mut s = BubbleScheduler::with_defaults();
        assert_eq!(s.offer("reentry", 1000), Decision::Show);
        assert_eq!(s.visible_count(), 1);
    }

    #[test]
    fn max_concurrent_blocks_second() {
        let mut s = BubbleScheduler::new(BubblePolicy { max_concurrent: 1, ..Default::default() });
        assert_eq!(s.offer("a", 1000), Decision::Show);
        // different kind, but channel full
        assert_eq!(s.offer("b", 1000), Decision::Suppress(Suppress::MaxConcurrent));
    }

    #[test]
    fn global_cooldown_blocks_rapid_fire() {
        let mut s = BubbleScheduler::new(BubblePolicy { max_concurrent: 2, global_cooldown_secs: 20, per_kind_cooldown_secs: 300 });
        assert_eq!(s.offer("a", 1000), Decision::Show);
        s.dismiss("a"); // free the slot, but global cooldown still applies
        assert_eq!(s.offer("b", 1005), Decision::Suppress(Suppress::GlobalCooldown));
        // after the global cooldown, a different kind may show
        assert_eq!(s.offer("b", 1025), Decision::Show);
    }

    #[test]
    fn per_kind_cooldown_blocks_repeat() {
        let mut s = BubbleScheduler::new(BubblePolicy { max_concurrent: 2, global_cooldown_secs: 0, per_kind_cooldown_secs: 300 });
        assert_eq!(s.offer("reentry", 1000), Decision::Show);
        s.dismiss("reentry");
        // same kind again soon → blocked by per-kind cooldown even though
        // global cooldown is 0 and a slot is free
        assert_eq!(s.offer("reentry", 1100), Decision::Suppress(Suppress::PerKindCooldown));
        // a DIFFERENT kind is fine
        assert_eq!(s.offer("slow", 1100), Decision::Show);
        // same kind after the cooldown → ok
        s.dismiss("reentry");
        assert_eq!(s.offer("reentry", 1400), Decision::Show);
    }

    #[test]
    fn snooze_suppresses_kind_forever() {
        let mut s = BubbleScheduler::new(BubblePolicy { max_concurrent: 2, global_cooldown_secs: 0, per_kind_cooldown_secs: 0 });
        s.snooze("newtab");
        assert!(s.is_snoozed("newtab"));
        assert_eq!(s.offer("newtab", 1000), Decision::Suppress(Suppress::Snoozed));
        assert_eq!(s.offer("newtab", 999999), Decision::Suppress(Suppress::Snoozed));
        // unsnooze restores it
        s.unsnooze("newtab");
        assert_eq!(s.offer("newtab", 999999), Decision::Show);
    }

    #[test]
    fn dismiss_frees_a_slot() {
        let mut s = BubbleScheduler::new(BubblePolicy { max_concurrent: 1, global_cooldown_secs: 0, per_kind_cooldown_secs: 0 });
        assert_eq!(s.offer("a", 1000), Decision::Show);
        assert_eq!(s.offer("b", 1000), Decision::Suppress(Suppress::MaxConcurrent));
        s.dismiss("a");
        assert_eq!(s.offer("b", 1000), Decision::Show);
    }

    #[test]
    fn realistic_reboot_storm_does_not_nag() {
        // Hector reboots 6 times in a session; each reboot offers a reentry
        // bubble. With a 5-min per-kind cooldown, only the ones spaced out
        // actually show — no nagging on rapid reboots.
        let mut s = BubbleScheduler::with_defaults();
        let reboot_times = [100u64, 130, 200, 800, 820, 1500]; // secs
        let mut shown = 0;
        for t in reboot_times {
            if let Decision::Show = s.offer("reentry", t) { shown += 1; s.dismiss("reentry"); }
        }
        // 100 shows; 130/200 within 5min of 100 → blocked; 800 (>300 since
        // 100) shows; 820 blocked; 1500 (>300 since 800) shows → 3 total.
        assert_eq!(shown, 3, "rapid reboots collapsed to non-nagging bubbles");
    }
}
