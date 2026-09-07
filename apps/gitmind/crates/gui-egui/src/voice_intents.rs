//! voice_intents — what gitmind listens for, and who it tells.
//!
//! `voiced-core` is the routing brain: apps register the intents they care about with a
//! [`Scope`], the daemon owns the microphone, and a recognised intent is dispatched to
//! whoever registered it. Two apps used it — `voiced` itself and voiceterm's QML build. No
//! egui app registered anything, so the daemon had a routing table with nothing in it for
//! the surfaces people actually run.
//!
//! gitmind is a good case for voice, and specifically for `Scope::Active`: its work is
//! two-handed (a phone in one hand, a device in the other) and its commands are short
//! sentences that name a thing — "restart voiced on the phone", "status", "identify the
//! plug". Tapping through a fleet page to restart one daemon is the interaction voice is
//! for.
//!
//! ## What is here and what is not
//!
//! Here: the grammar (phrases → event types with slots), the registration list, and the
//! mapping from a recognised intent back to the same action a button runs. All pure.
//!
//! Not here: audio. The daemon owns the microphone; this crate never opens one. That
//! division is the reason a grammar can be tested at all — every case below is a sentence
//! in and a command out, with no model and no sound card.
//!
//! ## Scope, and why almost everything is Active
//!
//! `Scope::Always` means an intent reaches this app even when it is backgrounded. That is
//! right for a wake phrase and wrong for everything else: "restart it" spoken at another
//! app must not restart a daemon behind your back. Only the explicit, unambiguous
//! **"gitmind status"** is Always — it names the app in the sentence, which is the same bar
//! the daemon's own "switch to <app>" clears.

use voiced_core::{Matcher, Scope};

/// The event types gitmind answers to. Strings because that is what crosses the daemon
/// boundary; the enum below is what the app actually switches on.
pub mod event {
    pub const DAEMON_CONTROL: &str = "gitmind.daemon.control";
    pub const DAEMON_STATUS: &str = "gitmind.daemon.status";
    pub const IDENTIFY: &str = "gitmind.device.identify";
    pub const OPEN_PAGE: &str = "gitmind.page.open";
}

/// What a recognised utterance asks gitmind to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceAction {
    /// start / stop / restart a named daemon.
    Daemon { action: String, name: String },
    /// Read the daemon states on the current target.
    Status,
    /// Ask the accessory or device to identify itself.
    Identify,
    /// Switch to a page by name.
    OpenPage(String),
}

/// A `Matcher` carrying gitmind's rules, active. This is what turns a transcript into an
/// event type and slots — and it runs voice-kit's hallucination filter on the way in, which
/// is why the app does not need its own.
pub fn matcher(app: &str) -> Matcher {
    let mut m = Matcher::new();
    for (event_type, phrase) in rules() {
        let toks: Vec<String> = phrase.split_whitespace().map(str::to_string).collect();
        m.register_rule(app, event_type, &toks);
    }
    m.set_active(app);
    m
}

/// The phrase table. Kept separate from [`grammar`] so the registration list and the tests
/// read the same source rather than two copies that drift.
pub fn rules() -> Vec<(&'static str, &'static str)> {
    vec![
        // The daemon verbs. The daemon NAME is the slot, so this is three rules and not
        // three-times-the-daemons.
        (event::DAEMON_CONTROL, "restart {name}"),
        (event::DAEMON_CONTROL, "start {name}"),
        (event::DAEMON_CONTROL, "stop {name}"),
        // Status, with and without the app named. The named form is what makes an
        // `Always` registration defensible.
        (event::DAEMON_STATUS, "gitmind status"),
        (event::DAEMON_STATUS, "daemon status"),
        (event::DAEMON_STATUS, "check the daemons"),
        (event::IDENTIFY, "identify"),
        (event::IDENTIFY, "which one is it"),
        (event::OPEN_PAGE, "open {page}"),
        (event::OPEN_PAGE, "go to {page}"),
    ]
}

/// What to register with the daemon: `(event_type, scope)`, deduplicated.
///
/// Only the app-named status phrase is `Always`. See the module docs: an intent that can
/// change a device's state must not fire while another app is focused.
pub fn registrations() -> Vec<(&'static str, Scope)> {
    vec![
        (event::DAEMON_CONTROL, Scope::Active),
        (event::DAEMON_STATUS, Scope::Always),
        (event::IDENTIFY, Scope::Active),
        (event::OPEN_PAGE, Scope::Active),
    ]
}

/// Register every gitmind intent on a `voiced_core::Registry`.
pub fn register(reg: &mut voiced_core::Registry, app: &str) {
    for (event_type, scope) in registrations() {
        reg.register(app, event_type, scope);
    }
}

/// Turn a recognised `(event_type, slots)` into the action to run.
///
/// `None` for an event gitmind did not register, or a slot that names nothing it has — a
/// spoken "restart the dishwasher" is not an error to report, it is a sentence for someone
/// else's grammar.
pub fn action_for(event_type: &str, slots: &[String], verb_hint: &str) -> Option<VoiceAction> {
    match event_type {
        event::DAEMON_STATUS => Some(VoiceAction::Status),
        event::IDENTIFY => Some(VoiceAction::Identify),
        event::OPEN_PAGE => {
            let page = slots.first()?.trim();
            (!page.is_empty()).then(|| VoiceAction::OpenPage(page.to_string()))
        }
        event::DAEMON_CONTROL => {
            let spoken = slots.first()?.trim().to_ascii_lowercase();
            let name = resolve_daemon(&spoken)?;
            let action = match verb_hint.trim().to_ascii_lowercase().as_str() {
                "start" => "start",
                "stop" => "stop",
                _ => "restart",
            };
            Some(VoiceAction::Daemon { action: action.to_string(), name: name.to_string() })
        }
        _ => None,
    }
}

