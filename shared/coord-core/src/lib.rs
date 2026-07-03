//! coord-core — the cross-app coordination brain.
//!
//! The scenario: the user is in a voice call on **snapclone**. Other apps should be able to
//! react without knowing about snapclone directly:
//!
//! * **unendlich** wants to pipe the call audio into the game and invite the peer to a
//!   server → it *shares* the call-audio resource.
//! * **voiceterm** notices the user is composing an email → it wants snapclone to mute the
//!   call for a moment and show a busy notifier → it *ducks* the call.
//!
//! The mechanism is uniform: apps publish an [`Activity`] (presence, over sharemd) and make
//! [`Claim`]s on shared [`Resource`]s. The [`Arbiter`] resolves the claims into a
//! [`Directive`] per app per resource — Own / Yield / Duck / Share — which each app then
//! acts on (mute, show busy, pipe audio, …). No app hard-codes another; they coordinate
//! through claims. Mic claims flow through voiced; presence + non-mic claims through sharemd.

/// What an app is currently doing — published as presence so others can react.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    VoiceCall,
    ComposingText,
    InGame,
    PlayingMedia,
    Recording,
}

/// A contended shared resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resource {
    /// The microphone (arbitrated by voiced).
    Mic,
    /// The live call/voice audio stream (can be shared or ducked).
    CallAudio,
    /// Foreground audio attention (who the user is listening to).
    Attention,
}

/// How a claim wants to hold a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Sole use; other exclusives yield.
    Exclusive,
    /// Momentarily quiet the current owner (mute a call while dictating), then release.
    Duck,
    /// Consume alongside the owner (pipe the call audio into a game).
    Share,
}

/// Standard priorities so contention is principled. Higher wins; a Duck is honoured only if
/// its priority is at least the owner's.
pub mod prio {
    pub const SYSTEM_ALERT: u8 = 240;
    pub const DICTATION: u8 = 180; // voiceterm composing
    pub const VOICE_CALL: u8 = 160; // snapclone call
    pub const GAME_VOICE: u8 = 120;
    pub const MEDIA: u8 = 80;
}

/// One app's claim on a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub app: String,
    pub resource: Resource,
    pub priority: u8,
    pub mode: Mode,
}

impl Claim {
    pub fn new(app: &str, resource: Resource, priority: u8, mode: Mode) -> Claim {
        Claim { app: app.to_string(), resource, priority, mode }
    }
}

/// What an app should do with a resource right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Directive {
    /// You hold it, act normally.
    Own,
    /// Someone outranks you — stop using it.
    Yield,
    /// Briefly quiet yourself (mute the call) for a higher-or-equal ducker; resume after.
    Duck,
    /// Use it alongside the owner (both get the audio).
    Share,
    /// You have no claim on this resource.
    Idle,
}

/// Holds current claims and resolves directives.
#[derive(Default)]
pub struct Arbiter {
    claims: Vec<Claim>,
}

impl Arbiter {
    pub fn new() -> Self {
        Arbiter::default()
    }

    /// Add or replace an app's claim on a resource.
    pub fn claim(&mut self, c: Claim) {
        self.claims.retain(|x| !(x.app == c.app && x.resource == c.resource));
        self.claims.push(c);
    }

    /// Drop one app's claim on a resource (e.g. voiceterm finished composing).
    pub fn release(&mut self, app: &str, resource: Resource) {
        self.claims.retain(|x| !(x.app == app && x.resource == resource));
    }

    /// Drop all of an app's claims (on disconnect).
    pub fn release_app(&mut self, app: &str) {
        self.claims.retain(|x| x.app != app);
    }

    /// The exclusive owner of a resource, if any (highest-priority Exclusive claim).
    pub fn owner(&self, resource: Resource) -> Option<&str> {
        self.claims
            .iter()
            .filter(|c| c.resource == resource && c.mode == Mode::Exclusive)
            .max_by_key(|c| c.priority)
            .map(|c| c.app.as_str())
    }

    /// The directive for `app` on `resource`, given all current claims.
    pub fn directive(&self, app: &str, resource: Resource) -> Directive {
        let mine = self.claims.iter().find(|c| c.app == app && c.resource == resource);
        let Some(mine) = mine else { return Directive::Idle };

        // the top exclusive claim on this resource
        let top_excl = self
            .claims
            .iter()
            .filter(|c| c.resource == resource && c.mode == Mode::Exclusive)
            .max_by_key(|c| c.priority);
        // is there a duck from someone else, honoured (priority >= owner's)?
        let owner_prio = top_excl.map(|c| c.priority).unwrap_or(0);
        let honoured_duck = self
            .claims
            .iter()
            .any(|c| c.resource == resource && c.mode == Mode::Duck && c.app != app && c.priority >= owner_prio);

        match mine.mode {
            Mode::Exclusive => {
                let is_top = top_excl.map(|c| c.app == app).unwrap_or(false);
                if !is_top {
                    Directive::Yield
                } else if honoured_duck {
                    Directive::Duck // the owner momentarily mutes for the ducker
                } else {
                    Directive::Own
                }
            }
            Mode::Duck => {
                // the ducker gets the floor iff its priority is high enough to be honoured
                if mine.priority >= owner_prio {
                    Directive::Own
                } else {
                    Directive::Yield
                }
            }
            Mode::Share => Directive::Share,
        }
    }
}

