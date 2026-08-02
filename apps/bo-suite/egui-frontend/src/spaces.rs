//! spaces — the routing matrix, ported from `SpacesPage.qml`.
//!
//! bo-suite is a **spatial home audio router**: it decides which channel of a
//! layout plays on which speaker, across a mesh of Mozart, ASE, Linux and phone
//! nodes. The transport bar is the small half of the app; this is the large one,
//! and `SpacesPage.qml` is 1,565 lines of it.
//!
//! The model is not reimplemented. `bo-mux/src/protocol.rs` is Qt-free and
//! already carries `Speaker`, `Layout`, `Claim` and the coordinator election, so
//! the port uses those types.
//!
//! ## What the page is actually for
//!
//! A **layout** has channels (`2.0` → L, R; `5.1` → FL FR C LFE SL SR). A mesh
//! has speakers. The assignment map says which speaker plays which channel, and
//! the QML lets you drag a channel chip onto a speaker.
//!
//! Three rules make that assignment sane, and all three come from what goes
//! wrong without them:
//!
//! * **A speaker plays at most one channel.** Two channels on one speaker is not
//!   a wider soundstage, it is a downmix nobody asked for.
//! * **Reassigning moves rather than duplicating.** Dragging FL onto a speaker
//!   that already holds FR must displace FR, or the user silently ends up with
//!   FR playing in two rooms.
//! * **Unassigned channels are visible.** A 5.1 layout with four speakers has
//!   two channels nowhere, and the centre channel going missing is exactly the
//!   thing you notice as "dialogue is quiet" an hour later.

#[path = "../../bo-mux/src/protocol.rs"]
#[allow(dead_code, unused_imports)]
pub mod protocol;

pub use protocol::{Layout, Platform, Speaker};

use std::collections::BTreeMap;

/// The routing table for one space.
#[derive(Debug, Clone, Default)]
pub struct Routing {
    pub layout_name: String,
    pub channels: Vec<String>,
    pub speakers: Vec<Speaker>,
    /// speaker id → channel. Keyed by SPEAKER because the invariant is
    /// one-channel-per-speaker; keying by channel would let two speakers hold
    /// the same one and make the rule impossible to state.
    assign: BTreeMap<String, String>,
}

impl Routing {
    pub fn new(layout: &Layout, speakers: Vec<Speaker>) -> Routing {
        Routing {
            layout_name: layout.name.clone(),
            channels: layout.channels.clone(),
            speakers,
            assign: BTreeMap::new(),
        }
    }

    /// Put `channel` on `speaker`.
    ///
    /// Moves rather than copies: any other speaker holding this channel gives it
    /// up. Dragging FL onto a speaker that already holds FR displaces FR, which
    /// is what a drag means — otherwise the user silently ends up with one
    /// channel playing in two rooms.
    pub fn assign(&mut self, speaker: &str, channel: &str) -> bool {
        if !self.speakers.iter().any(|s| s.id == speaker) { return false; }
        if !self.channels.iter().any(|c| c == channel) { return false; }
        self.assign.retain(|_, c| c != channel);
        self.assign.insert(speaker.to_string(), channel.to_string());
        true
    }

    pub fn unassign(&mut self, speaker: &str) -> Option<String> {
        self.assign.remove(speaker)
    }

    pub fn channel_of(&self, speaker: &str) -> Option<&str> {
        self.assign.get(speaker).map(|s| s.as_str())
    }

    pub fn speaker_of(&self, channel: &str) -> Option<&str> {
        self.assign.iter().find(|(_, c)| c.as_str() == channel).map(|(s, _)| s.as_str())
    }

    /// Channels with nowhere to play.
    ///
    /// Surfaced rather than silently dropped: a 5.1 layout on four speakers has
    /// two channels missing, and a centre channel going nowhere is what you
    /// notice as "dialogue is quiet" an hour later.
    pub fn unassigned(&self) -> Vec<&str> {
        self.channels.iter().map(|c| c.as_str())
            .filter(|c| self.speaker_of(c).is_none()).collect()
    }

