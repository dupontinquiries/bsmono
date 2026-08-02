//! punch — the UDP hole-punch state machine, stage 4's one remaining piece.
//!
//! `TransportSelector` already chooses `UdpPunchThenTcp { peer_addr, punch_port }`
//! and STUN, UPnP/NAT-PMP and the relay all exist in pure Rust. What was missing
//! is the thing that *executes* a punch: the ordered, timed exchange that gets
//! two NATed peers talking directly.
//!
//! ## Why a state machine rather than a function
//!
//! A punch is not a request/response. Both sides must transmit *simultaneously*
//! — the outbound packet is what opens the NAT binding that lets the peer's
//! packet in — so neither side can wait for the other. The exchange is a set of
//! deadlines, and deadlines are what a state machine is for. Written as a
//! blocking function it would deadlock on the first symmetric NAT.
//!
//! Time is a caller-supplied `u64` tick, like everywhere else in this crate, so
//! the whole FSM is deterministic and testable with no clock and no sockets.
//!
//! ## The rule that makes punching work at all
//!
//! **Keep sending while waiting.** A single probe that goes unanswered is
//! indistinguishable from a peer that is not there, and the binding it opened
//! expires in tens of seconds. Retransmitting on an interval is not a
//! reliability nicety here — it is the mechanism, because the peer may only
//! start punching after we do.

/// Where a punch attempt has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Not started.
    Idle,
    /// Sending probes and listening. Both sides sit here at once.
    Punching,
    /// A probe from the peer arrived: the path is open both ways.
    Open,
    /// Nothing arrived in time. The caller falls back — relay or mailbox.
    Failed(Reason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// No peer probe within the budget.
    Timeout,
    /// The peer's NAT assigned a different port per destination, so the address
    /// we were told to punch is already stale. Retrying cannot help; this is a
    /// distinct outcome from a timeout because the FALLBACK differs — a relay is
    /// the only answer, where a timeout might succeed on a second attempt.
    SymmetricNat,
    /// Cancelled by the caller.
    Cancelled,
}

/// What the caller should do this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send a probe to this address.
    SendProbe { to: String },
    /// Nothing to do; call again later.
    Wait,
    /// Terminal — stop calling.
    Done,
}

/// How often to retransmit, in ticks. Short enough that a peer starting late
/// still meets us inside its own NAT binding lifetime.
pub const PROBE_INTERVAL: u64 = 1;

/// Total budget before giving up. Long enough to cover a peer woken by a push
/// notification, short enough that a user is not staring at a spinner: the
/// fallback path is a relay, and reaching it late is worse than reaching it.
pub const BUDGET: u64 = 8;

/// One side of a punch.
#[derive(Debug, Clone)]
pub struct Punch {
    pub peer_addr: String,
    pub phase: Phase,
    started: u64,
    last_probe: Option<u64>,
    /// Distinct source addresses the peer's probes arrived from. More than one
    /// means a symmetric NAT is rewriting the port per destination.
    seen_from: Vec<String>,
    probes_sent: u32,
}

impl Punch {
    pub fn new(peer_addr: impl Into<String>) -> Punch {
        Punch { peer_addr: peer_addr.into(), phase: Phase::Idle,
                started: 0, last_probe: None, seen_from: Vec::new(), probes_sent: 0 }
    }

    pub fn start(&mut self, now: u64) {
        self.phase = Phase::Punching;
        self.started = now;
        self.last_probe = None;
    }

    /// Advance. Returns what to do now.
    pub fn poll(&mut self, now: u64) -> Action {
        match self.phase {
            Phase::Idle => Action::Wait,
            Phase::Open | Phase::Failed(_) => Action::Done,
            Phase::Punching => {
                if now.saturating_sub(self.started) >= BUDGET {
                    self.phase = Phase::Failed(Reason::Timeout);
                    return Action::Done;
                }
                let due = match self.last_probe {
                    None => true,
                    Some(t) => now.saturating_sub(t) >= PROBE_INTERVAL,
                };
                if due {
                    self.last_probe = Some(now);
                    self.probes_sent += 1;
                    // Keep sending while waiting: the binding we opened expires,
                    // and the peer may only start punching after we do.
                    Action::SendProbe { to: self.peer_addr.clone() }
                } else {
                    Action::Wait
                }
            }
        }
    }

