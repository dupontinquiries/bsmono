//! voice_coord — the group-call coordinator, ported from
//! `src/ui/GroupVoiceCoordinator.cpp`.
//!
//! The pieces around it were already ported: [`crate::voicebar::VoiceBar`] is
//! GroupVoiceBar, [`crate::roster::Roster`] is GroupVoiceRoster, and
//! [`crate::group_voice::GroupVoice`] is the floor session. What was missing
//! is the thing that WIRES them — who starts the mic, who stops it, what a
//! floor change does to three views at once, and what happens when the screen
//! goes off mid-call. In Qt that was 290 lines of `connect()`; the behaviour
//! lives in the connections, so porting the widgets without it leaves a call
//! that cannot actually be joined.
//!
//! ## Signals become an outbox
//!
//! Qt emits `floorAssertToSend` / `voicePacketToSend` / `privateModeRequested`
//! to whoever connected. There is no signal/slot here, so the coordinator
//! appends [`Out`] and the caller drains it — the same shape `voicebar.rs`
//! already uses for its own `Event`, so the app has one pattern rather than
//! two.
//!
//! ## The rules worth stating, because they are the port
//!
//! * **`join`/`leave` are idempotent.** The Qt destructor calls `leaveCall()`
//!   unconditionally and relies on it; a second leave must not re-emit a
//!   release or stop an already-stopped mic.
//! * **The mic follows interactivity, not the call.** ActivityGate drops the
//!   mic the moment the device stops being interactive — screen off,
//!   backgrounded, battery-saver — and resumes it ONLY if a call is still up.
//!   That is privacy and battery both, and it is the rule most easily lost in
//!   a port because it lives in a lambda far from `joinCall`.
//! * **Joining while non-interactive does NOT open the mic.** The Qt guards
//!   `capture->start()` on `interactive()`; the gate opens it later. A port
//!   that starts the mic on join leaks a hot mic on a locked phone.
//! * **The roster's level is the ACTIVE speaker's.** We only have our own mic
//!   locally, so the local level is fed to the roster only while WE hold the
//!   floor. A peer talking shows the ring via the active-speaker path with no
//!   level of its own.
//! * **Talk prominence tracks the mode.** Push-to-talk is a button you hold,
//!   so it is grand; the auto mode rarely touches it, so it relaxes. Qt had
//!   three modes (PTT / VOX / continuous) and `group_voice.rs` collapses the
//!   two auto ones into `Open`, so the rule is "PushToTalk or not".
//! * **Fresh views are caught up.** `setViews` re-applies the current holder
//!   and prominence, because a dialog reopened mid-call must not show an idle
//!   bar.
//!
//! ## Availability
//!
//! Qt compiled the backend behind `SC_VOICE_CORE`/`SC_VOICE_CODEC` and ran
//! the coordinator inert when absent, specifically so the conversation dialog
//! carries no feature `#ifdef`s. [`Coordinator::new_inert`] is that state: it
//! accepts every call, changes no view, and emits nothing.

use crate::group_voice::{GroupVoice, TalkMode};
use crate::roster::Roster;
use crate::voicebar::{Security, VoiceBar};

/// Conversation security tier.
///
/// A RE-EXPORT of `contacts::Tier`, not a second definition. The first draft
/// of this module declared its own on the strength of the parity ledger
/// saying `ConversationSecurityBar` was pending — it is not, `contacts.rs`
/// has had the bar and its three tiers all along, and the ledger was wrong
/// (again). Two `Tier` enums in one crate is how a coordinator and the bar it
/// drives end up disagreeing about what Securing means.
pub use crate::contacts::Tier;

/// The voice bar's vocabulary for a tier. Qt mapped these one to one.
///
/// A free function rather than an inherent method because `Tier` belongs to
/// `contacts` and this mapping is `voicebar`'s business — putting it on the
/// type would make the contacts module depend on the voice bar to say what a
/// tier means.
pub fn tier_security(tier: Tier) -> Security {
    match tier {
        Tier::Standard => Security::Public,
        Tier::Securing => Security::Securing,
        Tier::Private => Security::Private,
    }
}