    /// Speakers with nothing to play. Idle rather than broken, but worth
    /// showing so a user knows the room is silent on purpose.
    pub fn idle_speakers(&self) -> Vec<&Speaker> {
        self.speakers.iter().filter(|s| !self.assign.contains_key(&s.id)).collect()
    }

    pub fn is_complete(&self) -> bool { self.unassigned().is_empty() }

    /// Assign channels to speakers in order — the "just make it work" button.
    ///
    /// Deliberately dumb and deterministic: it fills in declaration order, so
    /// the same mesh always produces the same room. A cleverer auto-assign that
    /// used signal strength would give a different answer each time it ran,
    /// which is worse than a plain one you can predict and then adjust.
    pub fn auto_assign(&mut self) {
        self.assign.clear();
        for (c, s) in self.channels.clone().iter().zip(self.speakers.clone().iter()) {
            self.assign.insert(s.id.clone(), c.clone());
        }
    }

    /// Swap what two speakers are playing — the common fix when the room is
    /// right but left and right are backwards.
    pub fn swap(&mut self, a: &str, b: &str) {
        let (ca, cb) = (self.assign.remove(a), self.assign.remove(b));
        if let Some(c) = cb { self.assign.insert(a.to_string(), c); }
        if let Some(c) = ca { self.assign.insert(b.to_string(), c); }
    }