/// A spoken daemon name to a real one.
///
/// Speech does not produce `sharemd`; it produces "share md", "share dee", "the share
/// daemon". The match is on the name with spaces removed and a few spoken suffixes
/// stripped, against the registry gitmind already has — so a daemon added to
/// `cockpit::daemons` is speakable the day it lands, with nothing to update here.
pub fn resolve_daemon(spoken: &str) -> Option<&'static str> {
    let s: String = spoken
        .to_ascii_lowercase()
        .replace(" the ", " ")
        .replace(['-', '_'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("");
    let s = s.trim_start_matches("the");
    let s = s.trim_end_matches("daemon").trim_end_matches("service");
    if s.is_empty() {
        return None;
    }
    let all = cockpit::daemons::all();
    // exact, then prefix — "voice" reaches voiced, and "share" reaches sharemd, but a
    // prefix that matches two daemons resolves to neither rather than guessing.
    if let Some(d) = all.iter().find(|d| d.name == s) {
        return Some(d.name);
    }
    let mut hits = all.iter().filter(|d| d.name.starts_with(s) || s.starts_with(d.name));
    let first = hits.next()?;
    hits.next().is_none().then_some(first.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(phrase: &str) -> Option<VoiceAction> {
        let m = matcher("gitmind");
        match m.match_utterance(phrase) {
            voiced_core::Match::App { event_type, slots } => {
                let verb = phrase.split_whitespace().next().unwrap_or("");
                action_for(&event_type, &slots, verb)
            }
            _ => None,
        }
    }

    #[test]
    fn the_daemon_verbs_reach_the_right_daemon() {
        assert_eq!(
            heard("restart voiced"),
            Some(VoiceAction::Daemon { action: "restart".into(), name: "voiced".into() })
        );
        assert_eq!(
            heard("stop sharemd"),
            Some(VoiceAction::Daemon { action: "stop".into(), name: "sharemd".into() })
        );
        assert_eq!(
            heard("start settingsd"),
            Some(VoiceAction::Daemon { action: "start".into(), name: "settingsd".into() })
        );
    }

    /// Speech does not produce `sharemd`. It produces "share md" and "the share daemon".
    #[test]
    fn a_spoken_daemon_name_is_resolved_to_a_real_one() {
        assert_eq!(resolve_daemon("share md"), Some("sharemd"));
        assert_eq!(resolve_daemon("the share daemon"), Some("sharemd"));
        assert_eq!(resolve_daemon("voiced"), Some("voiced"));
        assert_eq!(resolve_daemon("voice"), Some("voiced"), "a prefix reaches its daemon");
        // nothing it has: not an error, just not ours
        assert_eq!(resolve_daemon("the dishwasher"), None);
        assert_eq!(resolve_daemon(""), None);
        assert_eq!(resolve_daemon("daemon"), None, "a bare suffix names nothing");
        assert_eq!(heard("restart the dishwasher"), None);
    }

    #[test]
    fn status_and_identify_need_no_slots() {
        assert_eq!(heard("gitmind status"), Some(VoiceAction::Status));
        assert_eq!(heard("daemon status"), Some(VoiceAction::Status));
        assert_eq!(heard("check the daemons"), Some(VoiceAction::Status));
        assert_eq!(heard("identify"), Some(VoiceAction::Identify));
        assert_eq!(heard("which one is it"), Some(VoiceAction::Identify));
    }

    #[test]
    fn a_page_is_opened_by_name() {
        assert_eq!(heard("open network"), Some(VoiceAction::OpenPage("network".into())));
        assert_eq!(heard("go to fleet"), Some(VoiceAction::OpenPage("fleet".into())));
        // an empty slot is not a page
        assert_eq!(action_for(event::OPEN_PAGE, &[String::new()], "open"), None);
    }

    /// The scope rule is the one with a safety argument behind it: an intent that changes a
    /// device's state must not fire while another app is focused.
    #[test]
    fn only_the_app_named_status_is_always() {
        for (e, scope) in registrations() {
            let always = scope == Scope::Always;
            assert_eq!(
                always,
                e == event::DAEMON_STATUS,
                "{e} has the wrong scope — restarting a daemon from another app's focus is not ok"
            );
        }
        // and the registrations cover every event the grammar can emit
        let emitted: std::collections::HashSet<&str> = rules().iter().map(|(e, _)| *e).collect();
        let registered: std::collections::HashSet<&str> = registrations().iter().map(|(e, _)| *e).collect();
        assert_eq!(emitted, registered, "a rule can fire an event nothing is registered for");
    }

    #[test]
    fn the_registry_routes_gitmind_intents_to_gitmind() {
        let mut reg = voiced_core::Registry::new();
        register(&mut reg, "gitmind");
        reg.set_active("gitmind");
        let intent = voiced_core::Intent::App {
            event_type: event::DAEMON_CONTROL.to_string(),
            slots: vec!["voiced".to_string()],
        };
        let d = reg.route(&intent);
        assert_eq!(d.len(), 1, "{d:?}");
        // backgrounded: the Active intent is not delivered, the Always one is
        reg.set_active("something-else");
        assert!(reg.route(&intent).is_empty(), "a control intent fired while gitmind was not focused");
        let status = voiced_core::Intent::App {
            event_type: event::DAEMON_STATUS.to_string(),
            slots: vec![],
        };
        assert_eq!(reg.route(&status).len(), 1, "the app-named status should still arrive");
    }
}