/// What the coordinator asks the transport (or the app) to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Claim the floor on the wire. Qt: `floorAssertToSend`.
    FloorAssert { holder: String, at: u64 },
    /// Release it. Qt: `floorReleaseToSend`.
    FloorRelease { holder: String },
    /// An encoded voice packet is ready to send.
    VoicePacket(Vec<u8>),
    /// The user asked for (or cancelled) a private conversation.
    PrivateModeRequested(bool),
    /// The mic could not be opened.
    CaptureError(String),
    /// Mic transitions, so the host can actually open/close the device.
    MicStart,
    MicStop,
}

/// Wires capture, the floor session and the views into a call.
#[derive(Debug, Clone)]
pub struct Coordinator {
    self_id: String,
    available: bool,
    in_call: bool,
    /// Whether the device is interactive — screen on, foregrounded, not in
    /// battery-saver. ActivityGate's single boolean.
    interactive: bool,
    /// Whether the mic is actually open, so start/stop stay idempotent.
    mic_open: bool,
    holder: String,
    tier: Tier,
    mode: TalkMode,
    out: Vec<Out>,
}

impl Coordinator {
    pub fn new(self_id: impl Into<String>) -> Coordinator {
        Coordinator {
            self_id: self_id.into(),
            available: true,
            in_call: false,
            // Qt reads ActivityGate at join; a fresh coordinator assumes the
            // app it was just constructed by is on screen.
            interactive: true,
            mic_open: false,
            holder: String::new(),
            tier: Tier::Standard,
            mode: TalkMode::PushToTalk,
            out: Vec::new(),
        }
    }

    /// The backend-not-built state: accepts every call, does nothing.
    pub fn new_inert(self_id: impl Into<String>) -> Coordinator {
        Coordinator { available: false, ..Coordinator::new(self_id) }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }
    pub fn in_call(&self) -> bool {
        self.in_call
    }
    pub fn mic_open(&self) -> bool {
        self.mic_open
    }
    pub fn tier(&self) -> Tier {
        self.tier
    }
    pub fn holder(&self) -> &str {
        &self.holder
    }
    pub fn talk_mode(&self) -> TalkMode {
        self.mode
    }

    /// Drain what the transport should act on.
    pub fn take_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    // ── mic, which only the two rules below may touch ────────────────────

    fn mic_start(&mut self) {
        if !self.available || self.mic_open {
            return;
        }
        self.mic_open = true;
        self.out.push(Out::MicStart);
    }

    fn mic_stop(&mut self) {
        if !self.mic_open {
            return;
        }
        self.mic_open = false;
        self.out.push(Out::MicStop);
    }

    // ── lifecycle ────────────────────────────────────────────────────────

    /// Join. Idempotent.
    ///
    /// Opens the mic only when the device is interactive; otherwise
    /// [`Self::set_interactive`] opens it when interactivity returns. A port
    /// that started the mic unconditionally would leave a hot mic on a locked
    /// phone.
    pub fn join(&mut self, gv: &mut GroupVoice) {
        if self.in_call {
            return;
        }
        self.in_call = true;
        if !self.available {
            return;
        }
        // Push the mode down before joining. `GroupVoice` defaults to
        // TalkMode::Open; this coordinator defaults to PushToTalk (the Qt
        // default), and a session left on Open has no floor at all -- every
        // peer may transmit and `active_speaker` is whoever is loudest.
        gv.mode = self.mode;
        gv.join();
        if self.interactive {
            self.mic_start();
        }
    }

    /// Leave. Idempotent, and the destructor's unconditional call depends on
    /// that. Releases the floor if held, and resets all three views.
    pub fn leave(&mut self, gv: &mut GroupVoice, bar: Option<&mut VoiceBar>, roster: Option<&mut Roster>) {
        if !self.in_call {
            return;
        }
        self.in_call = false;
        self.mic_stop();
        if self.available {
            if self.holder == self.self_id {
                let holder = self.self_id.clone();
                self.out.push(Out::FloorRelease { holder });
            }
            gv.leave();
        }
        if let Some(b) = bar {
            b.set_on_air(false);
            b.set_talker("", false);
        }
        if let Some(r) = roster {
            r.set_active_speaker("");
        }
        self.holder.clear();
    }

