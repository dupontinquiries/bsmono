//! workerkit.rs — which roles can live on another machine, and which cannot.
//!
//! A phone with a nearby M2 Ultra and an M4 mini, plus a remote API, is not one pool of compute. Each
//! role has a different tolerance for a round trip, and the difference is decisive rather than marginal.
//!
//! ## The arithmetic that decides placement
//!
//! ```text
//!   thinker, 40-token reply    gen 1628 ms + 30 ms rtt  =  1.8% overhead
//!                              gen 1628 ms + 150 ms rtt =  8.4%
//!   speculative draft, /round  gen   50 ms + 30 ms rtt  = 37.5% overhead
//!                              gen   50 ms + 150 ms rtt = 75.0%
//! ```
//!
//! A thinker amortises the round trip over a whole reply. A **speculative draft alternates with its
//! target every few tokens, so the round trip IS the cost** — and it exceeds the generation it was
//! meant to accelerate. That is `coresidency`'s finding arriving over the network instead of over a
//! PCIe bus: draft and target must be co-located, whatever the link.
//!
//! **Fan-out is not speculation.** Independent branches each generate a full reply before anything is
//! compared, so they amortise exactly as a thinker does. An M4 mini running fan-out branches is a good
//! use of a second machine; the same mini running a speculative draft for a remote target is not.
//!
//! ## KV cache does not cross, and mostly need not
//!
//! A KV cache is gigabytes and is invalid the moment the prompt differs. Shipping it is not worth
//! considering. What matters is that the roles which need a warm cache are the ones that stay local
//! anyway — and a remote thinker re-prefills, which is a cost paid once per turn rather than per token.

/// Where a role can run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    /// Must be on the same machine as its partner. Latency per token, not per turn.
    Colocated,
    /// A local network is fine; a WAN degrades but works.
    Lan,
    /// Any link, including a metered API over a slow connection.
    Anywhere,
}

/// What a worker is being asked to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Duty { Thinker, FanOutBranch, SpeculativeDraft, Liaison, Checker, Build }

impl Duty {
    /// How far this duty can be from the thing it serves.
    pub fn reach(self) -> Reach {
        match self {
            // One call, one long reply. The round trip is amortised over hundreds of tokens.
            Duty::Thinker => Reach::Anywhere,
            // A full independent reply before anything is compared — amortises like a thinker.
            Duty::FanOutBranch => Reach::Anywhere,
            // Alternates with its target every few tokens. The rtt exceeds the work it saves.
            Duty::SpeculativeDraft => Reach::Colocated,
            // Consulted constantly and interactively; a user waits on it.
            Duty::Liaison => Reach::Lan,
            // One verdict per artifact, and the artifact is small.
            Duty::Checker => Reach::Anywhere,
            // Minutes of work per dispatch.
            Duty::Build => Reach::Anywhere,
        }
    }

    /// Why, in one line, for a UI that has to explain a refusal.
    pub fn why(self) -> &'static str {
        match self {
            Duty::SpeculativeDraft => "a draft alternates with its target every few tokens, so a round \
trip costs more than the speculation saves — 37% overhead on a LAN, 75% on a slower link",
            Duty::Liaison => "the liaison answers while you wait, so a slow link is felt directly rather \
than amortised",
            _ => "one call per turn or per artifact, so the round trip amortises over the whole reply",
        }
    }
}

/// A machine offering capacity.
#[derive(Clone, PartialEq, Debug)]
pub struct Worker {
    pub id: String,
    /// Measured round trip in milliseconds. None = never contacted.
    pub rtt_ms: Option<u32>,
    /// Largest model it can hold resident, in GiB.
    pub capacity_gib: f32,
    /// True for a metered remote API rather than a machine you own.
    pub metered: bool,
}

impl Worker {
    fn class(&self) -> Reach {
        match self.rtt_ms {
            Some(r) if r <= 5 => Reach::Colocated,
            Some(r) if r <= 60 => Reach::Lan,
            _ => Reach::Anywhere,
        }
    }
    /// Can this worker take this duty?
    pub fn can_take(&self, d: Duty) -> bool {
        match (d.reach(), self.class()) {
            (Reach::Colocated, Reach::Colocated) => true,
            (Reach::Colocated, _) => false,
            (Reach::Lan, Reach::Anywhere) => false,
            _ => true,
        }
    }
}

/// Choose a worker for a duty.
///
/// Prefers the largest capacity among those that can take it, and breaks ties toward the UNMETERED
/// machine — a local M2 that can hold the model should be used before an API that bills per token, even
/// when the API is nominally more capable.
pub fn assign<'a>(d: Duty, workers: &'a [Worker]) -> Option<&'a Worker> {
    workers.iter().filter(|w| w.can_take(d))
        .max_by(|a, b| {
            (a.metered.cmp(&b.metered).reverse())      // unmetered first
                .then(a.capacity_gib.partial_cmp(&b.capacity_gib).unwrap_or(std::cmp::Ordering::Equal))
        })
}

