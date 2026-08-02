//! announce.rs — the LAN discovery datagram: what a machine says, and what a listener may believe.
//!
//! The transport is the flow's remaining blocker, and this is the part that must be right before any
//! socket exists: a wire format is the thing you cannot change later without breaking every peer.
//!
//! ## What an announcement may and may not do
//!
//! `rendezvous` established that discovery can only **locate** a machine you already registered, never
//! introduce one. So an announcement carries no trust claim — it says "id X is at address Y", and the
//! registry decides whether that matters. A listener that treats an announcement as a reason to trust
//! has reimplemented the thing `trustkit` exists to prevent.
//!
//! ## Why it is signed
//!
//! Without a signature anyone on the LAN can announce your peer's id at their own address and receive
//! whatever that peer was granted. The signature covers the id, the address and a timestamp, so an
//! announcement cannot be replayed at a different address or after it has aged out.
//!
//! The id is a fingerprint of the public key (`machineid`), so a listener can verify against the key it
//! registered without a separate key exchange — which is the property that made the id a fingerprint in
//! the first place.

/// Wire format version. First byte of every datagram.
///
/// Present from the start because adding it later means a flag day: an old peer reading a new datagram
/// has no way to know it should stop.
pub const WIRE_V1: u8 = 1;

/// How stale an announcement may be before a listener ignores it.
///
/// Bounds replay. Long enough for ordinary clock skew on a home network, short enough that a captured
/// datagram is useless by the time it is resent.
pub const MAX_AGE_S: u64 = 30;

/// What a machine broadcasts.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Announce {
    pub id: String,
    /// Where the sender believes it can be reached.
    pub addr: String,
    pub sent_at_s: u64,
}

/// Why an announcement was ignored.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reject {
    /// Not a version this build speaks.
    Version(u8),
    Malformed(&'static str),
    /// Too old, or from the future by more than the skew allowance.
    Stale { age_s: i64 },
    /// The signature does not verify under the key registered for this id.
    BadSignature,
    /// Nobody registered this id. Shown, never acted on.
    Unregistered,
}

impl Reject {
    pub fn explain(&self) -> String {
        match self {
            Reject::Version(v) => format!("wire version {v}, this build speaks {WIRE_V1}"),
            Reject::Malformed(w) => format!("malformed announcement: {w}"),
            Reject::Stale { age_s } => format!(
                "announced {age_s}s ago, past the {MAX_AGE_S}s window — a captured datagram must not \
stay usable, so this is refused rather than the window widened"),
            Reject::BadSignature => "the signature does not verify under the key registered for this \
id: either it is not that machine, or the key rotated and the registration is stale".into(),
            Reject::Unregistered => "nobody registered this id — shown so you can notice a machine you \
meant to add, and nothing was sent to it".into(),
        }
    }
}

impl Announce {
    /// The exact bytes signed, and the exact bytes on the wire before the signature.
    ///
    /// Domain-separated and length-prefixed for the same reasons as `peerauth`: a signature over this
    /// must not be valid over some other message, and without length prefixes `("ab","c")` and
    /// `("a","bc")` serialise identically, so an attacker who can influence one field forges the other.
    pub fn payload(&self) -> Vec<u8> {
        let mut v = vec![WIRE_V1];
        v.extend_from_slice(b"logscope-announce-v1\0");
        for part in [self.id.as_bytes(), self.addr.as_bytes()] {
            v.extend_from_slice(&(part.len() as u32).to_le_bytes());
            v.extend_from_slice(part);
        }
        v.extend_from_slice(&self.sent_at_s.to_le_bytes());
        v
    }

    /// Parse a datagram body (the part before the 64-byte signature).
    pub fn parse(buf: &[u8]) -> Result<Announce, Reject> {
        if buf.is_empty() { return Err(Reject::Malformed("empty")); }
        if buf[0] != WIRE_V1 { return Err(Reject::Version(buf[0])); }
        let tag = b"logscope-announce-v1\0";
        if buf.len() < 1 + tag.len() || &buf[1..1 + tag.len()] != tag {
            return Err(Reject::Malformed("wrong tag"));
        }
        let mut o = 1 + tag.len();
        let mut field = |o: &mut usize| -> Result<String, Reject> {
            if buf.len() < *o + 4 { return Err(Reject::Malformed("truncated length")); }
            let n = u32::from_le_bytes([buf[*o], buf[*o + 1], buf[*o + 2], buf[*o + 3]]) as usize;
            *o += 4;
            // A length that overruns the buffer is refused rather than clamped: clamping turns a
            // corrupt datagram into a plausible one.
            if buf.len() < *o + n { return Err(Reject::Malformed("truncated field")); }
            let s = String::from_utf8(buf[*o..*o + n].to_vec())
                .map_err(|_| Reject::Malformed("field is not utf-8"))?;
            *o += n;
            Ok(s)
        };
        let id = field(&mut o)?;
        let addr = field(&mut o)?;
        if buf.len() < o + 8 { return Err(Reject::Malformed("truncated timestamp")); }
        let sent_at_s = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
        Ok(Announce { id, addr, sent_at_s })
    }

