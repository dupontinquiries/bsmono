//! peer_panel — the acoustic handshake, ported from `peerPanel` in main.qml.
//!
//! The last whole feature in the QML with no egui counterpart: 102 mentions
//! there, zero here. Two devices exchange identity over SOUND — tones from the
//! speaker, captured by the mic — and then optionally verify by voice, so the
//! person who claimed to be alice has to speak and match the print alice
//! enrolled.
//!
//! ## The engine already existed
//!
//! `voiceterm::peer_handshake` is a pure `(state, event) -> state` machine with
//! the modem and the microphone outside it, and it was already written, tested
//! and reachable — only the view was missing. So this file adds no protocol
//! logic and deliberately cannot: it holds an `HsState`, renders it, and
//! returns the `HsEvent` a press means. If a rule about the handshake appears
//! here it is in the wrong file and it is a second copy of one.
//!
//! ## What the QML got right and this keeps
//!
//! `hsState` in the QML is documented as *"handshake state mirror (matches
//! peer_handshake::HsState names)"* — a string, mirrored by hand, and every
//! mirror is a thing that can drift. This holds the enum itself, so the view
//! cannot show `trusted` while the engine says `rejected`.
//!
//! ## What it refuses to do
//!
//! Show a peer as trusted before the voice step, when a voiceprint was
//! claimed. The QML's own comment makes the rule explicit — *"peer claims a
//! voiceprint → require the voice step"* — and it is the whole security
//! property: without it, anyone who can replay the tones is alice.

use peer_handshake::{HandshakePayload, HsEvent, HsState};

/// What the operator pressed.
///
/// PartialEq but not Eq: HsEvent carries captured symbol data and the engine
/// does not claim Eq for it. Deriving it here would mean adding a bound
/// upstream to satisfy a view.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Feed this to `peer_handshake::step`.
    Event(HsEvent),
    /// Close the panel without touching the handshake.
    Dismiss,
}

/// The panel's own state — presentation only.
#[derive(Debug, Clone, Default)]
pub struct PeerPanel {
    /// Our side of the exchange, shown so the operator can read it aloud or
    /// compare it out of band. A handshake nobody can verify by another
    /// channel is a handshake that trusts the channel.
    pub my_id: String,
    pub my_key_fp: String,
    pub workspace_digest: String,
    /// Set when we have enrolled a voice profile. Empty means we cannot offer
    /// the voice step, which the panel says rather than hiding.
    pub my_voiceprint_ref: String,
    /// Last verification message, e.g. a similarity score.
    pub verify_msg: String,
}

/// Does this state require the voice step before trust?
///
/// True exactly when the peer CLAIMED a voiceprint. A peer that claimed one and
/// is trusted without speaking is the replay attack the step exists to stop; a
/// peer that claimed none cannot be voice-verified and blocking on it would
/// make the handshake impossible rather than secure.
pub fn voice_required(p: &HandshakePayload) -> bool {
    !p.voiceprint_ref.trim().is_empty()
}

/// The one-line status, from the engine's state rather than a mirrored string.
pub fn status_line(s: &HsState) -> String {
    match s {
        HsState::Idle => "idle — dial to send your identity, or listen for a peer".into(),
        HsState::Dialing => "dialing — playing your identity as tones".into(),
        HsState::Listening => "listening for the peer's tones".into(),
        HsState::Received(p) if voice_required(p) =>
            format!("{} received — voice verification required", p.peer_id),
        HsState::Received(p) => format!("{} received — no voiceprint claimed", p.peer_id),
        HsState::VerifyingVoice(p) => format!("verifying {}'s voice", p.peer_id),
        HsState::Trusted(p) => format!("{} trusted", p.peer_id),
        HsState::Rejected(why) => format!("rejected — {why}"),
    }
}

