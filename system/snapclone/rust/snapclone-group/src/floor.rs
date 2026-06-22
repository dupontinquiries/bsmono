//! floor.rs — half-duplex floor control ("talking stick") for a group voice
//! channel (the group form of Buzz radio; see proposal_buzz_walkie_talkie.md).
//!
//! Half-duplex means at most ONE member transmits at a time, so people don't
//! talk over each other on the channel. There is no server, so there is no
//! authority to hand out the floor — instead **each member keeps its own view**
//! of who holds it, updated by announcements that ride the existing group
//! transport:
//!
//!   • we press talk        → `request()`        (grab if free)
//!   • we keep talking       → `request()` again  (doubles as keepalive)
//!   • we stop                → `release()`
//!   • a peer starts/keeps    → `remote_assert(holder, claim_ts, now)`
//!   • a peer stops           → `remote_release(holder)`
//!   • time passes            → `tick(now)`        (timeout + fairness cap)
//!
//! Two members can grab a free floor at nearly the same instant. That race is
//! resolved **deterministically and identically on every node** by a tiebreak
//! on `(claim_ts, holder_id)` — the earlier claim wins, ties break to the
//! smaller id. Because every node applies the same rule to the same
//! announcements, they converge on the same holder; the loser sees
//! `holds_floor()` flip to false and stops transmitting (optimistic-grab +
//! reconcile, the same shape as the rest of the P2P design).
//!
//! `claim_ts` is a caller-supplied comparable timestamp (a logical clock, or
//! NTP-ish wall time). It is used ONLY for the tiebreak ordering. Liveness
//! (timeout / fairness) uses the LOCAL clock the caller passes as `now_ms`, so
//! a peer with a skewed clock can't keep the floor forever.
//!
//! Pure, no I/O, no allocation beyond the holder id — unit-tested below. The
//! transport that carries the announcements, and the audio it gates, are
//! runtime glue layered on top.

use crate::dag::NodeId;

/// Who (if anyone) holds the floor, from this member's point of view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Floor {
    /// Nobody is talking — the floor is free to grab.
    Open,
    /// `holder` is talking; `claim_ts` is their claim timestamp (for tiebreak).
    Held { holder: NodeId, claim_ts: u64 },
}

/// Liveness tuning for the floor.
#[derive(Clone, Copy, Debug)]
pub struct FloorConfig {
    /// The holder must re-assert (keepalive) within this many ms or the floor is
    /// considered abandoned and reopens — covers a talker who crashed, walked
    /// out of range, or whose release was lost.
    pub hold_timeout_ms: u64,
    /// Hard cap on a single turn (fairness): even a holder who keeps asserting
    /// loses the floor after this long, so one person can't monopolise it.
    pub max_hold_ms: u64,
}

impl Default for FloorConfig {
    fn default() -> Self {
        FloorConfig {
            hold_timeout_ms: 1_500, // ~1.5 s without keepalive ⇒ abandoned
            max_hold_ms: 30_000,    // 30 s max turn
        }
    }
}

/// One member's view of the group floor.
pub struct FloorControl {
    me: NodeId,
    cfg: FloorConfig,
    floor: Floor,
    /// Local-clock time the current hold began (for the max-hold fairness cap).
    held_since_local_ms: u64,
    /// Local-clock time of the most recent assert/keepalive for the current
    /// holder (for the abandonment timeout).
    last_assert_local_ms: u64,
    /// Whether WE currently intend to talk (set by request, cleared by release).
    want_floor: bool,
}

impl FloorControl {
    pub fn new(me: NodeId, cfg: FloorConfig) -> Self {
        FloorControl {
            me,
            cfg,
            floor: Floor::Open,
            held_since_local_ms: 0,
            last_assert_local_ms: 0,
            want_floor: false,
        }
    }

    /// We want to talk (or are still talking — call repeatedly to keep alive).
    /// Returns true if we hold the floor now and may transmit; false if someone
    /// else holds it (caller should back off / show "busy").
    ///
    /// `claim_ts` is our claim timestamp for tiebreaking against a simultaneous
    /// grabber; `now_ms` is the local clock.
    pub fn request(&mut self, claim_ts: u64, now_ms: u64) -> bool {
        self.want_floor = true;
        match &self.floor {
            Floor::Open => {
                self.take_self(claim_ts, now_ms);
                true
            }
            Floor::Held { holder, .. } if *holder == self.me => {
                self.last_assert_local_ms = now_ms; // keepalive
                true
            }
            Floor::Held { .. } => false, // someone else has it
        }
    }

