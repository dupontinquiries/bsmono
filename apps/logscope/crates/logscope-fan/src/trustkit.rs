//! trustkit.rs — a machine gets your conversation only if you said it could.
//!
//! ## Why this is not a preference
//!
//! `workerkit::assign` preferred unmetered workers as a TIE-BREAK. That is the wrong mechanism for a
//! data-egress rule: a tie-break can be lost. Add a capable enough remote and the sort picks it, and
//! the conversation leaves the machines you own without anything having gone wrong.
//!
//! Trust is therefore a FILTER applied before ranking, and an untrusted worker is not in the candidate
//! set at all. The distinction matters because the failure mode of a preference is silent and the
//! failure mode of a filter is a visible refusal.
//!
//! ## Registration is explicit and per-machine
//!
//! A worker is trusted because a person said so, once, naming it. Not because it is on the same subnet
//! — a café LAN is a subnet — and not because it answered a discovery broadcast, which is what an
//! attacker on that LAN would also do.
//!
//! ## What "seamless handoff" requires beyond trust
//!
//! Picking a conversation up on another machine means the CONTENT moved there. So the same registration
//! that permits compute permits storage, and they should not be separable — a machine that may generate
//! your next turn has already seen the previous ones. Splitting the two would suggest a protection that
//! does not exist.

/// How much of your data a worker may see.
///
/// Ordered deliberately: each level implies the ones before it, so a comparison is meaningful and a
/// caller cannot accidentally grant storage without compute.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Trust {
    /// Never contacted for anything carrying conversation content.
    Unknown,
    /// May run builds and tests: sees source, not conversation.
    Build,
    /// May generate: sees prompts and replies for the current session.
    Compute,
    /// May hold the conversation for handoff: sees history, and keeps it.
    Handoff,
}

impl Trust {
    /// Does this level permit a duty?
    pub fn permits(self, d: crate::workerkit::Duty) -> bool {
        use crate::workerkit::Duty::*;
        match d {
            Build => self >= Trust::Build,
            // Everything that generates sees the prompt, which is conversation content.
            Thinker | FanOutBranch | SpeculativeDraft | Liaison | Checker => self >= Trust::Compute,
        }
    }

    /// May a conversation be resumed here?
    pub fn permits_handoff(self) -> bool { self >= Trust::Handoff }

    /// One line for a refusal, naming what the operator would have to do.
    pub fn why_refused(self, d: crate::workerkit::Duty) -> String {
        format!("this machine is registered as {self:?}, and {d:?} requires at least {:?}. Register it \
explicitly to allow that — it is not granted by being on the same network",
            if matches!(d, crate::workerkit::Duty::Build) { Trust::Build } else { Trust::Compute })
    }
}

/// The registry of machines a person has vouched for.
///
/// Absence is the default and means `Unknown`. A registry that defaulted to anything else would make
/// forgetting to configure it indistinguishable from configuring it permissively.
#[derive(Clone, Debug, Default)]
pub struct Registry { entries: Vec<(String, Trust, Option<Vec<u8>>)> }

impl Registry {
    pub fn new() -> Self { Self::default() }

    /// Register a machine at a level, WITHOUT a key.
    ///
    /// Retained for callers that only need the level, but a name-only entry cannot be authenticated —
    /// see `register_keyed`. `peerauth::verify` refuses such a peer rather than trusting the claim.
    pub fn register(&mut self, id: &str, level: Trust) {
        match self.entries.iter_mut().find(|(k, _, _)| k == id) {
            Some(e) => e.1 = level,
            None => self.entries.push((id.to_string(), level, None)),
        }
    }

    /// Register a machine and bind its public key.
    ///
    /// THE GAP THIS CLOSES: registration recorded a level against a NAME. An id is whatever a peer
    /// claims, so without a bound key the filter is decorative — anyone says "I am m2" and receives
    /// what m2 was granted. `peerauth` consumes this key, and refuses any peer that has none.
    pub fn register_keyed(&mut self, id: &str, level: Trust, key: Vec<u8>) {
        match self.entries.iter_mut().find(|(k, _, _)| k == id) {
            Some(e) => { e.1 = level; e.2 = Some(key); }
            None => self.entries.push((id.to_string(), level, Some(key))),
        }
    }

