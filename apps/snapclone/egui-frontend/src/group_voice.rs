//! group_voice — the voice channel roster and floor control.
//!
//! "Group" in snapclone means group VOICE, not group text: `GroupVoiceBar`,
//! `GroupVoiceRoster`, `GroupVoiceCoordinator`. The port had one mention of
//! "voice" and none of "group", so both of the asked-about features were
//! genuinely absent.
//!
//! The roster is the easy half. The interesting half is **floor control**, from
//! `GroupVoiceCoordinator`: `onPeerFloorAssert(holder, claimTs)` /
//! `onPeerFloorRelease(holder)`.
//!
//! ## Why the timestamp is in the message
//!
//! In push-to-talk over a peer network there is no server to arbitrate, so two
//! people pressing talk at the same moment both believe they hold the floor and
//! both start transmitting. The claim carries the time it was made, and every
//! peer independently picks the EARLIEST claim — so all of them reach the same
//! answer without asking anyone. Ties break on id, because two claims can carry
//! the same millisecond and a coin flip would have peers disagreeing.
//!
//! That is the same reasoning as studio's Lamport stamps, and for the same
//! reason: convergence without coordination.

/// Someone in the channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub is_self: bool,
}

/// How talking is arbitrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TalkMode {
    /// Everyone open; the roster shows who is loudest.
    #[default]
    Open,
    /// One speaker at a time, claimed and released.
    PushToTalk,
}

/// Who holds the floor, and since when.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub holder: String,
    /// Milliseconds; the value peers compare.
    pub at: u64,
}

/// The channel.
#[derive(Debug, Clone, Default)]
pub struct GroupVoice {
    pub participants: Vec<Participant>,
    pub mode: TalkMode,
    /// Current floor holder in push-to-talk.
    pub floor: Option<Claim>,
    /// Speaker level 0..1 per participant, for the roster's pulse.
    pub levels: std::collections::BTreeMap<String, f32>,
    pub joined: bool,
    pub muted: bool,
}

impl GroupVoice {
    /// `setParticipants` — order preserved, self can be anywhere.
    pub fn set_participants(&mut self, people: Vec<Participant>) {
        // A floor held by someone who has left must be released, or the channel
        // is silently locked until they return.
        if let Some(c) = &self.floor {
            if !people.iter().any(|p| p.id == c.holder) { self.floor = None; }
        }
        self.levels.retain(|id, _| people.iter().any(|p| &p.id == id));
        self.participants = people;
    }

    pub fn me(&self) -> Option<&Participant> { self.participants.iter().find(|p| p.is_self) }