    /// We stop talking. Releases the floor if we held it.
    pub fn release(&mut self, _now_ms: u64) {
        self.want_floor = false;
        if let Floor::Held { holder, .. } = &self.floor {
            if *holder == self.me {
                self.floor = Floor::Open;
            }
        }
    }

    /// A remote member announced it is taking or keeping the floor. Applies the
    /// deterministic tiebreak on contention. Returns whether WE hold the floor
    /// afterward (false means we just lost/were-denied it).
    pub fn remote_assert(&mut self, holder: NodeId, claim_ts: u64, now_ms: u64) -> bool {
        match &self.floor {
            Floor::Open => {
                self.set_holder(holder, claim_ts, now_ms);
            }
            Floor::Held { holder: cur, claim_ts: cur_ts } => {
                if *cur == holder {
                    // keepalive from the same holder
                    self.last_assert_local_ms = now_ms;
                } else {
                    // contention: earlier claim wins; tie → smaller id.
                    let incoming_wins = (claim_ts, &holder) < (*cur_ts, cur);
                    if incoming_wins {
                        self.set_holder(holder, claim_ts, now_ms);
                    }
                    // else: keep current holder, ignore the loser's claim
                }
            }
        }
        self.holds_floor()
    }

    /// A remote member announced it released the floor. Opens the floor only if
    /// that member is the one we currently believe holds it.
    pub fn remote_release(&mut self, holder: &NodeId) {
        if let Floor::Held { holder: cur, .. } = &self.floor {
            if cur == holder {
                self.floor = Floor::Open;
            }
        }
    }

    /// Advance the local clock. Reopens the floor if the holder went silent past
    /// `hold_timeout_ms` (abandoned) or the turn exceeded `max_hold_ms` (fair
    /// share). A no-op when the floor is already open.
    pub fn tick(&mut self, now_ms: u64) {
        if let Floor::Held { .. } = &self.floor {
            let abandoned =
                now_ms.saturating_sub(self.last_assert_local_ms) >= self.cfg.hold_timeout_ms;
            let too_long =
                now_ms.saturating_sub(self.held_since_local_ms) >= self.cfg.max_hold_ms;
            if abandoned || too_long {
                self.floor = Floor::Open;
            }
        }
    }

    /// Do WE hold the floor right now?
    pub fn holds_floor(&self) -> bool {
        matches!(&self.floor, Floor::Held { holder, .. } if *holder == self.me)
    }

    /// May we transmit this instant? (We must both want the floor and hold it.)
    pub fn may_transmit(&self) -> bool {
        self.want_floor && self.holds_floor()
    }

    /// Who holds the floor, if anyone.
    pub fn current_holder(&self) -> Option<&NodeId> {
        match &self.floor {
            Floor::Held { holder, .. } => Some(holder),
            Floor::Open => None,
        }
    }

    pub fn floor(&self) -> &Floor {
        &self.floor
    }

    pub fn config(&self) -> &FloorConfig {
        &self.cfg
    }

    fn take_self(&mut self, claim_ts: u64, now_ms: u64) {
        self.floor = Floor::Held { holder: self.me.clone(), claim_ts };
        self.held_since_local_ms = now_ms;
        self.last_assert_local_ms = now_ms;
    }