    /// ActivityGate. The mic follows the device, not the call.
    pub fn set_interactive(&mut self, interactive: bool) {
        self.interactive = interactive;
        if !interactive {
            self.mic_stop();
        } else if self.in_call {
            self.mic_start();
        }
    }

    // ── floor ────────────────────────────────────────────────────────────

    /// Push-to-talk pressed.
    pub fn press_talk(&mut self, gv: &mut GroupVoice, at: u64) {
        if !self.available || !self.in_call {
            return;
        }
        let me = self.self_id.clone();
        if gv.floor_assert(&me, at) {
            self.out.push(Out::FloorAssert { holder: me, at });
        }
    }

    pub fn release_talk(&mut self, gv: &mut GroupVoice) {
        if !self.available || !self.in_call {
            return;
        }
        let me = self.self_id.clone();
        if gv.floor_release(&me) {
            self.out.push(Out::FloorRelease { holder: me });
        }
    }

    /// A peer claimed the floor on the wire.
    pub fn peer_floor_assert(&mut self, gv: &mut GroupVoice, holder: &str, at: u64) {
        if self.available {
            gv.floor_assert(holder, at);
        }
    }

    pub fn peer_floor_release(&mut self, gv: &mut GroupVoice, holder: &str) {
        if self.available {
            gv.floor_release(holder);
        }
    }

    /// Reflect the current floor holder onto the views.
    ///
    /// Qt's `applyHolder`. Also called from `set_views` and after a
    /// participant change, so a name that arrived late still resolves.
    pub fn apply_holder(&mut self, gv: &GroupVoice, bar: Option<&mut VoiceBar>, roster: Option<&mut Roster>) {
        let holder = gv.active_speaker().unwrap_or("").to_string();
        self.holder = holder.clone();
        if let Some(r) = roster {
            r.set_active_speaker(&holder);
        }
        if let Some(b) = bar {
            if holder.is_empty() {
                b.set_talker("", false);
            } else {
                let is_self = holder == self.self_id;
                b.set_talker(&self.name_for(gv, &holder), is_self);
            }
        }
    }

    /// A participant's display name, falling back to the raw id.
    ///
    /// Qt returns the id when the name is empty OR the id is unknown — a
    /// blank talker label is worse than a hex id, because the bar then reads
    /// as "nobody is talking" while somebody is.
    pub fn name_for(&self, gv: &GroupVoice, id: &str) -> String {
        gv.participants
            .iter()
            .find(|p| p.id == id)
            .map(|p| if p.name.is_empty() { p.id.clone() } else { p.name.clone() })
            .unwrap_or_else(|| id.to_string())
    }

    // ── levels ───────────────────────────────────────────────────────────

    /// Local mic level.
    ///
    /// Always to the bar; to the roster ONLY while we hold the floor, because
    /// the roster's animated ring belongs to the ACTIVE speaker and we have
    /// no level for a remote one.
    pub fn mic_level(&self, level: f32, bar: Option<&mut VoiceBar>, roster: Option<&mut Roster>) {
        if let Some(b) = bar {
            b.set_mic_level(level);
        }
        if let Some(r) = roster {
            if self.holder == self.self_id && !self.holder.is_empty() {
                r.set_speaker_level(level);
            }
        }
    }

    pub fn capture_error(&mut self, why: impl Into<String>) {
        self.mic_open = false;
        self.out.push(Out::CaptureError(why.into()));
    }

    /// An encoded frame from the local encoder, gated on transmitting.
    pub fn voice_packet(&mut self, gv: &GroupVoice, packet: Vec<u8>) {
        if !self.available || !gv.may_transmit(&self.self_id) {
            return;
        }
        self.out.push(Out::VoicePacket(packet));
    }

    // ── modes, security, views ───────────────────────────────────────────

