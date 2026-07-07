//! reconcile — speculative local recognition with **asynchronous reference correction**.
//!
//! The rule: the on-device fastpath **decides and executes immediately** — we never block a command
//! on a heavy/remote model. Separately, *when a better model is available* (a Heavy host, possibly
//! offloaded via [`crate::mesh`]), we compute a **reference** transcription in the background and
//! reconcile it against what we already served:
//!
//! * **match** → reinforce; after a couple of confirmations the local mapping becomes
//!   **authoritative** and we stop paying for the reference (the device's own DB is now the source
//!   of truth for that utterance);
//! * **differ** → the reference **wins** and we rewrite the local mapping, so next time the fastpath
//!   is right.
//!
//! Net effect: **local latency always**, and the local DB keeps converging toward the best model's
//! quality for the commands this user actually says. The authority is the *issuing device's* own
//! reference-validated database — not the cloud, and never on the critical path.

/// How trustworthy the local mapping for an utterance is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// Not yet confirmed by a reference — serve it, but a reference check is still worthwhile.
    Unvalidated,
    /// Reference-confirmed enough times — the local DB is the fast source of truth; skip the
    /// reference next time.
    Authoritative,
}

/// A learned mapping from an utterance signature to a command text, with its validation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Stable signature of the utterance (audio fingerprint / normalized phrase).
    pub key: String,
    /// The command text the fastpath serves for this utterance (local authority once validated).
    pub text: String,
    /// How many times a reference has confirmed `text`.
    pub confirmations: u32,
}

/// Confirmations needed before the local mapping is treated as authoritative (and reference checks
/// can be skipped). Small on purpose — a couple of agreements from the good model is plenty.
pub const CONFIRM_THRESHOLD: u32 = 2;

impl Entry {
    pub fn new(key: impl Into<String>, text: impl Into<String>) -> Self {
        Entry { key: key.into(), text: text.into(), confirmations: 0 }
    }
    pub fn authority(&self) -> Authority {
        if self.confirmations >= CONFIRM_THRESHOLD {
            Authority::Authoritative
        } else {
            Authority::Unvalidated
        }
    }
    /// Is it worth spending a (possibly offloaded) reference computation on this utterance? No once
    /// the mapping is authoritative — the point is to learn what's uncertain, not re-check what the
    /// device already owns.
    pub fn needs_reference(&self) -> bool {
        self.authority() == Authority::Unvalidated
    }
}

/// The outcome of reconciling the already-served local `text` against a later `reference`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciliation {
    /// No reference was available (offline / no Heavy host) — keep local, still unvalidated.
    NoReference,
    /// Reference agreed — reinforced; `now_authoritative` if it just crossed the threshold.
    Confirmed { now_authoritative: bool },
    /// Reference disagreed — the DB was corrected to the reference; re-validation starts over.
    Corrected { from: String, to: String },
}

/// Reconcile a served local result against a background reference, mutating the entry's learned
/// state. The local command has ALREADY run by the time this is called — this only teaches the DB.
pub fn reconcile(entry: &mut Entry, reference: Option<&str>) -> Reconciliation {
    match reference {
        None => Reconciliation::NoReference,
        Some(r) if r == entry.text => {
            let was_auth = entry.authority() == Authority::Authoritative;
            entry.confirmations = entry.confirmations.saturating_add(1);
            let now_auth = entry.authority() == Authority::Authoritative;
            Reconciliation::Confirmed { now_authoritative: now_auth && !was_auth }
        }
        Some(r) => {
            let from = std::mem::replace(&mut entry.text, r.to_string());
            entry.confirmations = 1; // the corrected value has one confirmation (this reference)
            Reconciliation::Corrected { from, to: r.to_string() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmations_promote_to_authoritative_then_skip_reference() {
        let mut e = Entry::new("k", "open mail");
        assert!(e.needs_reference()); // unvalidated at first
        assert_eq!(reconcile(&mut e, Some("open mail")), Reconciliation::Confirmed { now_authoritative: false });
        // second agreement crosses CONFIRM_THRESHOLD
        assert_eq!(reconcile(&mut e, Some("open mail")), Reconciliation::Confirmed { now_authoritative: true });
        assert_eq!(e.authority(), Authority::Authoritative);
        assert!(!e.needs_reference(), "authoritative → stop paying for reference");
    }

    #[test]
    fn reference_wins_on_disagreement_and_db_learns() {
        let mut e = Entry::new("k", "open male"); // fastpath misheard
        let r = reconcile(&mut e, Some("open mail"));
        assert_eq!(r, Reconciliation::Corrected { from: "open male".into(), to: "open mail".into() });
        assert_eq!(e.text, "open mail", "the DB now serves the reference next time");
        assert_eq!(e.authority(), Authority::Unvalidated, "corrected value re-validates");
    }

    #[test]
    fn correction_then_confirmation_becomes_authoritative() {
        let mut e = Entry::new("k", "wrong");
        reconcile(&mut e, Some("right")); // corrected → confirmations = 1
        let r = reconcile(&mut e, Some("right")); // agrees → confirmations = 2 → authoritative
        assert_eq!(r, Reconciliation::Confirmed { now_authoritative: true });
        assert_eq!(e.text, "right");
    }

    #[test]
    fn no_reference_keeps_local_unvalidated() {
        let mut e = Entry::new("k", "play jazz");
        assert_eq!(reconcile(&mut e, None), Reconciliation::NoReference);
        assert!(e.needs_reference(), "still wants a reference when one becomes available");
        assert_eq!(e.text, "play jazz", "local result stands — it already ran, at full speed");
    }
}
