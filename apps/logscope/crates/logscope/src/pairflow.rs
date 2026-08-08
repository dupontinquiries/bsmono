//! pairflow.rs — connecting a phone to the cluster, and failing usefully when it does not.
//!
//! `pairing` makes the code, `reachability` decides whether a found peer can be reached. Between
//! them is the part the person actually experiences: a spinner, and then either a connection or a
//! sentence.
//!
//! ## On a phone the sentence is all you get
//!
//! There is no log to open, no terminal to check, no way to run a probe. **Every failure must be one
//! line and the action must be doable on the phone**, or the person's only recourse is to try again
//! and get the same result.
//!
//! ## Five causes wear one symptom
//!
//! "Could not connect" covers a mistyped code, a machine that is off, a NAT that will not open, the
//! wrong network, and an app that is not running on the other end. Those need five different
//! actions, and a single spinner-then-failure gives none of them.
//!
//! The states below exist to keep them apart, because the cheapest fix — "you are on cellular, join
//! the wifi" — is invisible if everything reports as a timeout.

/// Where the connection attempt is.
#[derive(Clone, PartialEq, Debug)]
pub enum Stage {
    /// Waiting for the code to be entered.
    AwaitingCode,
    /// The code is well-formed. **Says nothing about the peer** — it is a checksum, not a lookup.
    CodeAccepted,
    /// Looking the peer up.
    Locating,
    /// Found a record; trying to reach it.
    Connecting,
    Connected { device: String },
    Failed(Failure),
}

/// Why it did not connect, kept separate because each wants a different action.
#[derive(Clone, PartialEq, Debug)]
pub enum Failure {
    /// The checksum caught it. **Cheapest to fix and the only one the phone can settle alone.**
    BadCode { position: Option<usize> },
    /// No record anywhere. The other machine has probably never announced.
    NeverSeen,
    /// A record exists and is old.
    RecordStale { age_s: f32 },
    /// A record exists, is fresh, and packets do not arrive.
    Unreachable { on_cellular: bool },
    /// Reached something that refused. The app is likely not running or is a different version.
    Refused,
}

impl Failure {
    /// One line, and the action must be doable ON THE PHONE.
    ///
    /// "Check the firewall on the host" is true and useless to someone holding a phone in another
    /// room — so where the fix is not phone-doable, the line says who has to do it rather than
    /// pretending the person can.
    pub fn what_to_do(&self) -> String {
        match self {
            Failure::BadCode { position: Some(p) } => format!(
                "that code has a typo around character {} — the checksum caught it, so nothing was \
sent and nothing is wrong at the other end", p + 1),
            Failure::BadCode { position: None } =>
                "that code is not valid — check for a missing character rather than a wrong one, \
since the length is what failed".into(),
            Failure::NeverSeen =>
                "no machine has announced with that code. It is probably not running LogScope yet — \
this is the one case where the phone cannot help and someone has to start it".into(),
            Failure::RecordStale { age_s } => format!(
                "that machine last announced {:.0} minutes ago and has probably slept or moved. \
Waking it is enough; the code stays valid", age_s / 60.0),
            // The cheapest real fix and the one most often invisible.
            Failure::Unreachable { on_cellular: true } =>
                "found it, but packets are not getting through on cellular. Join the same wifi as \
the machine — that fixes this more often than anything else you can try from here".into(),
            Failure::Unreachable { on_cellular: false } =>
                "found it, and packets are not getting through. Both ends are probably behind \
routers that will not open a path; a relay is needed and this is not something the phone can \
arrange".into(),
            Failure::Refused =>
                "reached it and it refused. The app is running an older version, or is busy loading \
a model — worth one retry in a minute before assuming anything is wrong".into(),
        }
    }

    /// Can the person holding the phone fix this themselves?
    ///
    /// **Drives whether to offer a retry button or an explanation.** A retry on something the phone
    /// cannot change trains people to mash it.
    pub fn phone_can_fix(&self) -> bool {
        matches!(self, Failure::BadCode { .. } | Failure::Unreachable { on_cellular: true }
                     | Failure::Refused)
    }
}