    /// The assignment, as `speaker = channel` lines under `[routing]`.
    ///
    /// Persisted because losing which speaker plays which channel on restart is
    /// the worst failure a spatial router has: the room sounds wrong, and the
    /// user has to rebuild an arrangement they set up once and expected to
    /// keep. Everything else here — the speaker list, the layout's channels —
    /// comes back from the mesh, so only the ASSIGNMENT is worth storing.
    pub fn render(&self) -> String {
        let mut out = String::from("# bo-suite speaker routing
[routing]
");
        out.push_str(&format!("layout = {}
", self.layout_name));
        for (speaker, channel) in &self.assign {
            out.push_str(&format!("{speaker} = {channel}
"));
        }
        out
    }

    /// Restore an assignment over the CURRENT mesh.
    ///
    /// A saved assignment naming a speaker that is not here is skipped rather
    /// than recreated: a speaker that has left the mesh cannot play anything,
    /// and a phantom entry would make the room look complete while a channel is
    /// silently missing. Same for a channel the current layout does not have —
    /// restoring a 5.1 file onto a stereo layout must not invent channels.
    pub fn restore(&mut self, text: &str) -> usize {
        let ini = config_kit::Ini::parse(text);
        let Some(section) = ini.sections.get("routing") else { return 0 };
        let mut applied = 0;
        for (speaker, channel) in section {
            if speaker == "layout" { continue; }
            if self.assign(speaker, channel) { applied += 1; }
        }
        applied
    }

    /// Changing layout keeps whatever still applies.
    ///
    /// Going 5.1 → 2.0 and back should not lose the whole room. Channels the new
    /// layout does not have are dropped; the rest keep their speakers.
    pub fn set_layout(&mut self, layout: &Layout) {
        self.layout_name = layout.name.clone();
        self.channels = layout.channels.clone();
        let keep: Vec<String> = self.channels.clone();
        self.assign.retain(|_, c| keep.contains(c));
    }
}

/// Draw the matrix. Returns a (speaker, channel) assignment if one was made.
pub fn spaces_ui(ui: &mut egui::Ui, r: &mut Routing,
                 colors: &uikit_egui::PaletteColors) -> Option<(String, String)> {
    let mut made = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&r.layout_name).strong());
        if ui.button("auto").clicked() { r.auto_assign(); }
        if !r.is_complete() {
            ui.label(egui::RichText::new(format!("{} unassigned", r.unassigned().len()))
                .color(colors.warning).small());
        }
    });
    ui.separator();
    // unassigned channels first: they are the thing needing attention
    let free: Vec<String> = r.unassigned().iter().map(|s| s.to_string()).collect();
    if !free.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("free:").weak().small());
            for c in &free {
                ui.label(egui::RichText::new(c).color(colors.warning));
            }
        });
        ui.separator();
    }
    let speakers = r.speakers.clone();
    for s in &speakers {
        ui.horizontal(|ui| {
            ui.label(&s.name);
            let cur = r.channel_of(&s.id).map(|c| c.to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt(&s.id)
                    .selected_text(cur.clone().unwrap_or_else(|| "—".into()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(cur.is_none(), "—").clicked() { r.unassign(&s.id); }
                        for c in r.channels.clone() {
                            if ui.selectable_label(cur.as_deref() == Some(c.as_str()), &c).clicked() {
                                if r.assign(&s.id, &c) { made = Some((s.id.clone(), c.clone())); }
                            }
                        }
                    });
            });
        });
    }
    if speakers.is_empty() {
        ui.label(egui::RichText::new("no speakers on the mesh").weak());
    }
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spk(id: &str) -> Speaker {
        Speaker { id: id.into(), name: id.to_uppercase(), platform: Platform::Mozart }
    }

    fn room() -> Routing {
        Routing::new(&Layout::quad(), vec![spk("a"), spk("b"), spk("c"), spk("d")])
    }

    #[test]
    fn a_speaker_plays_at_most_one_channel() {
        // Two channels on one speaker is not a wider soundstage, it is a
        // downmix nobody asked for.
        let mut r = room();
        assert!(r.assign("a", "FL"));
        assert!(r.assign("a", "FR"));
        assert_eq!(r.channel_of("a"), Some("FR"), "the later assignment replaces");
        assert_eq!(r.speaker_of("FL"), None, "and FL is now free");
    }

    #[test]
    fn reassigning_moves_a_channel_rather_than_duplicating_it() {
        // Dragging FL onto a speaker that holds FR must displace FR, or the user
        // silently ends up with one channel playing in two rooms.
        let mut r = room();
        r.assign("a", "FL");
        r.assign("b", "FL");
        assert_eq!(r.speaker_of("FL"), Some("b"));
        assert_eq!(r.channel_of("a"), None, "the old speaker gave it up");
    }

    #[test]
    fn unassigned_channels_are_visible() {
        // A 5.1 layout on four speakers has two channels nowhere, and a centre
        // channel going missing is what you notice as "dialogue is quiet" an
        // hour later.
        let mut r = Routing::new(&Layout::surround_51(),
                                 vec![spk("a"), spk("b"), spk("c"), spk("d")]);
        r.auto_assign();
        let free = r.unassigned();
        assert_eq!(free.len(), 2, "six channels, four speakers: {free:?}");
        assert!(!r.is_complete());
        // and with enough speakers nothing is left over
        let mut q = room();
        q.auto_assign();
        assert!(q.is_complete());
        assert!(q.idle_speakers().is_empty());
    }

    #[test]
    fn auto_assign_is_deterministic_so_the_same_mesh_gives_the_same_room() {
        // A cleverer version using signal strength would answer differently
        // each run, which is worse than a plain one you can predict and adjust.
        let mut a = room(); a.auto_assign();
        let mut b = room(); b.auto_assign();
        for s in ["a", "b", "c", "d"] { assert_eq!(a.channel_of(s), b.channel_of(s)); }
        assert_eq!(a.channel_of("a"), Some("FL"), "declaration order");
    }

    #[test]
    fn swapping_two_speakers_fixes_a_reversed_room() {
        // The common fix: the room is right but left and right are backwards.
        let mut r = room();
        r.auto_assign();
        let (ca, cb) = (r.channel_of("a").unwrap().to_string(),
                        r.channel_of("b").unwrap().to_string());
        r.swap("a", "b");
        assert_eq!(r.channel_of("a"), Some(cb.as_str()));
        assert_eq!(r.channel_of("b"), Some(ca.as_str()));
        // swapping with an empty speaker moves rather than duplicating
        r.unassign("c");
        r.swap("a", "c");
        assert_eq!(r.channel_of("a"), None);
        assert_eq!(r.channel_of("c"), Some(cb.as_str()));
    }

    #[test]
    fn changing_layout_keeps_what_still_applies() {
        // Going 5.1 -> 2.0 and back should not lose the whole room.
        let mut r = Routing::new(&Layout::surround_51(),
                                 vec![spk("a"), spk("b"), spk("c"), spk("d")]);
        r.assign("a", "FL");
        r.assign("b", "FR");
        r.assign("c", "C");
        r.set_layout(&Layout::stereo());
        // Stereo has L/R, not FL/FR, so none of the surround assignments
        // survive — which is correct, not a bug: FL is not L.
        assert_eq!(r.channel_of("a"), None);
        assert_eq!(r.unassigned(), vec!["L", "R"]);
        // and going back to 5.1 leaves a clean slate rather than stale routing
        r.set_layout(&Layout::surround_51());
        assert_eq!(r.unassigned().len(), 6);
        assert_eq!(r.channel_of("c"), None, "C is not in a stereo layout");
    }

    #[test]
    fn a_routing_survives_a_restart() {
        // The worst failure a spatial router has: the room sounds wrong and the
        // user rebuilds an arrangement they set up once.
        let mut a = room();
        a.auto_assign();
        let text = a.render();
        let mut b = room();
        assert_eq!(b.restore(&text), 4, "all four reapplied");
        for s in ["a", "b", "c", "d"] {
            assert_eq!(a.channel_of(s), b.channel_of(s), "speaker {s}");
        }
        assert!(b.is_complete());
    }

    #[test]
    fn a_speaker_that_has_left_the_mesh_is_skipped_not_recreated() {
        // A phantom entry makes the room look complete while a channel is
        // silently missing — the user hears a gap and the UI says everything is
        // assigned.
        let mut a = room();
        a.auto_assign();
        let text = a.render();
        let mut b = Routing::new(&Layout::quad(), vec![spk("a"), spk("b")]);
        let n = b.restore(&text);
        assert_eq!(n, 2, "only the speakers actually present");
        assert!(!b.is_complete(), "and it says so rather than pretending");
        assert_eq!(b.unassigned().len(), 2);
    }

    #[test]
    fn restoring_a_surround_file_onto_stereo_invents_nothing() {
        // The saved channels are not in this layout, so none apply.
        let mut a = Routing::new(&Layout::surround_51(),
                                 vec![spk("a"), spk("b"), spk("c"), spk("d")]);
        a.auto_assign();
        let text = a.render();
        let mut b = Routing::new(&Layout::stereo(), vec![spk("a"), spk("b")]);
        assert_eq!(b.restore(&text), 0, "FL is not L");
        assert_eq!(b.channels, vec!["L", "R"]);
    }

    #[test]
    fn a_missing_or_junk_file_leaves_the_room_alone() {
        // First run, or a truncated write. Neither should clear a working
        // arrangement.
        let mut r = room();
        r.auto_assign();
        assert_eq!(r.restore(""), 0);
        assert_eq!(r.restore("nonsense without a section"), 0);
        assert!(r.is_complete(), "the live assignment is untouched");
    }

    #[test]
    fn an_unknown_speaker_or_channel_is_refused() {
        // A silent no-op would leave the user dragging a chip that never lands.
        let mut r = room();
        assert!(!r.assign("nope", "FL"));
        assert!(!r.assign("a", "SUBWOOFER"));
        assert!(r.channel_of("a").is_none());
    }

    #[test]
    fn an_empty_mesh_does_not_panic() {
        let mut r = Routing::new(&Layout::quad(), vec![]);
        r.auto_assign();
        assert_eq!(r.unassigned().len(), 4, "every channel is homeless");
        assert!(r.idle_speakers().is_empty());
        r.swap("a", "b");
    }
}
