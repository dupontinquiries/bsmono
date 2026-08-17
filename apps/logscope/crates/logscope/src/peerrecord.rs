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

// ---------------------------------------------------------------- wire format

/// A field the space-delimited wire can carry: non-empty, ASCII, no whitespace. Same reasoning as
/// microdht::valid_id — the protocol cannot quote, so a space in a field is a silently different
/// record on the far side, not an error.
fn wire_token(s: &str) -> bool {
    !s.is_empty() && s.is_ascii() && !s.contains(|c: char| c.is_ascii_whitespace())
}

impl PeerRecord {
    /// The bytes the signature is over: everything except the signature, in one canonical order.
    ///
    /// THIS is what makes `validate_record`'s verifier callable at all. The struct had a signature
    /// field and a validator that checks it, and no defined preimage — so no two implementations
    /// could agree on what was signed, which is a signature scheme in name only. Versioned ("pr1")
    /// so a future field lands as a new version instead of a silent meaning change; `gpu_gib` uses
    /// f32 Display, Rust's shortest round-tripping form, so decode-then-re-preimage reproduces the
    /// signed bytes exactly.
    pub fn signing_preimage(&self) -> Result<String, String> {
        for (name, v) in [("id", &self.id), ("addr", &self.addr), ("target_triple", &self.target_triple)] {
            if !wire_token(v) {
                return Err(format!("{name} {v:?} cannot ride a space-delimited wire — the far side \
would parse it as a different record, silently"));
            }
        }
        if !self.gpu_gib.is_finite() || self.gpu_gib < 0.0 {
            return Err(format!("gpu_gib {} is not a claimable amount of memory", self.gpu_gib));
        }
        Ok(format!("pr1 {} {} {} {} {} {}", self.id, self.addr, self.target_triple,
                   self.gpu_gib, self.published_at, self.ttl_s))
    }

    /// The DHT value: preimage + signature, hex-signed. An UNSIGNED record is refused here rather
    /// than published — the whole point of the signature is that a fleet never acts on an
    /// unauthenticated claim, and the cheapest place to enforce that is before the bytes exist.
    pub fn encode_wire(&self) -> Result<Vec<u8>, String> {
        let sig = self.signature.as_deref()
            .ok_or("refusing to encode an UNSIGNED record: a DHT accepts a write under any key \
from anyone, so publishing this would put a claim on the wire that no reader can distinguish \
from a forgery")?;
        let mut s = self.signing_preimage()?;
        s.push(' ');
        for b in sig { s.push_str(&format!("{b:02x}")); }
        Ok(s.into_bytes())
    }

    /// Parse a DHT value back into a record. The signature is CARRIED, not checked — checking
    /// needs a verifier and a clock, which is `validate_record`'s job; conflating parse with
    /// trust is how unauthenticated fields end up read before the signature is.
    pub fn decode_wire(bytes: &[u8]) -> Result<PeerRecord, String> {
        let s = std::str::from_utf8(bytes).map_err(|_| "record is not UTF-8".to_string())?;
        let mut it = s.split(' ');
        match it.next() {
            Some("pr1") => {}
            Some(v) => return Err(format!("record version {v:?} is not pr1 — a version mismatch, \
not corruption; retrying will not help")),
            None => return Err("empty record".into()),
        }
        let mut field = |name: &str| it.next().ok_or_else(|| format!("record ends before {name}"));
        let id = field("id")?.to_string();
        let addr = field("addr")?.to_string();
        let target_triple = field("target_triple")?.to_string();
        let gpu_gib: f32 = field("gpu_gib")?.parse().map_err(|_| "gpu_gib is not a number".to_string())?;
        let published_at: u64 = field("published_at")?.parse().map_err(|_| "published_at is not a number".to_string())?;
        let ttl_s: u32 = field("ttl_s")?.parse().map_err(|_| "ttl_s is not a number".to_string())?;
        let sig_hex = field("signature")?;
        if it.next().is_some() { return Err("trailing fields after the signature — a longer future \
version, or a tampered record; either way not pr1".into()); }
        if sig_hex.len() % 2 != 0 { return Err("signature hex has odd length".into()); }
        let signature = (0..sig_hex.len()).step_by(2)
            .map(|i| u8::from_str_radix(&sig_hex[i..i + 2], 16)
                .map_err(|_| "signature is not hex".to_string()))
            .collect::<Result<Vec<u8>, _>>()?;
        Ok(PeerRecord { id, addr, target_triple, gpu_gib, published_at, ttl_s,
                        signature: Some(signature) })
    }
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
    #[test]
    fn the_wire_ROUND_TRIPS_and_the_preimage_survives_the_trip() {
        // The property signatures need: decode(encode(r)) re-derives the SAME preimage bytes, or a
        // faithfully relayed record fails verification at the far side.
        let r = PeerRecord { signature: Some(vec![0xde, 0xad, 0xbe]), ..rec(Some(vec![]), 1234) };
        let back = PeerRecord::decode_wire(&r.encode_wire().unwrap()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.signing_preimage().unwrap(), r.signing_preimage().unwrap());
    }

    #[test]
    fn gpu_gib_round_trips_EXACTLY_through_display() {
        // The preimage carries a float; if Display and parse disagreed by one ulp the signature
        // would fail only for certain claimed sizes — a heisen-forgery. Rust's shortest form
        // round-trips by contract; pinned here so a formatting "improvement" cannot break it.
        for g in [0.0f32, 7.5, 23.999998, f32::from_bits(0x4172_3ab7)] {
            let r = PeerRecord { gpu_gib: g, signature: Some(vec![1]), ..rec(Some(vec![]), 9) };
            let back = PeerRecord::decode_wire(&r.encode_wire().unwrap()).unwrap();
            assert_eq!(back.gpu_gib.to_bits(), g.to_bits(), "g={g}");
        }
    }

    #[test]
    fn an_UNSIGNED_record_cannot_reach_the_wire() {
        let e = rec(None, 100).encode_wire().unwrap_err();
        assert!(e.contains("UNSIGNED"), "{e}");
    }

    #[test]
    fn a_field_with_a_SPACE_is_refused_before_it_becomes_a_different_record() {
        let mut r = rec(Some(vec![1]), 100);
        r.target_triple = "aarch64 linux".into();
        assert!(r.signing_preimage().unwrap_err().contains("space-delimited"));
    }

    #[test]
    fn a_FUTURE_version_is_named_as_a_mismatch_not_corruption() {
        let e = PeerRecord::decode_wire(b"pr9 x y z 1 2 3 aa").unwrap_err();
        assert!(e.contains("not pr1"));
        assert!(e.contains("retrying will not help"));
    }

}