    fn set_holder(&mut self, holder: NodeId, claim_ts: u64, now_ms: u64) {
        self.floor = Floor::Held { holder, claim_ts };
        self.held_since_local_ms = now_ms;
        self.last_assert_local_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc(me: &str) -> FloorControl {
        FloorControl::new(me.to_string(), FloorConfig::default())
    }

    #[test]
    fn open_request_grants_floor() {
        let mut f = fc("alice");
        assert!(f.request(100, 100));
        assert!(f.holds_floor());
        assert!(f.may_transmit());
        assert_eq!(f.current_holder().map(|s| s.as_str()), Some("alice"));
    }

    #[test]
    fn held_by_other_request_denied() {
        let mut f = fc("alice");
        f.remote_assert("bob".into(), 50, 50);
        assert!(!f.request(100, 100));
        assert!(!f.holds_floor());
        assert!(!f.may_transmit());
        assert_eq!(f.current_holder().map(|s| s.as_str()), Some("bob"));
    }

    #[test]
    fn release_opens_floor() {
        let mut f = fc("alice");
        assert!(f.request(100, 100));
        f.release(200);
        assert_eq!(f.floor(), &Floor::Open);
        assert!(!f.may_transmit());
    }

    #[test]
    fn remote_release_opens_floor() {
        let mut f = fc("alice");
        f.remote_assert("bob".into(), 50, 50);
        f.remote_release(&"bob".to_string());
        assert_eq!(f.floor(), &Floor::Open);
    }

    #[test]
    fn contention_earlier_claim_wins() {
        // We grabbed it at ts=100; bob claims an EARLIER ts=50 → bob wins.
        let mut f = fc("alice");
        assert!(f.request(100, 100));
        let still_ours = f.remote_assert("bob".into(), 50, 110);
        assert!(!still_ours);
        assert!(!f.holds_floor());
        assert_eq!(f.current_holder().map(|s| s.as_str()), Some("bob"));
    }

    #[test]
    fn contention_later_claim_loses() {
        // We grabbed it at ts=50; bob claims a LATER ts=100 → we keep it.
        let mut f = fc("alice");
        assert!(f.request(50, 50));
        let still_ours = f.remote_assert("bob".into(), 100, 60);
        assert!(still_ours);
        assert!(f.holds_floor());
    }

    #[test]
    fn contention_tie_breaks_to_smaller_id_deterministically() {
        // Same claim_ts; "alice" < "bob" lexicographically, so alice wins on
        // every node regardless of arrival order.
        // Node observing as a third party "carol":
        let mut carol = fc("carol");
        carol.remote_assert("bob".into(), 100, 100);   // bob arrives first
        carol.remote_assert("alice".into(), 100, 101);  // alice ties → wins
        assert_eq!(carol.current_holder().map(|s| s.as_str()), Some("alice"));
        // Reverse arrival order on another node → same winner.
        let mut dave = fc("dave");
        dave.remote_assert("alice".into(), 100, 100);
        dave.remote_assert("bob".into(), 100, 101);     // bob ties but loses
        assert_eq!(dave.current_holder().map(|s| s.as_str()), Some("alice"));
    }

    #[test]
    fn holder_timeout_reopens_floor() {
        let cfg = FloorConfig { hold_timeout_ms: 1000, max_hold_ms: 60_000 };
        let mut f = FloorControl::new("alice".into(), cfg);
        f.remote_assert("bob".into(), 0, 0); // bob holds at local t=0
        f.tick(500);                          // within timeout
        assert_eq!(f.current_holder().map(|s| s.as_str()), Some("bob"));
        f.tick(1000);                         // hit the abandonment timeout
        assert_eq!(f.floor(), &Floor::Open);
    }

    #[test]
    fn keepalive_prevents_timeout() {
        let cfg = FloorConfig { hold_timeout_ms: 1000, max_hold_ms: 60_000 };
        let mut f = FloorControl::new("alice".into(), cfg);
        f.remote_assert("bob".into(), 0, 0);
        f.remote_assert("bob".into(), 0, 800); // keepalive at t=800 refreshes
        f.tick(1500);                            // 700ms since refresh < 1000
        assert_eq!(f.current_holder().map(|s| s.as_str()), Some("bob"));
        f.tick(1900);                            // 1100ms since refresh ⇒ reopen
        assert_eq!(f.floor(), &Floor::Open);
    }

    #[test]
    fn max_hold_caps_a_monopolizing_holder() {
        let cfg = FloorConfig { hold_timeout_ms: 10_000, max_hold_ms: 2000 };
        let mut f = FloorControl::new("alice".into(), cfg);
        assert!(f.request(0, 0));
        // We keep asserting (keepalive) so the abandonment timeout never fires…
        f.request(0, 500);
        f.request(0, 1000);
        f.request(0, 1500);
        assert!(f.holds_floor());
        // …but the fairness cap still pulls the floor at max_hold.
        f.tick(2000);
        assert_eq!(f.floor(), &Floor::Open);
    }

    #[test]
    fn request_acts_as_keepalive_for_self() {
        let cfg = FloorConfig { hold_timeout_ms: 1000, max_hold_ms: 60_000 };
        let mut f = FloorControl::new("alice".into(), cfg);
        assert!(f.request(0, 0));
        assert!(f.request(0, 800)); // keepalive
        f.tick(1500);                // 700ms since last request < 1000 ⇒ still ours
        assert!(f.holds_floor());
    }

    #[test]
    fn unknown_remote_release_is_ignored() {
        let mut f = fc("alice");
        assert!(f.request(100, 100));
        f.remote_release(&"bob".to_string()); // bob doesn't hold it; no effect
        assert!(f.holds_floor());
    }

    #[test]
    fn may_transmit_requires_intent_and_hold() {
        // Holding without wanting (e.g. stale) shouldn't transmit; our model
        // only sets Held{me} via request which also sets want_floor, so the
        // pairing is enforced. After release, intent clears.
        let mut f = fc("alice");
        assert!(f.request(100, 100));
        assert!(f.may_transmit());
        f.release(200);
        assert!(!f.may_transmit());
    }
}