    /// The key bound at registration, if any.
    pub fn key(&self, id: &str) -> Option<&[u8]> {
        self.entries.iter().find(|(k, _, _)| k == id).and_then(|(_, _, v)| v.as_deref())
    }

    /// Registered machines that cannot be authenticated, for a UI to flag.
    ///
    /// Worth surfacing rather than silently downgrading: an operator who registered a machine expects
    /// it to work, and "trusted but unverifiable" is a state they should see rather than discover when
    /// a connection is refused.
    pub fn unverifiable(&self) -> Vec<&str> {
        self.entries.iter().filter(|(_, t, k)| *t != Trust::Unknown && k.is_none())
            .map(|(id, _, _)| id.as_str()).collect()
    }

    /// Revoke immediately.
    ///
    /// Downgrades rather than removes nothing: a revoked machine must fall to Unknown, not to whatever
    /// a default would have given it.
    pub fn revoke(&mut self, id: &str) { self.register(id, Trust::Unknown); }

    pub fn level(&self, id: &str) -> Trust {
        self.entries.iter().find(|(k, _, _)| k == id).map(|(_, t, _)| *t).unwrap_or(Trust::Unknown)
    }

    /// Filter workers to those permitted for a duty. Applied BEFORE ranking, never as a tie-break.
    pub fn eligible<'a>(&self, d: crate::workerkit::Duty, ws: &'a [crate::workerkit::Worker])
        -> Vec<&'a crate::workerkit::Worker> {
        ws.iter().filter(|w| self.level(&w.id).permits(d)).collect()
    }

    /// Machines a conversation may be resumed on.
    pub fn handoff_targets(&self) -> Vec<&str> {
        self.entries.iter().filter(|(_, t, _)| t.permits_handoff()).map(|(k, _, _)| k.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workerkit::{Duty, Worker};

    fn w(id: &str) -> Worker {
        Worker { id: id.into(), rtt_ms: Some(20), capacity_gib: 999.0, metered: false }
    }

    #[test]
    fn an_unregistered_machine_gets_NOTHING_by_default() {
        // Absence must mean Unknown. Any other default makes forgetting to configure the registry
        // indistinguishable from configuring it permissively.
        let r = Registry::new();
        assert_eq!(r.level("stranger"), Trust::Unknown);
        for d in [Duty::Thinker, Duty::Build, Duty::Liaison, Duty::Checker] {
            assert!(!r.level("stranger").permits(d), "{d:?} must be refused");
        }
    }

    #[test]
    fn trust_is_a_FILTER_not_a_tie_break() {
        // The correction. workerkit::assign preferred unmetered as a tie-break, and a tie-break can be
        // LOST — add a capable enough remote and the sort picks it, and the conversation leaves your
        // machines without anything having gone wrong. A filter refuses visibly instead.
        let mut r = Registry::new();
        r.register("mine", Trust::Compute);
        let ws = vec![w("mine"), w("huge-remote")];
        let ok = r.eligible(Duty::Thinker, &ws);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].id, "mine", "capacity cannot outvote registration");
    }

    #[test]
    fn a_build_only_machine_cannot_generate() {
        // The useful middle level: a build box sees source and never a prompt. Generating anything
        // means seeing the conversation, so every generating duty needs Compute.
        let mut r = Registry::new();
        r.register("ci", Trust::Build);
        assert!(r.level("ci").permits(Duty::Build));
        assert!(!r.level("ci").permits(Duty::Thinker));
        assert!(!r.level("ci").permits(Duty::Checker), "a checker reads the artifact and the prompt");
    }

    #[test]
    fn handoff_requires_a_level_above_compute() {
        // Compute sees the session; Handoff KEEPS it. A machine that may generate your next turn has
        // already seen the previous ones, but retaining them afterwards is a separate grant.
        let mut r = Registry::new();
        r.register("laptop", Trust::Compute);
        r.register("desktop", Trust::Handoff);
        assert!(!r.level("laptop").permits_handoff());
        assert!(r.level("desktop").permits_handoff());
        assert_eq!(r.handoff_targets(), vec!["desktop"]);
    }

    #[test]
    fn revoking_drops_to_unknown_not_to_a_default() {
        // A revoked machine must lose everything. Falling back to whatever a default would grant is how
        // a revocation quietly does nothing.
        let mut r = Registry::new();
        r.register("gone", Trust::Handoff);
        r.revoke("gone");
        assert_eq!(r.level("gone"), Trust::Unknown);
        assert!(!r.level("gone").permits(Duty::Build));
    }

    #[test]
    fn the_levels_are_ordered_so_a_comparison_is_meaningful() {
        // Ord is derived and load-bearing: `>= Trust::Compute` must not accidentally admit Build.
        assert!(Trust::Handoff > Trust::Compute);
        assert!(Trust::Compute > Trust::Build);
        assert!(Trust::Build > Trust::Unknown);
    }

    #[test]
    fn a_refusal_says_what_would_change_it() {
        // "Not permitted" teaches nobody. The message names the required level and states that a shared
        // network does not confer it — because a café LAN is a subnet, and so is home.
        let m = Trust::Unknown.why_refused(Duty::Thinker);
        assert!(m.contains("Register it explicitly"));
        assert!(m.contains("not granted by being on the same network"));
    }

    #[test]
    fn the_filter_and_the_ranker_compose_with_the_filter_FIRST() {
        // End to end, and the property that matters: an untrusted machine is not in the candidate set,
        // so no amount of capacity brings it back. assign() now takes a pre-filtered slice, which means
        // the ordering cannot be got wrong by a caller who forgets.
        let mut r = Registry::new();
        r.register("mine", Trust::Compute);
        let ws = vec![
            Worker { id: "mine".into(), rtt_ms: Some(20), capacity_gib: 8.0, metered: false },
            Worker { id: "enormous".into(), rtt_ms: Some(20), capacity_gib: 999.0, metered: false },
        ];
        let eligible = r.eligible(Duty::Thinker, &ws);
        let chosen = crate::workerkit::assign(Duty::Thinker, &eligible).unwrap();
        assert_eq!(chosen.id, "mine", "the 999 GiB machine is unregistered and never considered");
    }

    #[test]
    fn a_name_only_registration_is_flagged_as_UNVERIFIABLE() {
        // The gap peerauth surfaced. An id is whatever a peer claims, so a level bound to a name and
        // not a key means anyone claiming that name receives what it was granted. Surfaced rather than
        // silently downgraded: an operator who registered a machine expects it to work, and "trusted
        // but unverifiable" is a state they should SEE rather than discover at connection time.
        let mut r = Registry::new();
        r.register("named-only", Trust::Compute);
        r.register_keyed("proper", Trust::Compute, vec![1, 2, 3]);
        assert_eq!(r.unverifiable(), vec!["named-only"]);
        assert!(r.key("named-only").is_none());
        assert_eq!(r.key("proper"), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn an_unknown_machine_is_not_listed_as_unverifiable() {
        // It is not trusted at all, so "trusted but unverifiable" would be the wrong description and
        // would fill the list with every stranger that ever answered a broadcast.
        let r = Registry::new();
        assert!(r.unverifiable().is_empty());
    }

    #[test]
    fn adding_a_key_later_upgrades_the_same_entry() {
        // Registering by name then supplying the key must not create a second entry, or a revocation
        // could land on the keyless one and leave the keyed one live.
        let mut r = Registry::new();
        r.register("m", Trust::Build);
        r.register_keyed("m", Trust::Handoff, vec![9]);
        assert_eq!(r.level("m"), Trust::Handoff);
        assert_eq!(r.key("m"), Some(&[9u8][..]));
        r.revoke("m");
        assert_eq!(r.level("m"), Trust::Unknown, "the single entry is revoked");
    }

    #[test]
    fn re_registering_updates_rather_than_duplicating() {
        // Two entries for one machine would make the effective level depend on lookup order, which is
        // the kind of thing that works until a revocation lands on the wrong copy.
        let mut r = Registry::new();
        r.register("m", Trust::Build);
        r.register("m", Trust::Handoff);
        assert_eq!(r.level("m"), Trust::Handoff);
        r.revoke("m");
        assert_eq!(r.level("m"), Trust::Unknown, "revocation hits the only entry");
    }
}