impl Stage {
    /// What to show while this stage is current.
    ///
    /// **Each stage names what is happening rather than spinning.** "Connecting…" for eleven seconds
    /// tells the person nothing about whether to wait; "looking up the code" followed by "found it,
    /// opening a path" tells them it is progressing and roughly how far.
    pub fn caption(&self) -> String {
        match self {
            Stage::AwaitingCode => "enter the code shown on the machine".into(),
            Stage::CodeAccepted => "code looks right — checking whether that machine is online".into(),
            Stage::Locating => "looking it up".into(),
            Stage::Connecting => "found it, opening a path".into(),
            Stage::Connected { device } => format!("connected to {device}"),
            Stage::Failed(f) => f.what_to_do(),
        }
    }

    /// Should a retry be offered?
    pub fn offer_retry(&self) -> bool {
        match self { Stage::Failed(f) => f.phone_can_fix(), _ => false }
    }
}

/// How long a stage may sit before the caption is stale.
///
/// Past this the person is watching an unchanging screen, and a caption that has not moved is worse
/// than no caption because it implies progress that is not happening.
pub const STAGE_PATIENCE_S: f32 = 8.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_BAD_CODE_says_nothing_was_sent() {
        // The cheapest failure and the only one the phone settles alone. Saying "nothing is wrong at
        // the other end" stops someone walking to another room.
        let f = Failure::BadCode { position: Some(6) };
        assert!(f.what_to_do().contains("character 7"));
        assert!(f.what_to_do().contains("nothing is wrong at the other end"));
        assert!(f.phone_can_fix());
    }

    #[test]
    fn CELLULAR_gets_the_cheapest_real_fix_named_first() {
        // Found-but-unreachable on cellular is usually solved by joining the wifi, and that is
        // invisible if everything reports as a timeout.
        let f = Failure::Unreachable { on_cellular: true };
        assert!(f.what_to_do().contains("Join the same wifi"));
        assert!(f.phone_can_fix());
    }

    #[test]
    fn an_unreachable_peer_NOT_on_cellular_says_the_phone_cannot_fix_it() {
        // "Check the firewall on the host" is true and useless to someone holding a phone in another
        // room, so the line says who has to act rather than pretending they can.
        let f = Failure::Unreachable { on_cellular: false };
        assert!(!f.phone_can_fix());
        assert!(f.what_to_do().contains("not something the phone can arrange"));
    }

    #[test]
    fn RETRY_is_offered_only_where_it_could_help() {
        // A retry button on something the phone cannot change trains people to mash it, and then
        // they mash it on the one that would have worked.
        assert!(Stage::Failed(Failure::Refused).offer_retry());
        assert!(!Stage::Failed(Failure::NeverSeen).offer_retry());
    }

    #[test]
    fn NEVER_SEEN_admits_the_phone_is_not_the_place_to_fix_it() {
        let f = Failure::NeverSeen;
        assert!(f.what_to_do().contains("someone has to start it"));
        assert!(!f.phone_can_fix());
    }

    #[test]
    fn a_STALE_record_says_the_code_is_still_good() {
        // Otherwise the person re-pairs, which is several minutes of work for a machine that only
        // needs waking.
        let f = Failure::RecordStale { age_s: 900.0 };
        assert!(f.what_to_do().contains("the code stays valid"));
        assert!(f.what_to_do().contains("15 minutes"));
    }

    #[test]
    fn each_STAGE_names_what_is_happening_rather_than_spinning() {
        // "Connecting..." for eleven seconds says nothing about whether to wait.
        for s in [Stage::CodeAccepted, Stage::Locating, Stage::Connecting] {
            let c = s.caption();
            assert!(!c.is_empty() && !c.contains("..."), "{s:?}: {c}");
        }
        assert_ne!(Stage::Locating.caption(), Stage::Connecting.caption());
    }

    #[test]
    fn CODE_ACCEPTED_does_not_claim_the_peer_exists() {
        // It is a checksum, not a lookup, and "code accepted" reading as "connected" is how someone
        // waits for a machine that was never on.
        assert!(Stage::CodeAccepted.caption().contains("checking whether that machine is online"));
    }
}