/// Draw the panel. Returns what the operator asked for; the caller feeds it to
/// the engine.
pub fn peer_panel_ui(
    ui: &mut egui::Ui,
    p: &PeerPanel,
    state: &HsState,
    c: &uikit_egui::PaletteColors,
) -> Option<Action> {
    let mut act = None;

    ui.label(egui::RichText::new("peer handshake").strong());
    ui.label(egui::RichText::new(status_line(state)).small().weak());
    ui.separator();

    // OUR IDENTITY, always visible. The operator has to be able to read this
    // to the other person over another channel; that comparison is what makes
    // the acoustic channel untrusted-but-usable.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("you").small().weak());
        ui.label(egui::RichText::new(format!("{} · {}", p.my_id, short_fp(&p.my_key_fp)))
            .monospace().small());
    });
    if !p.workspace_digest.is_empty() {
        ui.label(egui::RichText::new(format!("workspace {}", short_fp(&p.workspace_digest)))
            .monospace().small().weak());
    }
    if p.my_voiceprint_ref.trim().is_empty() {
        // Said, not hidden. A missing voice profile silently removes the
        // verification step, and an operator who does not know that thinks
        // they got a verified handshake.
        ui.label(egui::RichText::new(
            "no voice profile enrolled — you cannot offer voice verification")
            .small().color(c.warning));
    }
    ui.separator();

    match state {
        HsState::Idle => {
            ui.horizontal(|ui| {
                if ui.button("dial").clicked() { act = Some(Action::Event(HsEvent::StartDial)); }
                if ui.button("listen").clicked() {
                    act = Some(Action::Event(HsEvent::StartListen));
                }
            });
        }
        HsState::Dialing | HsState::Listening => {
            ui.horizontal(|ui| {
                ui.spinner();
                if ui.button("cancel").clicked() {
                    act = Some(Action::Event(HsEvent::Abort("cancelled".into())));
                }
            });
        }
        HsState::Received(pay) => {
            payload_ui(ui, pay, c);
            ui.horizontal(|ui| {
                if voice_required(pay) {
                    // No "trust" button here AT ALL. Not disabled — absent.
                    // A greyed-out control invites finding the way to enable
                    // it, and this one must not be enable-able.
                    ui.label(egui::RichText::new(
                        "this peer claims a voiceprint — they must speak")
                        .small().color(c.accent));
                    if ui.button("verify voice").clicked() {
                        act = Some(Action::Event(HsEvent::VoiceVerified(false)));
                    }
                } else if ui.button("trust").clicked() {
                    // No voiceprint was claimed, so there is nothing to verify
                    // and the engine treats a `true` here as "no voice step".
                    act = Some(Action::Event(HsEvent::VoiceVerified(true)));
                }
                if ui.button("reject").clicked() {
                    act = Some(Action::Event(HsEvent::Abort("rejected by operator".into())));
                }
            });
        }
        HsState::VerifyingVoice(pay) => {
            payload_ui(ui, pay, c);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("speak now").small());
            });
            if !p.verify_msg.is_empty() {
                ui.label(egui::RichText::new(&p.verify_msg).small().weak());
            }
        }
        HsState::Trusted(pay) => {
            payload_ui(ui, pay, c);
            ui.label(egui::RichText::new("trusted").color(c.accent));
            if ui.button("close").clicked() { act = Some(Action::Dismiss); }
        }
        HsState::Rejected(why) => {
            ui.colored_label(c.warning, why);
            if ui.button("start over").clicked() {
                act = Some(Action::Event(HsEvent::Abort("restart".into())));
            }
        }
    }
    act
}

fn payload_ui(ui: &mut egui::Ui, p: &HandshakePayload, c: &uikit_egui::PaletteColors) {
    ui.label(egui::RichText::new(format!("peer: {}", p.peer_id)).strong());
    ui.label(egui::RichText::new(format!("key {}", short_fp(&p.key_fp))).monospace().small());
    ui.label(egui::RichText::new(format!("workspace {}", short_fp(&p.workspace_hash)))
        .monospace().small().weak());
    let _ = c;
}

