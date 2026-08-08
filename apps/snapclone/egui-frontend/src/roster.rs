//! roster — `GroupVoiceRoster`, ported.
//!
//! The header puts it well: *"The bar is the control (press to talk, the lock,
//! your own level); the roster is the room (the other people, and who is
//! speaking)."* `voicebar.rs` ported the control. This is the room.
//!
//! ## A pure view, and that is load-bearing
//!
//! *"It is a PURE VIEW — no audio or network objects."* Membership comes from
//! the channel, the active speaker from `floorHolderChanged`, the level from
//! whichever mic is live. The roster decides none of it, which is why it can be
//! tested without a call and why it cannot disagree with the floor logic — the
//! failure where the bar says you hold the floor and the roster highlights
//! somebody else.
//!
//! ## Order is preserved and self can be anywhere
//!
//! *"order preserved; self can be anywhere"*. Sorting the list — alphabetically,
//! or self-first — would reorder the room under someone mid-glance, and in a
//! voice call the list is how you keep track of who is present. The channel's
//! order is a fact about the channel; the roster is not entitled to improve it.
//!
//! ## An empty active id means the floor is OPEN, not missing
//!
//! *"Empty id = floor open (nobody highlighted)."* That is a real state and a
//! common one — between speakers, in a quiet moment — and it is distinct from
//! "we do not know who is speaking". Highlighting the last speaker through a
//! silence would show someone talking when nobody is.

/// One person in the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub is_self: bool,
}

impl Participant {
    /// Never blank: a row with no label cannot be told from another row with no
    /// label, and the roster exists to tell people apart.
    pub fn label(&self) -> String {
        let n = self.name.trim();
        if n.is_empty() { format!("({})", self.id) } else { n.to_string() }
    }
}

/// How quickly the displayed level follows the real one, per second.
///
/// Smoothed because a mic level is noisy at frame rate, and a ring that jumped
/// with every sample would read as interference rather than as speech.
pub const LEVEL_EASE: f32 = 8.0;

/// The room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Roster {
    people: Vec<Participant>,
    /// Empty means the floor is open.
    active: String,
    /// Target level, as reported.
    level: f32,
    /// Displayed level, easing toward it.
    shown: f32,
}

impl Roster {
    pub fn count(&self) -> usize { self.people.len() }
    pub fn people(&self) -> &[Participant] { &self.people }
    pub fn is_empty(&self) -> bool { self.people.is_empty() }

    /// Who holds the floor, or `None` when it is open.
    pub fn active_speaker(&self) -> Option<&str> {
        if self.active.is_empty() { return None; }
        Some(&self.active)
    }

    pub fn floor_open(&self) -> bool { self.active.is_empty() }

    /// Set the channel membership. Order is preserved exactly as given.
    pub fn set_participants(&mut self, people: Vec<Participant>) {
        // An active speaker who has left is cleared. Highlighting someone no
        // longer in the room is worse than highlighting nobody, and the floor
        // logic will name a new holder on its own schedule.
        if !self.active.is_empty() && !people.iter().any(|p| p.id == self.active) {
            self.active.clear();
            self.level = 0.0;
            self.shown = 0.0;
        }
        self.people = people;
    }

    /// Who holds the floor. An empty id opens it.
    pub fn set_active_speaker(&mut self, id: &str) {
        if id.is_empty() {
            self.active.clear();
            // The level belongs to the speaker, so it goes with them. Leaving
            // it would let the ring keep breathing around an empty floor.
            self.level = 0.0;
            return;
        }
        // A speaker who is not in the roster is refused rather than stored: the
        // roster cannot highlight a row it does not have, and silently keeping
        // the id would make `active_speaker()` name someone invisible.
        if !self.people.iter().any(|p| p.id == id) { return; }
        if self.active != id {
            self.active = id.to_string();
            // A new speaker starts from silence rather than inheriting the last
            // one's level, which would make them appear to start mid-shout.
            self.level = 0.0;
            self.shown = 0.0;
        }
    }

    /// The active speaker's mic level, 0..=1.
    pub fn set_speaker_level(&mut self, level01: f32) {
        // Ignored while the floor is open: a level with no speaker has nothing
        // to be drawn around.
        if self.active.is_empty() { return; }
        self.level = level01.clamp(0.0, 1.0);
    }

    /// Advance the level smoothing.
    pub fn tick(&mut self, dt_s: f32) {
        let a = (LEVEL_EASE * dt_s).clamp(0.0, 1.0);
        self.shown += (self.level - self.shown) * a;
    }

    /// The displayed level for the breathing ring.
    pub fn ring_level(&self) -> f32 { self.shown.clamp(0.0, 1.0) }

    /// Is this row the one speaking?
    pub fn is_speaking(&self, id: &str) -> bool {
        !self.active.is_empty() && self.active == id
    }

