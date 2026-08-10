//! peerrecord.rs — what a peer publishes, and why it cannot be believed.
//!
//! `reachability` decides whether a discovered peer may be assigned work. This is the thing it
//! discovers: the record a machine puts in the DHT so others can find it.
//!
//! ## A DHT record is unauthenticated
//!
//! **Anyone can publish a record under any key.** Nothing in a plain DHT stops a third party
//! writing a record for your identity pointing at their address — and a fleet that trusts records
//! will hand them work, source and whatever the work reads.
//!
//! That is not a reason to avoid the DHT. It is a reason the record must be **signed by the key the
//! identity is derived from**, so a forged record is detectable without asking anyone. The DHT then
//! carries data whose authenticity does not depend on the DHT.
//!
//! ## This overlaps `snapclone-presence::gossip::PeerRecord`, and the difference is the point
//!
//! That one exists and is a scaffold: `sc_id`, `address`, `seen_at`. No signature, no TTL, no
//! capability. **Checked before writing this**, because six duplicate implementations have already
//! been found in this tree.
//!
//! The interesting difference is not the extra fields, it is `seen_at` versus `published_at`:
//!
//! * **`seen_at` is OUR clock** — the publisher cannot forge it, and it does not survive the record
//!   being passed on by a third party, because then it records when the RELAY saw it.
//! * **`published_at` is THEIRS** — forgeable, which is why it is signed, and it means the same
//!   thing however many hops it travels.
//!
//! Neither replaces the other. A gossip view that only ever hears directly from peers is safer with
//! `seen_at`; a DHT record fetched from strangers needs `published_at` and a signature. **The merge
//! should keep both**, and this module should move into snapclone-presence rather than persist beside it.
//!
//! (That crate was `snapclone-dht` until the network triage: its gossip, presence and identity
//! modules were the only part not duplicating snapcore, and they were extracted so the rest could
//! go. The overlap is unchanged — only the address is.)
//!
//! ## Staleness is not the same as absence
//!
//! A record outlives the machine that published it: the publisher can vanish and its record sits
//! there until TTL. So a record carries **when it was published**, not merely that it exists, and
//! consumers compare that against their own clock rather than trusting a "still alive" flag the
//! publisher chose.

/// What a machine publishes about itself.
#[derive(Clone, PartialEq, Debug)]
pub struct PeerRecord {
    /// The identity this claims to be.
    pub id: String,
    /// Reflexive address discovered by STUN, as `host:port`.
    pub addr: String,
    /// Target triple — what `deviceplan` needs to know whether a build can be verified here.
    pub target_triple: String,
    /// Usable accelerator memory, zero for none. **A claim, not a measurement**: the publisher is
    /// describing itself and may be wrong or lying.
    pub gpu_gib: f32,
    /// Unix seconds at publication, by the PUBLISHER's clock.
    pub published_at: u64,
    /// Seconds the publisher believes this stays valid.
    pub ttl_s: u32,
    /// Signature over everything above, by the key the id derives from.
    pub signature: Option<Vec<u8>>,
}

/// Why a record must not be acted on.
///
/// Named `RecordFault` rather than `Reject` because a crate this size collides on generic nouns by
/// default — the duplicate ratchet caught this one on the run that added it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RecordFault {
    /// No signature at all.
    Unsigned,
    /// Signed, and not by the key this identity derives from.
    WrongSigner,
    /// Past its TTL.
    Expired { age_s: u64, ttl_s: u32 },
    /// Published in the future by more than clock skew allows.
    FromTheFuture { ahead_s: u64 },
}

impl RecordFault {
    pub fn explain(&self) -> String {
        match self {
            RecordFault::Unsigned =>
                "unsigned. A DHT accepts a write under any key from anyone, so an unsigned record is \
a claim by an unknown party — and a fleet that acts on it hands work, source and whatever that work \
reads to whoever published it".into(),
            RecordFault::WrongSigner =>
                "signed by a key this identity does not derive from. That is not a corrupt record, \
it is someone else's record wearing this name".into(),
            RecordFault::Expired { age_s, ttl_s } => format!(
                "published {age_s}s ago against a {ttl_s}s TTL. A record OUTLIVES the machine that \
wrote it, so an expired one is the normal trace of a peer that went away rather than evidence of \
anything wrong"),
            RecordFault::FromTheFuture { ahead_s } => format!(
                "timestamped {ahead_s}s in the future. Beyond plausible clock skew this is either a \
badly set clock or a replay with a bumped timestamp, and neither is safe to treat as fresh"),
        }
    }

    /// Is this worth telling the operator about, or is it routine?
    ///
    /// **Expiry is routine and the others are not.** A fleet where machines come and go produces
    /// expired records constantly; a wrongly-signed one is a fact about someone else's behaviour.
    pub fn noteworthy(&self) -> bool { !matches!(self, RecordFault::Expired { .. }) }
}

/// Clock skew tolerated before a record is treated as future-dated.
///
/// Generous, because unsynchronised clocks are ordinary and rejecting a good peer costs more than
/// accepting a slightly odd timestamp.
pub const SKEW_TOLERANCE_S: u64 = 300;