    /// Push-to-talk is grand (you hold it); the auto mode relaxes it.
    ///
    /// Takes `gv` because the mode belongs to the SESSION, not to this
    /// coordinator: the first version tracked `self.mode` and only resized the
    /// button, so the session stayed on its default `TalkMode::Open` where
    /// everyone may transmit and there is no floor to hold. Every floor test
    /// failed at once, which is the right way for that to show up.
    pub fn set_talk_mode(&mut self, mode: TalkMode, gv: &mut GroupVoice, bar: Option<&mut VoiceBar>) {
        self.mode = mode;
        gv.mode = mode;
        if let Some(b) = bar {
            b.set_talk_prominent(mode == TalkMode::PushToTalk);
        }
    }

    pub fn set_security_tier(&mut self, tier: Tier, bar: Option<&mut VoiceBar>) {
        self.tier = tier;
        if let Some(b) = bar {
            b.security = tier_security(tier);
        }
    }

    /// The user toggled "make this private".
    ///
    /// Goes to Securing rather than straight to Private: the tier is a claim
    /// about the transport, and the transport has not confirmed yet. Qt does
    /// the same, which is why Securing is its own tier rather than a spinner.
    pub fn request_private(&mut self, on: bool, bar: Option<&mut VoiceBar>) {
        self.set_security_tier(if on { Tier::Securing } else { Tier::Standard }, bar);
        self.out.push(Out::PrivateModeRequested(on));
    }

