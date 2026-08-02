//! mic_source — get the microphone through the broker, not around it.
//!
//! `dictation.rs` modelled the transcript state machine and had **no audio
//! source at all**: `start()` set a flag. `shared/mic-kit` is the broker that
//! was written for this — profile-gated, with frames sealed to the requesting
//! profile — and its description names bo-suite, logscope and voiceterm as
//! consumers. No port had ever depended on it.
//!
//! ## Why a keyboard needs the broker rather than the device
//!
//! A keyboard is not the only thing that wants the mic. On a phone-shaped
//! device the assistant, a call and dictation all want it, and a keyboard that
//! opens the device directly either fails silently when something else holds it
//! or takes it away mid-call. The broker answers **who is allowed** and **who
//! else is listening**, and both are things the user needs to see.
//!
//! ## The refusal is the feature
//!
//! `Denied::NotPermitted` and `Denied::Unavailable` are different problems with
//! different fixes: one is a settings change and the other is plugging in a
//! microphone. Collapsing them to "dictation failed" leaves the user with no
//! idea which — the single most common way a permissions UI wastes someone's
//! afternoon.

pub use mic_kit::{Denied, MicBroker, MicGrant, MicId, MicPolicy, ProfileId};

/// What the keyboard shows about its microphone.
#[derive(Debug, Clone, PartialEq)]
pub enum MicState {
    /// Not asked for yet.
    Idle,
    /// Held, with anyone else currently sharing it.
    Live { mic: String, also: Vec<String> },
    /// Refused, with the reason kept distinct.
    Refused(Denied),
}

impl MicState {
    /// One line for the keyboard's status row. Says what to DO, not what
    /// happened: "allow it in settings" is actionable, "not permitted" is not.
    pub fn label(&self) -> String {
        match self {
            MicState::Idle => "mic idle".into(),
            MicState::Live { mic, also } if also.is_empty() => format!("listening on {mic}"),
            MicState::Live { mic, also } =>
                format!("listening on {mic} — also {}", also.join(", ")),
            MicState::Refused(Denied::NotPermitted) =>
                "this profile may not use the microphone — allow it in settings".into(),
            MicState::Refused(Denied::Unavailable) =>
                "no microphone connected".into(),
        }
    }

    pub fn is_live(&self) -> bool { matches!(self, MicState::Live { .. }) }

    /// Is this worth a retry button? An absent microphone may appear; a
    /// permission will not fix itself, so offering retry there is a button that
    /// can only disappoint.
    pub fn retryable(&self) -> bool { matches!(self, MicState::Refused(Denied::Unavailable)) }
}

/// Ask the broker for the mic and describe the outcome.
///
/// The grant is returned alongside the state so the caller can release it —
/// a grant dropped without releasing leaves the broker believing the keyboard
/// is still listening, and every other profile then sees a phantom sharer.
pub fn acquire(broker: &mut MicBroker, profile: &ProfileId, mic: &MicId,
               secret: &[u8]) -> (MicState, Option<MicGrant>) {
    match broker.request(profile, mic, secret) {
        Ok(grant) => {
            let also: Vec<String> = broker.sharers(mic).into_iter()
                .filter(|p| p != profile)
                .map(|p| p.0)
                .collect();
            (MicState::Live { mic: mic.0.clone(), also }, Some(grant))
        }
        Err(d) => (MicState::Refused(d), None),
    }
}

/// Hand the mic back. Idempotent, because a keyboard can be dismissed twice —
/// once by the user and once by the host — and a double release must not panic
/// or resurrect a grant.
pub fn release(broker: &mut MicBroker, grant: Option<MicGrant>) {
    if let Some(g) = grant { broker.release(&g); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(permit: bool, present: bool) -> (MicBroker, ProfileId, MicId) {
        let profile = ProfileId("board".into());
        let mic = MicId("mic0".into());
        let mut policy = MicPolicy::new();
        if permit { policy.allow(mic.clone(), profile.clone()); }
        let mut b = MicBroker::new(policy);
        if present { b.set_present([mic.clone()]); }
        (b, profile, mic)
    }

    #[test]
    fn the_two_refusals_stay_distinct_because_the_fixes_differ() {
        // One is a settings change and the other is plugging something in.
        // Collapsing them to "dictation failed" is the single most common way a
        // permissions UI wastes an afternoon.
        let (mut b, p, m) = setup(false, true);
        let (s, g) = acquire(&mut b, &p, &m, b"secret");
        assert_eq!(s, MicState::Refused(Denied::NotPermitted));
        assert!(g.is_none());
        assert!(s.label().contains("settings"), "{}", s.label());
        assert!(!s.retryable(), "a permission will not fix itself");

        let (mut b2, p2, m2) = setup(true, false);
        let (s2, _) = acquire(&mut b2, &p2, &m2, b"secret");
        assert_eq!(s2, MicState::Refused(Denied::Unavailable));
        assert!(s2.label().contains("no microphone"));
        assert!(s2.retryable(), "a mic can be plugged in");
    }

    #[test]
    fn a_granted_mic_reports_who_else_is_listening() {
        // On a phone the assistant, a call and dictation all want it, and the
        // user needs to see when they are sharing.
        let (mut b, p, m) = setup(true, true);
        let other = ProfileId("assistant".into());
        let mut policy = MicPolicy::new();
        policy.allow(m.clone(), p.clone());
        policy.allow(m.clone(), other.clone());
        b = MicBroker::new(policy);
        b.set_present([m.clone()]);
        let _first = b.request(&other, &m, b"s").expect("assistant gets it");
        let (s, g) = acquire(&mut b, &p, &m, b"s");
        assert!(g.is_some());
        match s {
            MicState::Live { ref also, .. } => {
                assert_eq!(also, &vec!["assistant".to_string()], "and not ourselves");
            }
            other => panic!("expected Live, got {other:?}"),
        }
        assert!(s.label().contains("also assistant"));
    }

    #[test]
    fn a_sole_holder_is_not_told_it_is_sharing() {
        // Listing yourself as a sharer would imply something else is listening.
        let (mut b, p, m) = setup(true, true);
        let (s, _) = acquire(&mut b, &p, &m, b"s");
        assert_eq!(s, MicState::Live { mic: "mic0".into(), also: vec![] });
        assert_eq!(s.label(), "listening on mic0");
        assert!(s.is_live());
    }

    #[test]
    fn releasing_twice_is_harmless() {
        // A keyboard can be dismissed by the user and by the host, and a grant
        // dropped without releasing leaves the broker believing we are still
        // listening — every other profile then sees a phantom sharer.
        let (mut b, p, m) = setup(true, true);
        let (_, g) = acquire(&mut b, &p, &m, b"s");
        let g2 = g.clone();
        release(&mut b, g);
        assert!(b.sharers(&m).is_empty());
        release(&mut b, g2);
        assert!(b.sharers(&m).is_empty(), "the second release changes nothing");
        release(&mut b, None);
    }

    #[test]
    fn idle_is_a_state_rather_than_a_failure() {
        // Not having asked yet must not read as having been refused.
        let s = MicState::Idle;
        assert!(!s.is_live());
        assert!(!s.retryable());
        assert_eq!(s.label(), "mic idle");
    }
}