    /// Is this fresh enough to act on?
    ///
    /// Rejects the future as well as the past: a datagram timestamped ahead of now would otherwise
    /// stay valid indefinitely, which is a replay window with extra steps.
    pub fn freshness(&self, now_s: u64) -> Result<(), Reject> {
        let age = now_s as i64 - self.sent_at_s as i64;
        if age.unsigned_abs() > MAX_AGE_S { return Err(Reject::Stale { age_s: age }); }
        Ok(())
    }
}

/// Decide what a listener does with a parsed announcement.
///
/// Takes the registry, so an unregistered id cannot be acted on — the same shape as
/// `rendezvous::resolve`, and for the same reason.
pub fn accept(a: &Announce, now_s: u64, reg: &crate::trustkit::Registry,
              verify: impl Fn(&[u8], &[u8]) -> bool) -> Result<crate::rendezvous::Candidate, Reject> {
    a.freshness(now_s)?;
    let key = match reg.key(&a.id) {
        Some(k) => k,
        None if reg.level(&a.id) == crate::trustkit::Trust::Unknown =>
            return Err(Reject::Unregistered),
        // Registered by name with no key: cannot be verified, so it cannot be acted on either.
        None => return Err(Reject::BadSignature),
    };
    if !verify(key, &a.payload()) { return Err(Reject::BadSignature); }
    Ok(crate::rendezvous::Candidate {
        addr: a.addr.clone(), route: crate::rendezvous::Route::Lan, age_s: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trustkit::{Registry, Trust};

    fn ann() -> Announce {
        Announce { id: "m2".into(), addr: "10.0.0.5:7000".into(), sent_at_s: 1000 }
    }
    fn reg_with_key() -> Registry {
        let mut r = Registry::new();
        r.register_keyed("m2", Trust::Compute, vec![9, 9, 9]);
        r
    }
    fn ok(_k: &[u8], _m: &[u8]) -> bool { true }
    fn no(_k: &[u8], _m: &[u8]) -> bool { false }

    #[test]
    fn it_round_trips() {
        let a = ann();
        assert_eq!(Announce::parse(&a.payload()).unwrap(), a);
    }

    #[test]
    fn a_TRUNCATED_field_is_refused_rather_than_clamped() {
        // Clamping turns a corrupt datagram into a plausible one, and a plausible address is one a
        // listener connects to.
        let mut b = ann().payload();
        b.truncate(b.len() - 4);
        assert!(matches!(Announce::parse(&b), Err(Reject::Malformed(_))));
    }

    #[test]
    fn a_length_that_OVERRUNS_the_buffer_is_refused() {
        let mut b = ann().payload();
        let o = 1 + b"logscope-announce-v1\0".len();
        b[o..o + 4].copy_from_slice(&9999u32.to_le_bytes());
        assert!(matches!(Announce::parse(&b), Err(Reject::Malformed(_))));
    }

    #[test]
    fn an_unknown_wire_version_is_named_not_guessed() {
        // The version byte exists from the start because adding it later is a flag day: an old peer
        // reading a new datagram has no way to know it should stop.
        let mut b = ann().payload();
        b[0] = 99;
        assert_eq!(Announce::parse(&b), Err(Reject::Version(99)));
    }

    #[test]
    fn the_payload_is_length_prefixed_so_fields_cannot_be_slid() {
        // Without this, ("ab","c") and ("a","bc") serialise identically and an attacker who can
        // influence one field forges the other — the same bug peerauth's payload avoids.
        let x = Announce { id: "ab".into(), addr: "c".into(), sent_at_s: 1 };
        let y = Announce { id: "a".into(), addr: "bc".into(), sent_at_s: 1 };
        assert_ne!(x.payload(), y.payload());
    }

    #[test]
    fn a_stale_OR_future_announcement_is_rejected() {
        // A datagram timestamped ahead of now would otherwise stay valid indefinitely — a replay
        // window with extra steps.
        assert!(ann().freshness(1000 + MAX_AGE_S).is_ok());
        assert!(ann().freshness(1000 + MAX_AGE_S + 1).is_err());
        assert!(ann().freshness(1000 - MAX_AGE_S - 1).is_err(), "the future is also refused");
    }

    #[test]
    fn an_UNREGISTERED_id_is_reported_and_not_acted_on() {
        // Discovery locates, it never introduces — rendezvous' rule, enforced here too because this is
        // where an attacker on the LAN would announce.
        let e = accept(&ann(), 1000, &Registry::new(), ok).unwrap_err();
        assert_eq!(e, Reject::Unregistered);
        assert!(e.explain().contains("nothing was sent to it"));
    }

    #[test]
    fn a_registered_id_with_NO_KEY_cannot_be_accepted() {
        // register() without a key leaves nothing to verify against, and accepting on the strength of
        // a matching name is exactly what the signature exists to prevent.
        let mut r = Registry::new();
        r.register("m2", Trust::Compute);
        assert_eq!(accept(&ann(), 1000, &r, ok).unwrap_err(), Reject::BadSignature);
    }

    #[test]
    fn a_good_announcement_yields_a_LAN_candidate() {
        let c = accept(&ann(), 1000, &reg_with_key(), ok).expect("accepted");
        assert_eq!(c.route, crate::rendezvous::Route::Lan);
        assert_eq!(c.age_s, 0, "just heard");
        assert_eq!(accept(&ann(), 1000, &reg_with_key(), no).unwrap_err(), Reject::BadSignature);
    }
}