    /// The loudest speaker, for the roster highlight. `None` when nobody is
    /// above the noise floor — highlighting the least-quiet person in a silent
    /// room makes the indicator meaningless.
    pub fn active_speaker(&self) -> Option<&str> {
        if let Some(c) = &self.floor { return Some(&c.holder); }
        self.levels.iter()
            .filter(|(_, l)| **l > SPEAKING_FLOOR)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(id, _)| id.as_str())
    }

    pub fn set_level(&mut self, id: &str, level: f32) {
        if self.participants.iter().any(|p| p.id == id) {
            self.levels.insert(id.into(), level.clamp(0.0, 1.0));
        }
    }

    /// A peer claims the floor. Returns true if they now hold it.
    ///
    /// The earliest claim wins, and every peer computes that independently from
    /// the timestamp in the message — no server, no round trip, same answer
    /// everywhere. Ties break on id because two claims can carry the same
    /// millisecond, and a coin flip would have peers disagreeing about who is
    /// talking, which is exactly the failure this prevents.
    pub fn floor_assert(&mut self, holder: &str, at: u64) -> bool {
        if self.mode != TalkMode::PushToTalk { return false; }
        if !self.participants.iter().any(|p| p.id == holder) { return false; }
        let wins = match &self.floor {
            None => true,
            Some(cur) => at < cur.at || (at == cur.at && holder < cur.holder.as_str()),
        };
        if wins { self.floor = Some(Claim { holder: holder.into(), at }); }
        self.holds_floor(holder)
    }

    /// A peer releases. Only the holder can release, so a late release from
    /// someone who already lost the floor cannot silence the winner.
    pub fn floor_release(&mut self, holder: &str) -> bool {
        match &self.floor {
            Some(c) if c.holder == holder => { self.floor = None; true }
            _ => false,
        }
    }

    pub fn holds_floor(&self, id: &str) -> bool {
        self.floor.as_ref().map(|c| c.holder == id).unwrap_or(false)
    }

    /// May this participant's audio be transmitted right now?
    pub fn may_transmit(&self, id: &str) -> bool {
        if !self.joined { return false; }
        if self.muted && self.me().map(|m| m.id == id).unwrap_or(false) { return false; }
        match self.mode {
            TalkMode::Open => true,
            TalkMode::PushToTalk => self.holds_floor(id),
        }
    }

    pub fn join(&mut self) { self.joined = true; }

    /// Leaving must drop the floor as well: a peer that leaves holding it would
    /// otherwise lock the channel for everyone else.
    pub fn leave(&mut self) {
        self.joined = false;
        if let Some(me) = self.me().map(|m| m.id.clone()) { self.floor_release(&me); }
        self.levels.clear();
    }
}

/// Below this, a participant is not speaking. Highlighting the least-quiet
/// person in a silent room makes the indicator meaningless.
pub const SPEAKING_FLOOR: f32 = 0.08;