    /// Views were (re)attached — catch them up.
    ///
    /// A conversation dialog reopened mid-call must not show an idle bar.
    pub fn set_views(&mut self, gv: &GroupVoice, mut bar: Option<&mut VoiceBar>, roster: Option<&mut Roster>) {
        if let Some(b) = bar.as_deref_mut() {
            b.set_talk_prominent(self.mode == TalkMode::PushToTalk);
            b.security = tier_security(self.tier);
            b.set_on_air(self.in_call && self.holder == self.self_id && !self.holder.is_empty());
        }
        self.apply_holder(gv, bar, roster);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_voice::Participant;

    fn people() -> Vec<Participant> {
        vec![
            Participant { id: "me".into(), name: "Ada".into(), is_self: true },
            Participant { id: "pb".into(), name: "Bo".into(), is_self: false },
            Participant { id: "nameless".into(), name: String::new(), is_self: false },
        ]
    }

    fn setup() -> (Coordinator, GroupVoice) {
        let mut gv = GroupVoice::default();
        gv.set_participants(people());
        (Coordinator::new("me"), gv)
    }

    #[test]
    fn join_and_leave_are_idempotent() {
        let (mut c, mut gv) = setup();
        c.join(&mut gv);
        let first = c.take_out();
        assert!(first.contains(&Out::MicStart));
        c.join(&mut gv);
        assert!(c.take_out().is_empty(), "a second join must do nothing");

        c.leave(&mut gv, None, None);
        assert!(c.take_out().contains(&Out::MicStop));
        c.leave(&mut gv, None, None);
        assert!(c.take_out().is_empty(), "the destructor's second leave must be silent");
    }

    /// THE HOT-MIC RULE. Joining while the screen is off must NOT open the
    /// mic; the gate opens it when interactivity returns.
    #[test]
    fn joining_while_non_interactive_does_not_open_the_mic() {
        let (mut c, mut gv) = setup();
        c.set_interactive(false);
        c.join(&mut gv);
        assert!(!c.mic_open(), "a locked phone must not open the mic");
        assert!(!c.take_out().contains(&Out::MicStart));

        c.set_interactive(true);
        assert!(c.mic_open(), "and the gate opens it when the screen comes back");
        assert!(c.take_out().contains(&Out::MicStart));
    }

    /// The mic follows the DEVICE, not the call — and does not come back for
    /// a creature that has left.
    #[test]
    fn the_mic_drops_when_the_device_sleeps_and_returns_only_if_still_in_a_call() {
        let (mut c, mut gv) = setup();
        c.join(&mut gv);
        c.take_out();

        c.set_interactive(false);
        assert!(!c.mic_open());
        c.set_interactive(true);
        assert!(c.mic_open(), "still in the call, so it resumes");

        c.leave(&mut gv, None, None);
        c.take_out();
        c.set_interactive(false);
        c.set_interactive(true);
        assert!(!c.mic_open(), "not in a call — interactivity must not open a mic");
        assert!(c.take_out().is_empty());
    }

    #[test]
    fn leaving_resets_every_view_and_releases_a_held_floor() {
        let (mut c, mut gv) = setup();
        let mut bar = VoiceBar::default();
        let mut roster = Roster::default();
        c.join(&mut gv);
        c.press_talk(&mut gv, 100);
        c.apply_holder(&gv, Some(&mut bar), Some(&mut roster));
        assert_eq!(c.holder(), "me");
        c.take_out();

        c.leave(&mut gv, Some(&mut bar), Some(&mut roster));
        assert_eq!(c.holder(), "");
        assert!(roster.active_speaker().is_none());
        assert!(c.take_out().iter().any(|o| matches!(o, Out::FloorRelease { .. })),
            "a held floor is released on the way out");
    }

    /// The roster ring is the ACTIVE speaker's. We have no level for a remote
    /// talker, so ours must not be fed while they hold the floor.
    #[test]
    fn the_local_mic_level_reaches_the_roster_only_while_we_hold_the_floor() {
        let (mut c, mut gv) = setup();
        let mut roster = Roster::default();
        c.join(&mut gv);

        // A peer is talking: our level must not drive their ring.
        c.peer_floor_assert(&mut gv, "pb", 10);
        c.apply_holder(&gv, None, Some(&mut roster));
        c.mic_level(0.9, None, Some(&mut roster));
        roster.tick(1.0);
        assert!(roster.ring_level() < 0.5, "a peer's ring must not show our mic: {}", roster.ring_level());

        // Now we hold it.
        c.peer_floor_release(&mut gv, "pb");
        c.press_talk(&mut gv, 20);
        c.apply_holder(&gv, None, Some(&mut roster));
        c.mic_level(0.9, None, Some(&mut roster));
        roster.tick(1.0);
        assert!(roster.ring_level() > 0.5, "our own ring does: {}", roster.ring_level());
    }

    /// A blank talker label reads as "nobody is talking" while somebody is.
    #[test]
    fn a_nameless_or_unknown_participant_falls_back_to_the_id() {
        let (c, gv) = setup();
        assert_eq!(c.name_for(&gv, "pb"), "Bo");
        assert_eq!(c.name_for(&gv, "nameless"), "nameless", "empty name falls back to id");
        assert_eq!(c.name_for(&gv, "ghost"), "ghost", "unknown id is still shown");
    }

    #[test]
    fn talk_prominence_tracks_the_mode() {
        let (mut c, mut gv) = setup();
        let mut bar = VoiceBar::default();
        c.set_talk_mode(TalkMode::PushToTalk, &mut gv, Some(&mut bar));
        assert_eq!(c.talk_mode(), TalkMode::PushToTalk);
        assert_eq!(gv.mode, TalkMode::PushToTalk, "the SESSION has to hear about it");
        // Open is the auto mode here; only PTT is grand. (Qt also had VOX and
        // continuous; group_voice.rs models them as a single Open mode, so the
        // prominence rule is "PushToTalk or not" rather than a three-way.)
        c.set_talk_mode(TalkMode::Open, &mut gv, Some(&mut bar));
        assert_eq!(c.talk_mode(), TalkMode::Open);
        assert_eq!(gv.mode, TalkMode::Open);
    }

    /// THE BUG THE BUILD FOUND. The coordinator defaulted to PushToTalk and
    /// never told the session, which sat on its own default of Open -- where
    /// there is no floor, everyone may transmit, and `active_speaker` is
    /// whoever is loudest. Four floor tests failed at once.
    #[test]
    fn joining_puts_the_session_into_the_coordinators_talk_mode() {
        let (mut c, mut gv) = setup();
        assert_eq!(gv.mode, TalkMode::Open, "GroupVoice's own default");
        c.join(&mut gv);
        assert_eq!(gv.mode, TalkMode::PushToTalk, "join must push the mode down");
        // ...and with a real floor, pressing talk actually claims it.
        c.press_talk(&mut gv, 1);
        c.apply_holder(&gv, None, None);
        assert_eq!(c.holder(), "me");
    }

    /// Private goes to Securing, not straight to Private: the tier is a claim
    /// the transport has not confirmed.
    #[test]
    fn requesting_private_goes_through_securing_and_tells_the_transport() {
        let (mut c, _gv) = setup();
        let mut bar = VoiceBar::default();
        c.request_private(true, Some(&mut bar));
        assert_eq!(c.tier(), Tier::Securing, "not Private until confirmed");
        assert!(c.take_out().contains(&Out::PrivateModeRequested(true)));

        // The transport confirms.
        c.set_security_tier(Tier::Private, Some(&mut bar));
        assert_eq!(c.tier(), Tier::Private);

        c.request_private(false, Some(&mut bar));
        assert_eq!(c.tier(), Tier::Standard);
        assert!(c.take_out().contains(&Out::PrivateModeRequested(false)));
    }

    /// A dialog reopened mid-call must not show an idle bar.
    #[test]
    fn fresh_views_are_caught_up_to_the_live_call() {
        let (mut c, mut gv) = setup();
        c.join(&mut gv);
        c.press_talk(&mut gv, 5);
        c.request_private(true, None);
        c.take_out();

        let mut bar = VoiceBar::default();
        let mut roster = Roster::default();
        roster.set_participants(
            people().into_iter()
                .map(|p| crate::roster::Participant { id: p.id, name: p.name, is_self: p.is_self })
                .collect());
        c.set_views(&gv, Some(&mut bar), Some(&mut roster));
        assert_eq!(roster.active_speaker(), Some("me"), "the roster shows the live holder");
        assert_eq!(c.holder(), "me");
    }

    /// Packets must not leak out when we do not hold the floor — that is a
    /// live mic going to the group.
    #[test]
    fn voice_packets_are_gated_on_holding_the_floor() {
        let (mut c, mut gv) = setup();
        c.join(&mut gv);
        c.take_out();

        c.peer_floor_assert(&mut gv, "pb", 1);
        c.voice_packet(&gv, vec![1, 2, 3]);
        assert!(!c.take_out().iter().any(|o| matches!(o, Out::VoicePacket(_))),
            "a peer holds the floor — nothing of ours goes on the wire");

        c.peer_floor_release(&mut gv, "pb");
        c.press_talk(&mut gv, 2);
        c.voice_packet(&gv, vec![1, 2, 3]);
        assert!(c.take_out().iter().any(|o| matches!(o, Out::VoicePacket(_))));
    }

    /// A capture failure must not leave the coordinator believing the mic is
    /// open, or the gate will never retry it.
    #[test]
    fn a_capture_error_closes_the_mic_state() {
        let (mut c, mut gv) = setup();
        c.join(&mut gv);
        c.take_out();
        assert!(c.mic_open());
        c.capture_error("no such device");
        assert!(!c.mic_open());
        assert!(c.take_out().iter().any(|o| matches!(o, Out::CaptureError(_))));
        // ...and the gate can bring it back.
        c.set_interactive(false);
        c.set_interactive(true);
        assert!(c.mic_open());
    }

    /// The backend-not-built state: every call accepted, nothing done. Qt ran
    /// inert so the dialog carries no feature #ifdefs.
    #[test]
    fn an_inert_coordinator_accepts_everything_and_emits_nothing() {
        let mut gv = GroupVoice::default();
        gv.set_participants(people());
        let mut c = Coordinator::new_inert("me");
        assert!(!c.is_available());
        c.join(&mut gv);
        c.press_talk(&mut gv, 1);
        c.voice_packet(&gv, vec![1]);
        c.set_interactive(false);
        c.set_interactive(true);
        c.leave(&mut gv, None, None);
        assert!(!c.mic_open(), "an inert coordinator never opens a mic");
        assert!(c.take_out().is_empty(), "and never asks the transport for anything");
    }
}