/// Check a record against the current time and a signature verifier.
///
/// The verifier is passed in rather than imported so this stays testable without a crypto
/// dependency, and so the identity scheme can be snapclone's `ScId` without this module knowing it.
pub fn validate_record(
    r: &PeerRecord,
    now_s: u64,
    verify: impl FnOnce(&PeerRecord, &[u8]) -> bool,
) -> Result<(), RecordFault> {
    // Signature FIRST. Every other field is the publisher's claim, so validating freshness before
    // authenticity means reasoning carefully about numbers an attacker chose.
    let Some(sig) = r.signature.as_deref() else { return Err(RecordFault::Unsigned) };
    if !verify(r, sig) { return Err(RecordFault::WrongSigner) }

    if r.published_at > now_s + SKEW_TOLERANCE_S {
        return Err(RecordFault::FromTheFuture { ahead_s: r.published_at - now_s });
    }
    let age = now_s.saturating_sub(r.published_at);
    if age > r.ttl_s as u64 { return Err(RecordFault::Expired { age_s: age, ttl_s: r.ttl_s }) }
    Ok(())
}

/// How long a record should live.
///
/// **Short enough that a departed peer disappears, long enough that republishing is not constant.**
/// Ten minutes: a peer that vanishes is gone from the fleet's view within one coffee break, and a
/// live peer republishes six times an hour rather than continuously.
pub const DEFAULT_TTL_S: u32 = 600;

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(sig: Option<Vec<u8>>, published: u64) -> PeerRecord {
        PeerRecord {
            id: "sc-abc123".into(), addr: "203.0.113.7:4001".into(),
            target_triple: "aarch64-unknown-linux-gnu".into(), gpu_gib: 24.0,
            published_at: published, ttl_s: DEFAULT_TTL_S, signature: sig,
        }
    }
    fn good(_r: &PeerRecord, _s: &[u8]) -> bool { true }
    fn bad(_r: &PeerRecord, _s: &[u8]) -> bool { false }

    #[test]
    fn an_UNSIGNED_record_is_refused_and_says_what_trusting_it_would_cost() {
        // A DHT accepts a write under any key from anyone.
        let e = validate_record(&rec(None, 1000), 1000, good).unwrap_err();
        assert_eq!(e, RecordFault::Unsigned);
        assert!(e.explain().contains("hands work, source and whatever that work reads"));
    }

    #[test]
    fn a_WRONGLY_SIGNED_record_is_someone_elses_wearing_this_name() {
        // Not a corrupt record — a different fault with a different response.
        let e = validate_record(&rec(Some(vec![1, 2, 3]), 1000), 1000, bad).unwrap_err();
        assert!(e.explain().contains("someone else's record wearing this name"));
    }

    #[test]
    fn the_SIGNATURE_is_checked_BEFORE_the_timestamps() {
        // Every other field is the publisher's claim, so validating freshness first means reasoning
        // about numbers an attacker chose.
        let mut r = rec(None, 0);
        r.published_at = 99_999_999;      // both expired-in-the-future AND unsigned
        assert_eq!(validate_record(&r, 1000, good).unwrap_err(), RecordFault::Unsigned);
    }

    #[test]
    fn an_EXPIRED_record_is_ROUTINE_and_the_others_are_not() {
        // A fleet where machines come and go produces expired records constantly; a wrongly-signed
        // one is a fact about someone else's behaviour.
        let expired = validate_record(&rec(Some(vec![1]), 0), 100_000, good).unwrap_err();
        assert!(!expired.noteworthy());
        assert!(RecordFault::Unsigned.noteworthy());
        assert!(expired.explain().contains("normal trace of a peer that went away"));
    }

    #[test]
    fn CLOCK_SKEW_is_tolerated_and_a_real_future_date_is_not() {
        // Unsynchronised clocks are ordinary, and rejecting a good peer costs more than accepting a
        // slightly odd timestamp.
        assert!(validate_record(&rec(Some(vec![1]), 1200), 1000, good).is_ok(), "200s ahead is skew");
        let e = validate_record(&rec(Some(vec![1]), 100_000), 1000, good).unwrap_err();
        assert!(matches!(e, RecordFault::FromTheFuture { .. }));
        assert!(e.explain().contains("replay with a bumped timestamp"));
    }

    #[test]
    fn a_FRESH_signed_record_passes() {
        assert!(validate_record(&rec(Some(vec![1]), 1000), 1100, good).is_ok());
    }

    #[test]
    fn gpu_gib_is_documented_as_a_CLAIM_rather_than_a_measurement() {
        // The publisher is describing itself and may be wrong or lying, so a scheduler must not size
        // work from this without probing.
        let src = include_str!("peerrecord.rs");
        assert!(src.contains("A claim, not a measurement"));
    }

    #[test]
    fn the_overlap_with_snapclone_dht_is_DOCUMENTED_rather_than_silent() {
        // Seventh duplicate found in this tree, and the first one checked for BEFORE writing rather
        // than after. The doc has to survive, or the next reader re-derives the same module again.
        let src = include_str!("peerrecord.rs");
        assert!(src.contains("snapclone-presence::gossip::PeerRecord"));
        assert!(src.contains("should move into snapclone-presence rather than persist beside it"));
    }

    #[test]
    fn the_TTL_is_short_enough_that_a_departed_peer_disappears() {
        // Within one coffee break, against republishing six times an hour rather than continuously.
        assert!(DEFAULT_TTL_S <= 900);
        assert!(DEFAULT_TTL_S >= 120, "and long enough that republishing is not constant");
    }
}
