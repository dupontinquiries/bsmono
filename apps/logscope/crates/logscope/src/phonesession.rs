//! phonesession.rs — the phone sleeps and the generation does not.
//!
//! `pairflow` gets connected. What follows is the case that happens on every long generation: the
//! screen locks, or the person switches apps, and the OS suspends the process within seconds.
//!
//! ## The work is not on the phone
//!
//! A naive client treats a dropped socket as a cancelled request — it shows an error and the person
//! starts again. **But the cluster is still generating.** The tokens exist, the KV cache is warm, and
//! reconnecting should hand back what was produced rather than beginning afresh.
//!
//! Getting this wrong is expensive in a way that is easy to miss: a 2000-token answer regenerated
//! from scratch costs the cluster the whole thing again, and the person waits twice for the same
//! text.
//!
//! ## Suspend is not disconnect and neither is cancel
//!
//! Three things that look alike from the socket's side and mean different things:
//!
//! * **suspended** — the OS froze the app. The person did not ask for anything.
//! * **disconnected** — the network went away. The person did not ask for anything.
//! * **cancelled** — the person pressed stop. **The only one where generation should end.**
//!
//! Treating the first two as the third throws away work nobody asked to discard.

/// Why the client stopped reading.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Detach {
    /// The OS suspended the app — backgrounded, screen locked.
    Suspended,
    /// The network dropped.
    NetworkLost,
    /// The person pressed stop.
    Cancelled,
}

impl Detach {
    /// Should the cluster stop generating?
    ///
    /// **Only for Cancelled.** The other two are things that happened TO the person, and ending
    /// their generation because their screen locked is a decision nobody made.
    pub fn should_stop_generation(self) -> bool { matches!(self, Detach::Cancelled) }

    pub fn explain(self) -> &'static str {
        match self {
            Detach::Suspended =>
                "the phone was suspended by the OS — generation continues and the tokens are held \
for reattachment, because the person did not ask for anything to stop",
            Detach::NetworkLost =>
                "the network went away — generation continues for a grace period, since a lift \
between wifi and cellular takes seconds and regenerating takes as long as the original",
            Detach::Cancelled =>
                "the person pressed stop — this is the only detach that ends generation, and it \
should free the KV cache immediately",
        }
    }
}

/// How long produced tokens are held for a client that went away.
///
/// **Long enough to cover a screen lock and a walk between rooms**, short enough that an abandoned
/// session does not hold a KV cache forever. A 2000-token answer regenerated costs more than five
/// minutes of cache.
pub const REATTACH_WINDOW_S: f32 = 300.0;

/// A generation the phone can come back to.
#[derive(Clone, PartialEq, Debug)]
pub struct HeldSession {
    pub id: String,
    /// Tokens produced so far, held for reattachment.
    pub produced: usize,
    /// Whether generation is still running or finished while detached.
    pub still_generating: bool,
    pub detached_for_s: f32,
}

/// What the phone should do on coming back.
#[derive(Clone, PartialEq, Debug)]
pub enum Resume {
    /// Reattach and stream the rest. Nothing was lost.
    Continue { already_have: usize, still_generating: bool },
    /// It finished while away — collect the whole thing.
    Collect { tokens: usize },
    /// Held too long. **Says what was lost rather than pretending nothing happened.**
    Expired { lost_tokens: usize, held_for_s: f32 },
}

impl Resume {
    pub fn caption(&self) -> String {
        match self {
            // Not "reconnecting" — that implies starting over, which is what the person fears when
            // their screen has been off for a minute.
            Resume::Continue { already_have, still_generating: true } => format!(
                "picking up where it left off — {already_have} tokens are already here and it is \
still writing"),
            Resume::Continue { already_have, still_generating: false } => format!(
                "picking up — {already_have} tokens were produced while you were away"),
            Resume::Collect { tokens } => format!("it finished while you were away — {tokens} tokens"),
            Resume::Expired { lost_tokens, held_for_s } => format!(
                "held for {:.0} minutes and then released, losing {lost_tokens} tokens. Anything \
past {:.0} minutes stops being worth a warm cache on the machine",
                held_for_s / 60.0, REATTACH_WINDOW_S / 60.0),
        }
    }

    /// Did the person lose work?
    pub fn lost_anything(&self) -> bool { matches!(self, Resume::Expired { .. }) }
}

/// Decide what a returning phone gets.
pub fn on_return(s: &HeldSession) -> Resume {
    if s.detached_for_s > REATTACH_WINDOW_S {
        return Resume::Expired { lost_tokens: s.produced, held_for_s: s.detached_for_s };
    }
    if s.still_generating {
        Resume::Continue { already_have: s.produced, still_generating: true }
    } else if s.produced > 0 {
        Resume::Collect { tokens: s.produced }
    } else {
        // Finished with nothing: an empty completion is a result and must not read as a loss, or the
        // person retries a prompt the model answered with silence for a reason.
        Resume::Continue { already_have: 0, still_generating: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(produced: usize, gen: bool, secs: f32) -> HeldSession {
        HeldSession { id: "s1".into(), produced, still_generating: gen, detached_for_s: secs }
    }

    #[test]
    fn a_SUSPENDED_phone_does_not_stop_the_generation() {
        // The OS froze the app; the person did not ask for anything. Ending their generation because
        // a screen locked is a decision nobody made.
        assert!(!Detach::Suspended.should_stop_generation());
        assert!(!Detach::NetworkLost.should_stop_generation());
        assert!(Detach::Cancelled.should_stop_generation());
    }

    #[test]
    fn CANCEL_is_the_only_one_that_frees_the_cache() {
        assert!(Detach::Cancelled.explain().contains("free the KV cache immediately"));
    }

    #[test]
    fn coming_back_MID_GENERATION_picks_up_rather_than_restarting() {
        // "Reconnecting" implies starting over, which is exactly what the person fears after their
        // screen has been off a minute.
        let r = on_return(&held(400, true, 30.0));
        assert!(r.caption().contains("picking up where it left off"));
        assert!(r.caption().contains("still writing"));
        assert!(!r.lost_anything());
    }

    #[test]
    fn a_generation_that_FINISHED_while_away_is_collected_whole() {
        let r = on_return(&held(2000, false, 120.0));
        assert_eq!(r, Resume::Collect { tokens: 2000 });
        assert!(r.caption().contains("finished while you were away"));
    }

    #[test]
    fn an_EXPIRED_session_says_what_was_lost() {
        // Pretending nothing happened is worse than the loss: the person re-asks and cannot tell
        // whether the first attempt worked.
        let r = on_return(&held(1500, true, 600.0));
        assert!(r.lost_anything());
        assert!(r.caption().contains("losing 1500 tokens"));
    }

    #[test]
    fn the_window_covers_a_screen_lock_and_a_walk_between_rooms() {
        // Long enough for the real case, short enough that an abandoned session does not hold a KV
        // cache forever — and a 2000-token regeneration costs more than five minutes of cache.
        assert!(REATTACH_WINDOW_S >= 120.0);
        assert!(REATTACH_WINDOW_S <= 600.0);
    }

    #[test]
    fn an_EMPTY_completion_is_a_result_not_a_loss() {
        // Otherwise the person retries a prompt the model answered with silence for a reason.
        let r = on_return(&held(0, false, 10.0));
        assert!(!r.lost_anything());
    }
}