    /// What the header says.
    ///
    /// Names the count rather than listing, and says the floor is open in words
    /// — an unhighlighted list looks the same as a list nobody is speaking in
    /// and a list whose highlight failed to render.
    pub fn summary(&self) -> String {
        match self.people.len() {
            0 => "Nobody in the channel".into(),
            1 => "1 person".into(),
            n => format!("{n} people"),
        }
    }
}

/// Draw the roster.
pub fn roster_ui(ui: &mut egui::Ui, r: &Roster, c: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        ui.colored_label(c.text_weak, r.summary());
        if r.floor_open() && !r.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.text_weak, "floor open");
            });
        }
    });
    for p in r.people() {
        ui.horizontal(|ui| {
            let speaking = r.is_speaking(&p.id);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0),
                                                   egui::Sense::hover());
            // The ring breathes with the level, and only around the speaker.
            let base = 8.0;
            let radius = if speaking { base + 4.0 * r.ring_level() } else { base };
            let col = if speaking { c.accent } else { c.text_weak };
            ui.painter().circle_stroke(rect.center(), radius,
                                       egui::Stroke::new(2.0, col));
            let name = if p.is_self { format!("{} (you)", p.label()) } else { p.label() };
            ui.colored_label(if speaking { c.text } else { c.text_weak }, name);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str, name: &str, is_self: bool) -> Participant {
        Participant { id: id.into(), name: name.into(), is_self }
    }

    fn room() -> Roster {
        let mut r = Roster::default();
        r.set_participants(vec![person("ada", "Ada", false),
                                person("me", "Me", true),
                                person("bob", "Bob", false)]);
        r
    }

    #[test]
    fn the_channels_order_is_preserved_and_self_stays_where_it_is() {
        // Sorting would reorder the room under someone mid-glance, and in a
        // call the list is how you keep track of who is present. The channel's
        // order is a fact about the channel.
        let r = room();
        let ids: Vec<&str> = r.people().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["ada", "me", "bob"]);
        assert!(r.people()[1].is_self, "self is second and stays second");
    }

    #[test]
    fn an_empty_active_id_means_the_floor_is_open_not_unknown() {
        // A real and common state — between speakers, in a quiet moment.
        let r = room();
        assert!(r.floor_open());
        assert_eq!(r.active_speaker(), None);
    }

    #[test]
    fn setting_a_speaker_highlights_exactly_one_row() {
        let mut r = room();
        r.set_active_speaker("bob");
        assert_eq!(r.active_speaker(), Some("bob"));
        assert!(r.is_speaking("bob"));
        assert!(!r.is_speaking("ada"));
        assert!(!r.floor_open());
    }

    #[test]
    fn opening_the_floor_clears_the_highlight() {
        // Highlighting the last speaker through a silence would show someone
        // talking when nobody is.
        let mut r = room();
        r.set_active_speaker("bob");
        r.set_active_speaker("");
        assert!(r.floor_open());
        assert!(!r.is_speaking("bob"));
    }

    #[test]
    fn a_speaker_who_is_not_in_the_room_is_refused() {
        // The roster cannot highlight a row it does not have, and storing the
        // id anyway would make active_speaker() name someone invisible.
        let mut r = room();
        r.set_active_speaker("stranger");
        assert!(r.floor_open());
        assert_eq!(r.active_speaker(), None);
    }

    #[test]
    fn a_speaker_who_leaves_is_cleared_rather_than_left_highlighted() {
        // Highlighting someone no longer in the room is worse than
        // highlighting nobody; the floor logic will name a new holder itself.
        let mut r = room();
        r.set_active_speaker("bob");
        r.set_participants(vec![person("ada", "Ada", false), person("me", "Me", true)]);
        assert!(r.floor_open());
        assert_eq!(r.count(), 2);
    }

    #[test]
    fn a_speaker_who_stays_keeps_the_floor_across_a_membership_change() {
        // Someone joining must not interrupt the person talking.
        let mut r = room();
        r.set_active_speaker("bob");
        r.set_participants(vec![person("ada", "Ada", false),
                                person("me", "Me", true),
                                person("bob", "Bob", false),
                                person("cy", "Cy", false)]);
        assert_eq!(r.active_speaker(), Some("bob"));
    }

    #[test]
    fn the_level_is_smoothed_rather_than_followed_exactly() {
        // A mic level is noisy at frame rate, and a ring that jumped with every
        // sample would read as interference rather than as speech.
        let mut r = room();
        r.set_active_speaker("bob");
        r.set_speaker_level(1.0);
        r.tick(1.0 / 60.0);
        let first = r.ring_level();
        assert!(first > 0.0 && first < 1.0, "eased, at {first}");
        for _ in 0..60 { r.tick(1.0 / 60.0); }
        assert!(r.ring_level() > 0.9);
    }

    #[test]
    fn a_new_speaker_starts_from_silence() {
        // Inheriting the last speaker's level would make them appear to start
        // mid-shout.
        let mut r = room();
        r.set_active_speaker("bob");
        r.set_speaker_level(1.0);
        for _ in 0..60 { r.tick(1.0 / 60.0); }
        r.set_active_speaker("ada");
        assert_eq!(r.ring_level(), 0.0);
    }

    #[test]
    fn the_level_goes_with_the_speaker_when_the_floor_opens() {
        // Otherwise the ring keeps breathing around an empty floor.
        let mut r = room();
        r.set_active_speaker("bob");
        r.set_speaker_level(1.0);
        for _ in 0..60 { r.tick(1.0 / 60.0); }
        r.set_active_speaker("");
        for _ in 0..60 { r.tick(1.0 / 60.0); }
        assert!(r.ring_level() < 0.01);
    }

    #[test]
    fn a_level_with_no_speaker_is_ignored() {
        // It has nothing to be drawn around.
        let mut r = room();
        r.set_speaker_level(1.0);
        r.tick(1.0);
        assert_eq!(r.ring_level(), 0.0);
    }

    #[test]
    fn an_out_of_range_level_is_clamped() {
        let mut r = room();
        r.set_active_speaker("bob");
        r.set_speaker_level(4.0);
        for _ in 0..120 { r.tick(1.0 / 60.0); }
        assert!(r.ring_level() <= 1.0);
    }

    #[test]
    fn a_nameless_participant_still_renders_as_something() {
        // A row with no label cannot be told from another row with no label,
        // and the roster exists to tell people apart.
        let p = person("peer-7f3a", "  ", false);
        assert_eq!(p.label(), "(peer-7f3a)");
    }

    #[test]
    fn an_empty_room_says_so_rather_than_showing_nothing() {
        let r = Roster::default();
        assert!(r.is_empty());
        assert_eq!(r.summary(), "Nobody in the channel");
        assert!(r.floor_open());
    }

    #[test]
    fn the_summary_counts_people_rather_than_listing_them() {
        let mut r = Roster::default();
        r.set_participants(vec![person("a", "A", false)]);
        assert_eq!(r.summary(), "1 person");
        r.set_participants(vec![person("a", "A", false), person("b", "B", false)]);
        assert_eq!(r.summary(), "2 people");
    }
}