/// Draw the roster. Returns a participant id if one was activated.
pub fn roster_ui(ui: &mut egui::Ui, gv: &GroupVoice,
                 colors: &uikit_egui::PaletteColors) -> Option<String> {
    let active = gv.active_speaker().map(|s| s.to_string());
    let mut hit = None;
    for p in &gv.participants {
        let speaking = active.as_deref() == Some(p.id.as_str());
        let level = gv.levels.get(&p.id).copied().unwrap_or(0.0);
        let resp = ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(20.0), egui::Sense::hover());
            // the pulse: radius follows level, so a talker is visible at a glance
            let radius = 7.0 + level * 6.0;
            ui.painter().circle_filled(r.center(), radius,
                if speaking { colors.accent } else { colors.bg_card });
            let name = if p.is_self { format!("{} (you)", p.name) } else { p.name.clone() };
            ui.label(if speaking { egui::RichText::new(name).strong() }
                     else { egui::RichText::new(name) });
            if gv.holds_floor(&p.id) {
                ui.label(egui::RichText::new("holding").color(colors.accent).small());
            }
        }).response;
        if resp.interact(egui::Sense::click()).clicked() { hit = Some(p.id.clone()); }
    }
    if gv.participants.is_empty() {
        ui.label(egui::RichText::new("nobody in the channel").weak());
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(id: &str, me: bool) -> Participant {
        Participant { id: id.into(), name: id.to_uppercase(), is_self: me }
    }

    fn channel() -> GroupVoice {
        let mut gv = GroupVoice { mode: TalkMode::PushToTalk, ..Default::default() };
        gv.set_participants(vec![who("ada", true), who("bo", false), who("cy", false)]);
        gv.join();
        gv
    }

    #[test]
    fn the_earliest_claim_wins_so_every_peer_agrees_without_asking() {
        // No server arbitrates, so two people pressing talk at once must be
        // resolved identically on every device from the timestamp alone.
        let mut a = channel();
        let mut b = channel();
        // the same two claims, delivered in OPPOSITE orders
        a.floor_assert("bo", 100);
        a.floor_assert("cy", 90);
        b.floor_assert("cy", 90);
        b.floor_assert("bo", 100);
        assert_eq!(a.floor.as_ref().map(|c| c.holder.as_str()), Some("cy"));
        assert_eq!(a.floor, b.floor, "both peers reached the same answer");
    }

    #[test]
    fn a_tie_breaks_on_id_rather_than_a_coin_flip() {
        // Two claims can carry the same millisecond, and a random winner has
        // peers disagreeing about who is talking — the exact failure this
        // scheme exists to prevent.
        let mut a = channel();
        let mut b = channel();
        a.floor_assert("cy", 100);
        a.floor_assert("bo", 100);
        b.floor_assert("bo", 100);
        b.floor_assert("cy", 100);
        assert_eq!(a.floor.as_ref().map(|c| c.holder.as_str()), Some("bo"), "lower id wins");
        assert_eq!(a.floor, b.floor);
    }

    #[test]
    fn only_the_holder_can_release_the_floor() {
        // A late release from someone who already lost the claim must not
        // silence the winner.
        let mut gv = channel();
        gv.floor_assert("bo", 100);
        assert!(!gv.floor_release("cy"), "a non-holder cannot release");
        assert!(gv.holds_floor("bo"));
        assert!(gv.floor_release("bo"));
        assert!(gv.floor.is_none());
    }

    #[test]
    fn transmission_follows_the_floor_in_push_to_talk() {
        let mut gv = channel();
        assert!(!gv.may_transmit("ada"), "nobody holds the floor yet");
        gv.floor_assert("ada", 100);
        assert!(gv.may_transmit("ada"));
        assert!(!gv.may_transmit("bo"), "one speaker at a time");
        // muting silences ME even while holding the floor, which is what a mute
        // button has to mean or it is a lie
        gv.muted = true;
        assert!(!gv.may_transmit("ada"));
    }

    #[test]
    fn open_mode_lets_everyone_talk_but_still_respects_join_and_mute() {
        let mut gv = channel();
        gv.mode = TalkMode::Open;
        assert!(gv.may_transmit("bo"));
        gv.muted = true;
        assert!(!gv.may_transmit("ada"), "my mute applies to me");
        assert!(gv.may_transmit("bo"), "and not to anyone else");
        gv.muted = false;
        gv.leave();
        assert!(!gv.may_transmit("bo"), "having left, nothing transmits");
    }

    #[test]
    fn a_departing_holder_does_not_lock_the_channel() {
        // A peer that leaves holding the floor would otherwise silence everyone
        // until they came back.
        let mut gv = channel();
        gv.floor_assert("bo", 100);
        gv.set_participants(vec![who("ada", true), who("cy", false)]);
        assert!(gv.floor.is_none(), "the floor was released with the leaver");
        // and leaving myself drops my own floor
        gv.floor_assert("ada", 200);
        gv.leave();
        assert!(gv.floor.is_none());
    }

    #[test]
    fn the_active_speaker_is_nobody_in_a_silent_room() {
        // Highlighting the least-quiet person when nobody is speaking makes the
        // indicator meaningless.
        let mut gv = channel();
        gv.mode = TalkMode::Open;
        gv.set_level("bo", 0.01);
        gv.set_level("cy", 0.02);
        assert_eq!(gv.active_speaker(), None);
        gv.set_level("cy", 0.6);
        assert_eq!(gv.active_speaker(), Some("cy"));
        // and in push-to-talk the holder IS the speaker regardless of level
        gv.mode = TalkMode::PushToTalk;
        gv.floor_assert("bo", 10);
        assert_eq!(gv.active_speaker(), Some("bo"));
    }

    #[test]
    fn levels_for_departed_participants_do_not_linger() {
        let mut gv = channel();
        gv.mode = TalkMode::Open;
        gv.set_level("bo", 0.9);
        gv.set_participants(vec![who("ada", true)]);
        assert!(gv.levels.get("bo").is_none(), "a ghost cannot be the active speaker");
        assert_eq!(gv.active_speaker(), None);
        // and a level for someone not in the channel is refused
        gv.set_level("nobody", 0.9);
        assert!(gv.levels.get("nobody").is_none());
    }
}
