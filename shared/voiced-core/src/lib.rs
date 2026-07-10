//! voiced-core — the routing brain of the unified voice daemon.
//!
//! One daemon owns the microphone and the whisper pool (that stage is voice-kit +
//! audio-kit, lifted from voiceterm). This crate is the part above the audio: apps
//! register the intents they care about, each with a [`Scope`], and when an intent is
//! recognised the registry decides **who receives it**:
//!
//! * [`Scope::Always`] registrations receive the intent even when the app is backgrounded
//!   (wake words, "new message" alerts).
//! * [`Scope::Active`] registrations receive it only while their app is focused
//!   (most commands).
//! * **System intents** — "switch to <app>", "open <doc> in <app>" — are owned by the
//!   daemon itself and change focus / launch targets rather than going to an app's grammar.
//!
//! Apps reach the daemon through ipc-kit and, per the socket-first rule, fall back to
//! their own in-process voice code if `voiced` isn't running.

pub mod gate;

use ipc_kit::Scope;
use std::collections::HashMap;

pub mod grammar;
pub use grammar::{Grammar, Match, Matcher, Rule, Transcriber};

/// A recognised, structured intent coming out of the audio stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// An app-domain intent: an event type plus filled slots.
    App { event_type: String, slots: Vec<String> },
    /// "switch to <app>" — focus/raise an app.
    SwitchApp { app: String },
    /// "open <document> in <app>".
    OpenDoc { app: String, document: String },
}

impl Intent {
    /// Very small NL front-end for the two system intents the daemon owns. Apps supply
    /// richer grammars; this only needs to catch the universal system phrases so they work
    /// regardless of which app is focused.
    pub fn parse_system(utterance: &str) -> Option<Intent> {
        let u = utterance.trim().to_lowercase();
        if let Some(rest) = u.strip_prefix("switch to ") {
            let app = rest.trim();
            if !app.is_empty() {
                return Some(Intent::SwitchApp { app: app.to_string() });
            }
        }
        // "open <doc> in <app>"
        if let Some(rest) = u.strip_prefix("open ") {
            if let Some((doc, app)) = rest.rsplit_once(" in ") {
                let (doc, app) = (doc.trim(), app.trim());
                if !doc.is_empty() && !app.is_empty() {
                    return Some(Intent::OpenDoc { app: app.to_string(), document: doc.to_string() });
                }
            }
        }
        None
    }
}

/// What the daemon should do with an intent after routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// Deliver this event type (+ slots) to each named app.
    Deliver { app: String, event_type: String, slots: Vec<String> },
    /// Raise/focus an app (also becomes the new active app).
    Focus { app: String },
    /// Focus an app and hand it a document to open.
    Open { app: String, document: String },
    /// Nothing registered wanted it.
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reg {
    event_type: String,
    scope: Scope,
}

/// Tracks registrations per app and the currently-active app, and routes intents.
#[derive(Default)]
pub struct Registry {
    by_app: HashMap<String, Vec<Reg>>,
    active: Option<String>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    /// An app registers interest in an event type at a scope. Re-registering the same type
    /// updates its scope.
    pub fn register(&mut self, app: &str, event_type: &str, scope: Scope) {
        let regs = self.by_app.entry(app.to_string()).or_default();
        if let Some(r) = regs.iter_mut().find(|r| r.event_type == event_type) {
            r.scope = scope;
        } else {
            regs.push(Reg { event_type: event_type.to_string(), scope });
        }
    }

    pub fn unregister_app(&mut self, app: &str) {
        self.by_app.remove(app);
        if self.active.as_deref() == Some(app) {
            self.active = None;
        }
    }

    pub fn set_active(&mut self, app: &str) {
        self.active = Some(app.to_string());
    }

    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// Route an intent to a dispatch plan. System intents change focus; app intents fan out
    /// to every registration that matches the event type *and* whose scope allows delivery
    /// right now (Always always; Active only to the focused app).
    pub fn route(&mut self, intent: &Intent) -> Vec<Dispatch> {
        match intent {
            Intent::SwitchApp { app } => {
                self.set_active(app);
                vec![Dispatch::Focus { app: app.clone() }]
            }
            Intent::OpenDoc { app, document } => {
                self.set_active(app);
                vec![Dispatch::Open { app: app.clone(), document: document.clone() }]
            }
            Intent::App { event_type, slots } => {
                let active = self.active.clone();
                let mut out = Vec::new();
                // deterministic order: active app first, then others alphabetically
                let mut apps: Vec<&String> = self.by_app.keys().collect();
                apps.sort();
                if let Some(a) = &active {
                    apps.sort_by_key(|k| (*k != a, (*k).clone()));
                }
                for app in apps {
                    let regs = &self.by_app[app];
                    let wants = regs.iter().any(|r| {
                        r.event_type == *event_type
                            && (r.scope == Scope::Always || active.as_deref() == Some(app))
                    });
                    if wants {
                        out.push(Dispatch::Deliver {
                            app: app.clone(),
                            event_type: event_type.clone(),
                            slots: slots.clone(),
                        });
                    }
                }
                if out.is_empty() {
                    out.push(Dispatch::Dropped);
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_intent_parsing() {
        assert_eq!(Intent::parse_system("switch to gitmind"), Some(Intent::SwitchApp { app: "gitmind".into() }));
        assert_eq!(
            Intent::parse_system("open budget.md in logscope"),
            Some(Intent::OpenDoc { app: "logscope".into(), document: "budget.md".into() })
        );
        assert_eq!(Intent::parse_system("play the next track"), None);
    }

    #[test]
    fn active_scope_only_reaches_focused_app() {
        let mut r = Registry::new();
        r.register("logscope", "next_page", Scope::Active);
        r.register("unendlich", "next_page", Scope::Active);
        r.set_active("logscope");
        let d = r.route(&Intent::App { event_type: "next_page".into(), slots: vec![] });
        assert_eq!(d, vec![Dispatch::Deliver { app: "logscope".into(), event_type: "next_page".into(), slots: vec![] }]);
    }

    #[test]
    fn always_scope_reaches_backgrounded_app() {
        let mut r = Registry::new();
        r.register("voiceterm", "mail_incoming", Scope::Always);
        r.register("krapper", "hum_capture", Scope::Active);
        r.set_active("krapper"); // voiceterm is backgrounded
        let d = r.route(&Intent::App { event_type: "mail_incoming".into(), slots: vec!["from:alice".into()] });
        assert_eq!(
            d,
            vec![Dispatch::Deliver { app: "voiceterm".into(), event_type: "mail_incoming".into(), slots: vec!["from:alice".into()] }]
        );
    }

    #[test]
    fn switch_and_open_change_focus() {
        let mut r = Registry::new();
        assert_eq!(r.route(&Intent::SwitchApp { app: "beatspace".into() }), vec![Dispatch::Focus { app: "beatspace".into() }]);
        assert_eq!(r.active(), Some("beatspace"));
        let d = r.route(&Intent::OpenDoc { app: "logscope".into(), document: "run.log".into() });
        assert_eq!(d, vec![Dispatch::Open { app: "logscope".into(), document: "run.log".into() }]);
        assert_eq!(r.active(), Some("logscope"));
    }

    #[test]
    fn unmatched_intent_is_dropped() {
        let mut r = Registry::new();
        r.register("logscope", "next_page", Scope::Active);
        let d = r.route(&Intent::App { event_type: "unknown".into(), slots: vec![] });
        assert_eq!(d, vec![Dispatch::Dropped]);
    }
}