/// Presence key convention for the data bus: `presence/<app>` holds the app's activity.
pub fn presence_key(app: &str) -> String {
    format!("presence/{app}")
}

impl Resource {
    pub fn as_str(self) -> &'static str {
        match self {
            Resource::Mic => "mic",
            Resource::CallAudio => "call_audio",
            Resource::Attention => "attention",
        }
    }
    pub fn parse(s: &str) -> Option<Resource> {
        match s {
            "mic" => Some(Resource::Mic),
            "call_audio" => Some(Resource::CallAudio),
            "attention" => Some(Resource::Attention),
            _ => None,
        }
    }
    /// All resources, for iterating when recomputing directives.
    pub fn all() -> [Resource; 3] {
        [Resource::Mic, Resource::CallAudio, Resource::Attention]
    }
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Exclusive => "exclusive",
            Mode::Duck => "duck",
            Mode::Share => "share",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "exclusive" => Some(Mode::Exclusive),
            "duck" => Some(Mode::Duck),
            "share" => Some(Mode::Share),
            _ => None,
        }
    }
}

impl Directive {
    pub fn as_str(self) -> &'static str {
        match self {
            Directive::Own => "own",
            Directive::Yield => "yield",
            Directive::Duck => "duck",
            Directive::Share => "share",
            Directive::Idle => "idle",
        }
    }
}

impl Activity {
    pub fn as_str(self) -> &'static str {
        match self {
            Activity::Idle => "idle",
            Activity::VoiceCall => "voice_call",
            Activity::ComposingText => "composing_text",
            Activity::InGame => "in_game",
            Activity::PlayingMedia => "playing_media",
            Activity::Recording => "recording",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The headline scenario: snapclone call, voiceterm ducks to compose, unendlich shares.
    #[test]
    fn call_duck_and_share_scenario() {
        let mut arb = Arbiter::new();
        // snapclone is in a call: owns mic + call audio
        arb.claim(Claim::new("snapclone", Resource::CallAudio, prio::VOICE_CALL, Mode::Exclusive));
        arb.claim(Claim::new("snapclone", Resource::Mic, prio::VOICE_CALL, Mode::Exclusive));
        assert_eq!(arb.owner(Resource::CallAudio), Some("snapclone"));
        assert_eq!(arb.directive("snapclone", Resource::CallAudio), Directive::Own);

        // unendlich pipes the call audio into the game (shares)
        arb.claim(Claim::new("unendlich", Resource::CallAudio, prio::GAME_VOICE, Mode::Share));
        assert_eq!(arb.directive("unendlich", Resource::CallAudio), Directive::Share);
        // sharing doesn't disturb the owner
        assert_eq!(arb.directive("snapclone", Resource::CallAudio), Directive::Own);

        // voiceterm: user is composing → duck the call momentarily (dictation outranks call)
        arb.claim(Claim::new("voiceterm", Resource::CallAudio, prio::DICTATION, Mode::Duck));
        assert_eq!(arb.directive("snapclone", Resource::CallAudio), Directive::Duck, "snapclone mutes momentarily");
        assert_eq!(arb.directive("voiceterm", Resource::CallAudio), Directive::Own, "voiceterm gets the floor");

        // when voiceterm finishes composing, the call resumes
        arb.release("voiceterm", Resource::CallAudio);
        assert_eq!(arb.directive("snapclone", Resource::CallAudio), Directive::Own);
    }

    #[test]
    fn low_priority_duck_is_not_honoured() {
        let mut arb = Arbiter::new();
        arb.claim(Claim::new("snapclone", Resource::CallAudio, prio::VOICE_CALL, Mode::Exclusive));
        // a media player tries to duck a live call — too low, ignored
        arb.claim(Claim::new("beatspace", Resource::CallAudio, prio::MEDIA, Mode::Duck));
        assert_eq!(arb.directive("snapclone", Resource::CallAudio), Directive::Own);
        assert_eq!(arb.directive("beatspace", Resource::CallAudio), Directive::Yield);
    }

    #[test]
    fn exclusive_contention_yields_to_higher_priority() {
        let mut arb = Arbiter::new();
        arb.claim(Claim::new("snapclone", Resource::Mic, prio::VOICE_CALL, Mode::Exclusive));
        arb.claim(Claim::new("voiceterm", Resource::Mic, prio::DICTATION, Mode::Exclusive));
        assert_eq!(arb.owner(Resource::Mic), Some("voiceterm"));
        assert_eq!(arb.directive("voiceterm", Resource::Mic), Directive::Own);
        assert_eq!(arb.directive("snapclone", Resource::Mic), Directive::Yield);
    }

    #[test]
    fn presence_key_and_release_app() {
        assert_eq!(presence_key("snapclone"), "presence/snapclone");
        let mut arb = Arbiter::new();
        arb.claim(Claim::new("x", Resource::Mic, 100, Mode::Exclusive));
        arb.release_app("x");
        assert_eq!(arb.directive("x", Resource::Mic), Directive::Idle);
    }
}