    /// A probe arrived from `from`.
    ///
    /// Receiving anything at all means our outbound packet opened a binding the
    /// peer could reach — which is the whole test. The path is open.
    pub fn probe_received(&mut self, from: &str, _now: u64) {
        if self.phase != Phase::Punching { return; }
        if !self.seen_from.iter().any(|s| s == from) {
            self.seen_from.push(from.to_string());
        }
        // Probes from two different source addresses mean the peer's NAT is
        // rewriting the port per destination. Retrying cannot help, and calling
        // it a timeout would send the caller back for a second attempt that
        // cannot succeed.
        if self.seen_from.len() > 1 {
            self.phase = Phase::Failed(Reason::SymmetricNat);
            return;
        }
        self.phase = Phase::Open;
    }

    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Punching | Phase::Idle) {
            self.phase = Phase::Failed(Reason::Cancelled);
        }
    }

    pub fn is_open(&self) -> bool { self.phase == Phase::Open }
    pub fn probes_sent(&self) -> u32 { self.probes_sent }

    /// Should the caller fall back, and is a retry worth it?
    ///
    /// A timeout may succeed on a second attempt — the peer might have been
    /// asleep. A symmetric NAT will not, ever, so retrying wastes the user's
    /// time twice over.
    pub fn retry_worthwhile(&self) -> bool {
        matches!(self.phase, Phase::Failed(Reason::Timeout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> Punch {
        let mut p = Punch::new("203.0.113.7:41234");
        p.start(0);
        p
    }

    #[test]
    fn probing_starts_immediately_rather_than_after_an_interval() {
        // The first probe is what opens our NAT binding; waiting a tick to send
        // it delays the whole exchange for no reason.
        let mut p = started();
        assert_eq!(p.poll(0), Action::SendProbe { to: "203.0.113.7:41234".into() });
    }

    #[test]
    fn probes_keep_going_while_waiting() {
        // Not a reliability nicety: the binding we opened expires, and the peer
        // may only start punching after we do. One probe and silence is
        // indistinguishable from a peer that is not there.
        let mut p = started();
        p.poll(0);
        assert_eq!(p.poll(0), Action::Wait, "not twice in one tick");
        assert!(matches!(p.poll(1), Action::SendProbe { .. }));
        assert!(matches!(p.poll(2), Action::SendProbe { .. }));
        assert!(p.probes_sent() >= 3);
    }

    #[test]
    fn any_probe_from_the_peer_opens_the_path() {
        // Receiving anything means our outbound packet opened a binding the peer
        // could reach, which IS the test.
        let mut p = started();
        p.poll(0);
        p.probe_received("203.0.113.7:41234", 1);
        assert!(p.is_open());
        assert_eq!(p.poll(2), Action::Done, "and polling stops");
    }

    #[test]
    fn two_source_addresses_mean_symmetric_nat_not_a_timeout() {
        // The fallback DIFFERS: a relay is the only answer here, where a timeout
        // might succeed on a second attempt. Reporting it as a timeout sends the
        // caller back for an attempt that cannot work.
        let mut p = started();
        p.poll(0);
        p.probe_received("203.0.113.7:41234", 1);
        assert!(p.is_open());
        // a second, different source on a fresh attempt
        let mut q = started();
        q.poll(0);
        q.probe_received("203.0.113.7:41234", 1);
        q.phase = Phase::Punching;                  // still probing
        q.probe_received("203.0.113.7:51999", 2);
        assert_eq!(q.phase, Phase::Failed(Reason::SymmetricNat));
        assert!(!q.retry_worthwhile(), "retrying a symmetric NAT wastes time twice");
    }

    #[test]
    fn a_timeout_ends_the_attempt_and_is_worth_retrying() {
        // The peer may simply have been asleep.
        let mut p = started();
        for t in 0..BUDGET { p.poll(t); }
        assert_eq!(p.poll(BUDGET), Action::Done);
        assert_eq!(p.phase, Phase::Failed(Reason::Timeout));
        assert!(p.retry_worthwhile());
    }

    #[test]
    fn a_late_probe_after_the_budget_does_not_reopen_a_failed_punch() {
        // The caller has already fallen back to a relay; flipping to Open now
        // would leave two paths believing they are the live one.
        let mut p = started();
        for t in 0..=BUDGET { p.poll(t); }
        assert!(matches!(p.phase, Phase::Failed(_)));
        p.probe_received("203.0.113.7:41234", BUDGET + 1);
        assert!(!p.is_open(), "stays failed");
    }

    #[test]
    fn cancelling_stops_the_probes() {
        let mut p = started();
        p.poll(0);
        p.cancel();
        assert_eq!(p.poll(1), Action::Done);
        assert_eq!(p.phase, Phase::Failed(Reason::Cancelled));
        assert!(!p.retry_worthwhile(), "the user cancelled; do not try again");
    }

    #[test]
    fn an_unstarted_punch_sends_nothing() {
        let mut p = Punch::new("203.0.113.7:41234");
        assert_eq!(p.poll(5), Action::Wait);
        assert_eq!(p.probes_sent(), 0);
    }
}