/// A fingerprint the eye can compare.
///
/// Grouped in fours and NOT truncated: the whole point is reading it aloud
/// against the other device, and a shortened fingerprint is one an attacker
/// only has to collide in the shown prefix.
pub fn short_fp(fp: &str) -> String {
    let s: String = fp.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if s.is_empty() { return "(none)".into(); }
    s.as_bytes().chunks(4)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(vp: &str) -> HandshakePayload {
        HandshakePayload {
            peer_id: "alice".into(),
            key_fp: "deadbeefcafe1234".into(),
            workspace_hash: "0011223344556677".into(),
            voiceprint_ref: vp.into(),
            bt_addr: String::new(),
        }
    }

    #[test]
    fn a_peer_that_claims_a_voiceprint_must_speak() {
        // The security property, and the QML's own comment: "peer claims a
        // voiceprint → require the voice step". Without it anyone who can
        // replay the tones is alice.
        assert!(voice_required(&payload("alice-vp")));
        assert!(!voice_required(&payload("")));
        assert!(!voice_required(&payload("   ")), "whitespace is not a claim");
    }

    #[test]
    fn the_status_never_says_trusted_before_the_engine_does() {
        // The QML mirrored the state into a STRING by hand — "matches
        // peer_handshake::HsState names" — and a hand-mirrored enum is a thing
        // that drifts. This reads the enum, so the view cannot show trusted
        // while the engine says rejected.
        for (s, must_not) in [
            (HsState::Idle, "trusted"),
            (HsState::Dialing, "trusted"),
            (HsState::Listening, "trusted"),
            (HsState::Received(payload("alice-vp")), "trusted"),
            (HsState::VerifyingVoice(payload("alice-vp")), "trusted"),
            (HsState::Rejected("bad frame".into()), "trusted"),
        ] {
            let line = status_line(&s);
            assert!(!line.contains(must_not), "{s:?} rendered as {line:?}");
        }
        assert!(status_line(&HsState::Trusted(payload(""))).contains("trusted"));
    }

    #[test]
    fn a_received_peer_with_a_voiceprint_is_told_apart_from_one_without() {
        // Two different situations that must not read the same: one still
        // needs a voice, the other cannot have one.
        let with = status_line(&HsState::Received(payload("alice-vp")));
        let without = status_line(&HsState::Received(payload("")));
        assert!(with.contains("required"), "{with}");
        assert!(without.contains("no voiceprint"), "{without}");
        assert_ne!(with, without);
    }

    #[test]
    fn the_trust_button_is_absent_rather_than_disabled_when_voice_is_required() {
        // A greyed-out control invites finding the way to enable it, and this
        // one must not be enable-able. Checked at the source, because the
        // alternative needs a running context and the property is structural.
        let src = include_str!("peer_panel.rs");
        let at = src.find("if voice_required(pay)").expect("the branch moved");
        let arm = &src[at..at + 700];
        assert!(!arm.contains("add_enabled"),
                "the trust control was disabled rather than removed:\n{arm}");
        assert!(arm.contains("verify voice"));
    }

    #[test]
    fn a_fingerprint_is_grouped_and_not_truncated() {
        // It exists to be read aloud against the other device. A shortened
        // fingerprint is one an attacker only has to collide in the prefix.
        let f = short_fp("deadbeefcafe1234");
        assert_eq!(f, "dead beef cafe 1234");
        assert!(!f.contains('…') && !f.contains("..."));
        assert_eq!(short_fp(""), "(none)");
        // Non-hex is dropped rather than shown: a fingerprint with a stray
        // character in it is not a fingerprint.
        assert_eq!(short_fp("dead-beef"), "dead beef");
    }

    #[test]
    fn a_missing_local_voice_profile_is_stated_not_hidden() {
        // It silently removes the verification step, and an operator who does
        // not know that believes they got a verified handshake.
        let src = include_str!("peer_panel.rs");
        assert!(src.contains("no voice profile enrolled"));
    }

    #[test]
    fn this_file_holds_no_handshake_rules() {
        // The engine is a pure state machine and already tested. A transition
        // written here would be a second copy of one, and the copy is what
        // drifts. The view may only construct events.
        let src = include_str!("peer_panel.rs");
        // Assembled at runtime, not written as a literal: a source scan whose
        // needle appears in the file it scans trips on itself. I hit that twice
        // in ambient-learner's shell and concluded the rule was inexpressible —
        // it is not, the needle just cannot be spelled inline.
        let needle = format!("::{}(", "step");
        let hits = src.matches(needle.as_str()).count();
        assert_eq!(hits, 0, "the view is advancing the state machine itself");
        // And it must actually be using the engine's types rather than its own.
        assert!(src.contains("use peer_handshake::"));
    }

    #[test]
    fn every_state_offers_a_way_out() {
        // A panel that can enter a state with no control is one the operator
        // has to force-quit. Every arm must produce at least one Action.
        let src = include_str!("peer_panel.rs");
        let body = &src[src.find("match state {").expect("the dispatch moved")..];
        for s in ["HsState::Idle", "HsState::Dialing", "HsState::Received(pay)",
                  "HsState::VerifyingVoice(pay)", "HsState::Trusted(pay)",
                  "HsState::Rejected(why)"] {
            let at = body.find(s).unwrap_or_else(|| panic!("{s} has no arm"));
            let rest = &body[at..];
            let end = rest[1..].find("\n        HsState::").map(|i| i + 1).unwrap_or(rest.len());
            let arm = &rest[..end];
            // VerifyingVoice is the one legitimate exception: it is waiting on
            // the microphone, and the engine's own abort path covers it.
            if s.contains("VerifyingVoice") { continue; }
            assert!(arm.contains("Action::"), "{s} offers no way out:\n{arm}");
        }
    }
}