/// Drive the roster from the same floor signal the bar uses.
///
/// One call site, because the bar and the roster disagreeing about who holds
/// the floor is the visible failure this pairing exists to prevent: the bar
/// says you are talking while the roster highlights somebody else, and the user
/// cannot tell which is lying.
///
/// The bar carries the talker as a NAME (it draws it); the roster needs an ID
/// (it matches rows). Resolving here rather than at either end means the lookup
/// rule lives in one place — and a name that matches nobody opens the floor
/// rather than highlighting a guess.
pub fn sync_from_floor(r: &mut Roster, talker_name: &str, level01: f32) {
    if talker_name.trim().is_empty() {
        r.set_active_speaker("");
        return;
    }
    let id = r.people().iter()
        .find(|p| p.name == talker_name)
        .map(|p| p.id.clone());
    match id {
        Some(id) => {
            r.set_active_speaker(&id);
            r.set_speaker_level(level01);
        }
        // Not in the roster: open the floor rather than highlight a guess. The
        // membership list and the floor signal can arrive out of order, and a
        // wrong highlight is worse than none for the second it takes to settle.
        None => r.set_active_speaker(""),
    }
}

#[cfg(test)]
mod floor_sync_tests {
    use super::*;

    fn room() -> Roster {
        let mut r = Roster::default();
        r.set_participants(vec![
            Participant { id: "ada".into(), name: "Ada".into(), is_self: false },
            Participant { id: "me".into(), name: "Me".into(), is_self: true }]);
        r
    }

    #[test]
    fn the_bars_talker_highlights_the_matching_row() {
        // One source of truth: the bar saying you are talking while the roster
        // highlights someone else is the failure this prevents.
        let mut r = room();
        sync_from_floor(&mut r, "Ada", 0.6);
        assert_eq!(r.active_speaker(), Some("ada"));
    }

    #[test]
    fn an_empty_talker_opens_the_floor() {
        let mut r = room();
        sync_from_floor(&mut r, "Ada", 0.6);
        sync_from_floor(&mut r, "", 0.0);
        assert!(r.floor_open());
    }

    #[test]
    fn a_talker_who_is_not_in_the_roster_opens_the_floor() {
        // Membership and the floor signal can arrive out of order, and a wrong
        // highlight is worse than none for the second it takes to settle.
        let mut r = room();
        sync_from_floor(&mut r, "Stranger", 0.9);
        assert!(r.floor_open());
    }

    #[test]
    fn the_level_follows_the_talker_through_the_same_call() {
        let mut r = room();
        sync_from_floor(&mut r, "Ada", 1.0);
        for _ in 0..60 { r.tick(1.0 / 60.0); }
        assert!(r.ring_level() > 0.9);
    }
}