/// Explain a placement, or why none was possible.
pub fn explain(d: Duty, chosen: Option<&Worker>) -> String {
    match chosen {
        Some(w) => format!("{:?} -> {} ({} GiB{}{})", d, w.id, w.capacity_gib,
            if w.metered { ", metered" } else { "" },
            w.rtt_ms.map(|r| format!(", {r} ms")).unwrap_or_default()),
        None => format!("{:?} has no eligible worker: {}", d, d.why()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: &str, rtt: Option<u32>, gib: f32, metered: bool) -> Worker {
        Worker { id: id.into(), rtt_ms: rtt, capacity_gib: gib, metered }
    }

    #[test]
    fn a_speculative_draft_cannot_cross_the_network() {
        // The pushback on using a remote mini as a fan-out draft. Speculation alternates every few
        // tokens, so the round trip IS the cost — 37% overhead on a LAN, 75% on anything worse. This is
        // coresidency's finding arriving over a network instead of a bus.
        let phone = w("phone", Some(0), 4.0, false);
        let mini = w("m4-mini", Some(20), 24.0, false);
        assert!(phone.can_take(Duty::SpeculativeDraft));
        assert!(!mini.can_take(Duty::SpeculativeDraft), "20 ms per round is not a draft");
        assert!(Duty::SpeculativeDraft.why().contains("costs more than the speculation saves"));
    }

    #[test]
    fn a_fan_out_branch_CAN_cross_because_it_is_not_speculation() {
        // The distinction that makes a second machine useful. Independent branches each generate a full
        // reply before anything is compared, so they amortise exactly as a thinker does.
        let mini = w("m4-mini", Some(20), 24.0, false);
        assert!(mini.can_take(Duty::FanOutBranch));
        assert_eq!(Duty::FanOutBranch.reach(), Reach::Anywhere);
        assert_ne!(Duty::FanOutBranch.reach(), Duty::SpeculativeDraft.reach());
    }

    #[test]
    fn a_thinker_may_be_a_remote_api() {
        // One call, one long reply: 1.8% overhead on a LAN and 8.4% on a slow link. That is what makes
        // a remote frontier model usable as the thinker at all.
        let api = w("api", Some(400), 999.0, true);
        assert!(api.can_take(Duty::Thinker));
    }

    #[test]
    fn an_unmetered_local_machine_beats_a_metered_api_of_equal_reach() {
        // A local M2 that can hold the model should be used before something that bills per token, even
        // when the API is nominally more capable — the tie-break has to be explicit or capacity alone
        // always picks the API.
        let ws = vec![w("m2", Some(15), 48.0, false), w("api", Some(300), 999.0, true)];
        assert_eq!(assign(Duty::Thinker, &ws).unwrap().id, "m2");
    }

    #[test]
    fn a_metered_api_is_still_chosen_when_nothing_local_qualifies() {
        // Preferring local must not mean refusing to work. With only the API eligible, it is the answer.
        let ws = vec![w("api", Some(300), 999.0, true)];
        assert_eq!(assign(Duty::Thinker, &ws).unwrap().id, "api");
        assert!(assign(Duty::SpeculativeDraft, &ws).is_none(), "but not for a draft");
    }

    #[test]
    fn a_liaison_wants_a_LAN_and_refuses_a_wan() {
        // A user waits on the liaison, so a slow link is felt directly rather than amortised.
        assert!(w("mini", Some(20), 24.0, false).can_take(Duty::Liaison));
        assert!(!w("api", Some(400), 999.0, true).can_take(Duty::Liaison));
    }

    #[test]
    fn an_impossible_placement_explains_itself_rather_than_returning_nothing() {
        // A UI showing "no worker" teaches nobody why. The reason is the actionable part — it tells the
        // operator to move the draft, not to find a faster network.
        let ws = vec![w("api", Some(400), 999.0, true)];
        let e = explain(Duty::SpeculativeDraft, assign(Duty::SpeculativeDraft, &ws));
        assert!(e.contains("no eligible worker"));
        assert!(e.contains("37% overhead"));
    }

    #[test]
    fn an_uncontacted_worker_is_treated_as_distant() {
        // rtt None means unmeasured. Assuming it is close would place a draft on a machine that may be
        // anywhere, and the failure would be a slow generation nobody attributes to placement.
        let unknown = w("new", None, 32.0, false);
        assert!(!unknown.can_take(Duty::SpeculativeDraft));
        assert!(unknown.can_take(Duty::Thinker));
    }
}
