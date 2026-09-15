//! speaker_list — the voice selection list: who is enrolled, who is speaking,
//! and who you are attributing to.
//!
//! The registry itself was already here. `src/speakers.rs` is Qt-free and
//! `lib.rs` mounts it by `#[path]`, so enrolment, ids, colours and renaming
//! all work in this frontend today. What was missing is the LIST — nothing in
//! `egui-frontend` called `speakers::list_with_ids()` at all, and the only
//! mention of the module outside the mount was a doc comment in
//! `voice_routing.rs`. So the data was reachable and unreachable-to-the-user
//! at the same time.
//!
//! This is the selection model over that registry: pure, testable without a
//! window, and deliberately NOT a second copy of the registry.
//!
//! ## Id, not name
//!
//! `speakers.rs` is emphatic that `id` is stable and `name` is display-only —
//! rename "Me" to "Dad" and every back-reference must survive. A list that
//! tracked its selection by name would break on exactly the operation the
//! registry went to the trouble of supporting, so [`SpeakerList`] selects by
//! id and resolves the name for display at read time.
//!
//! ## The live speaker and the selected speaker are different things
//!
//! Two highlights, easily conflated into one and wrong if they are:
//!
//! * **Selected** — who the user is pointing at, to rename or attribute to.
//!   Changes only when the user acts.
//! * **Speaking** — who the recogniser currently believes is talking. Changes
//!   on its own, several times a sentence.
//!
//! Merging them means the selection jumps out from under a rename dialog the
//! moment somebody else says a word. They are separate fields here and the
//! tests hold them apart.

/// One row, resolved for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Stable id. What selection and attribution key on.
    pub id: String,
    /// Display name. Mutable; never used as a key.
    pub name: String,
    /// Registry colour, as stored.
    pub color: String,
    pub selected: bool,
    /// The recogniser currently believes this one is talking.
    pub speaking: bool,
}

/// The selection model over the speaker registry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpeakerList {
    /// `(id, name, color)` exactly as the registry returned it, in registry
    /// order. Not re-sorted: enrolment order is the order people expect,
    /// and alphabetising a four-name list gains nothing while making the
    /// positions move when somebody is renamed.
    entries: Vec<(String, String, String)>,
    selected: Option<String>,
    speaking: Option<String>,
}

impl SpeakerList {
    /// Build from `speakers::list_with_ids()`'s shape.
    pub fn from_entries(entries: Vec<(String, String, String)>) -> SpeakerList {
        SpeakerList { entries, selected: None, speaking: None }
    }

    /// Re-read the registry, keeping the selection if that speaker survives.
    ///
    /// A refresh that silently cleared the selection would lose the user's
    /// place every time anyone enrolled, and enrolment happens mid-conversation.
    pub fn refresh(&mut self, entries: Vec<(String, String, String)>) {
        self.entries = entries;
        if let Some(sel) = &self.selected {
            if !self.entries.iter().any(|(id, _, _)| id == sel) {
                // The selected speaker was deleted. Clearing is right —
                // pointing at a row that no longer exists is worse than
                // pointing at nothing — but it must be deliberate, not a
                // side effect of refreshing.
                self.selected = None;
            }
        }
        if let Some(sp) = &self.speaking {
            if !self.entries.iter().any(|(id, _, _)| id == sp) {
                self.speaking = None;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn speaking_id(&self) -> Option<&str> {
        self.speaking.as_deref()
    }

    /// Select by id. An unknown id selects nothing rather than panicking or
    /// holding a dangling selection.
    pub fn select(&mut self, id: &str) -> bool {
        if self.entries.iter().any(|(i, _, _)| i == id) {
            self.selected = Some(id.to_string());
            true
        } else {
            false
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// The recogniser's current belief. `None` when it does not know, which
    /// is a real state and distinct from "nobody is talking".
    pub fn set_speaking(&mut self, id: Option<&str>) {
        self.speaking = match id {
            Some(i) if self.entries.iter().any(|(x, _, _)| x == i) => Some(i.to_string()),
            // An unrecognised speaker must not leave the PREVIOUS one lit.
            // The old highlight would read as "still them", which is the
            // opposite of what the recogniser just said.
            _ => None,
        };
    }

    /// Move the selection by `delta` rows, for keyboard and voice navigation.
    ///
    /// Clamps rather than wraps: wrapping a short list makes "down" from the
    /// last row jump to the first, which in a list this size reads as the
    /// selection having been lost. With nothing selected, a move selects the
    /// first row, so the first keypress does something visible.
    pub fn move_selection(&mut self, delta: i32) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let cur = self
            .selected
            .as_ref()
            .and_then(|s| self.entries.iter().position(|(i, _, _)| i == s));
        let next = match cur {
            None => 0,
            Some(c) => (c as i32 + delta).clamp(0, self.entries.len() as i32 - 1) as usize,
        };
        self.selected = Some(self.entries[next].0.clone());
        true
    }

    /// Find by display name, case-insensitively — how a voice command arrives
    /// ("attribute that to Dad"). Returns the id, because everything
    /// downstream keys on id.
    ///
    /// Ambiguity returns `None`: two speakers called "Sam" is a real
    /// possibility and picking the first is how the wrong person gets
    /// credited for a recording.
    pub fn id_for_name(&self, name: &str) -> Option<&str> {
        let want = name.trim().to_lowercase();
        let mut found = None;
        for (id, n, _) in &self.entries {
            if n.trim().to_lowercase() == want {
                if found.is_some() {
                    return None;
                }
                found = Some(id.as_str());
            }
        }
        found
    }

    /// The rows to draw.
    pub fn rows(&self) -> Vec<Row> {
        self.entries
            .iter()
            .map(|(id, name, color)| Row {
                id: id.clone(),
                name: name.clone(),
                color: color.clone(),
                selected: self.selected.as_deref() == Some(id.as_str()),
                speaking: self.speaking.as_deref() == Some(id.as_str()),
            })
            .collect()
    }

    /// What to show when the list is empty.
    ///
    /// Not "no speakers": that reads as a failure. Nobody has enrolled yet is
    /// the ordinary first-run state and the message should say what to do.
    pub fn empty_hint(&self) -> Option<&'static str> {
        self.is_empty().then_some("No voices enrolled yet — speak and confirm to add one.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people() -> Vec<(String, String, String)> {
        vec![
            ("spk_1".into(), "Ada".into(), "#e06c75".into()),
            ("spk_2".into(), "Bo".into(), "#98c379".into()),
            ("spk_3".into(), "Cy".into(), "#61afef".into()),
        ]
    }

    fn list() -> SpeakerList {
        SpeakerList::from_entries(people())
    }

    /// THE POINT OF KEYING ON ID. `speakers.rs` supports renaming precisely so
    /// back-references survive; a list selecting by name would break on the
    /// one operation the registry went to trouble over.
    #[test]
    fn a_rename_does_not_lose_the_selection() {
        let mut l = list();
        assert!(l.select("spk_2"));
        let mut renamed = people();
        renamed[1].1 = "Bojack".into();
        l.refresh(renamed);
        assert_eq!(l.selected_id(), Some("spk_2"), "still the same person");
        let row = l.rows().into_iter().find(|r| r.selected).unwrap();
        assert_eq!(row.name, "Bojack", "and shows the new name");
    }

    /// Selected and speaking are different questions. Merging them makes the
    /// selection jump out from under a rename the moment anyone else talks.
    #[test]
    fn the_live_speaker_does_not_move_the_selection() {
        let mut l = list();
        l.select("spk_1");
        l.set_speaking(Some("spk_3"));
        assert_eq!(l.selected_id(), Some("spk_1"));
        assert_eq!(l.speaking_id(), Some("spk_3"));
        let rows = l.rows();
        assert!(rows[0].selected && !rows[0].speaking);
        assert!(rows[2].speaking && !rows[2].selected);
    }

    /// An unrecognised speaker must clear the highlight, not leave the last
    /// one lit — a stale highlight reads as "still them".
    #[test]
    fn an_unknown_speaker_clears_the_highlight() {
        let mut l = list();
        l.set_speaking(Some("spk_2"));
        assert_eq!(l.speaking_id(), Some("spk_2"));
        l.set_speaking(None);
        assert_eq!(l.speaking_id(), None);
        l.set_speaking(Some("spk_2"));
        l.set_speaking(Some("nobody_here"));
        assert_eq!(l.speaking_id(), None, "an unknown id is not the previous one");
    }

    /// Enrolment happens mid-conversation; a refresh must not cost the user
    /// their place.
    #[test]
    fn enrolling_someone_new_keeps_the_current_selection() {
        let mut l = list();
        l.select("spk_1");
        let mut more = people();
        more.push(("spk_4".into(), "Dee".into(), "#c678dd".into()));
        l.refresh(more);
        assert_eq!(l.selected_id(), Some("spk_1"));
        assert_eq!(l.len(), 4);
    }

    /// ...but a deleted speaker must not leave a dangling selection.
    #[test]
    fn deleting_the_selected_speaker_clears_the_selection() {
        let mut l = list();
        l.select("spk_2");
        l.speaking = Some("spk_2".into());
        l.refresh(vec![people()[0].clone(), people()[2].clone()]);
        assert_eq!(l.selected_id(), None);
        assert_eq!(l.speaking_id(), None, "and the highlight goes with it");
    }

    #[test]
    fn selecting_an_unknown_id_changes_nothing() {
        let mut l = list();
        l.select("spk_1");
        assert!(!l.select("spk_99"));
        assert_eq!(l.selected_id(), Some("spk_1"), "the old selection survives a bad id");
    }

    /// Clamping, not wrapping: in a three-row list, "down" from the bottom
    /// jumping to the top reads as the selection being lost.
    #[test]
    fn moving_clamps_at_both_ends() {
        let mut l = list();
        l.select("spk_3");
        l.move_selection(1);
        assert_eq!(l.selected_id(), Some("spk_3"), "no wrap past the end");
        l.select("spk_1");
        l.move_selection(-1);
        assert_eq!(l.selected_id(), Some("spk_1"), "no wrap past the start");
        l.move_selection(1);
        assert_eq!(l.selected_id(), Some("spk_2"));
    }

    /// The first keypress must do something visible.
    #[test]
    fn moving_with_nothing_selected_takes_the_first_row() {
        let mut l = list();
        assert!(l.move_selection(1));
        assert_eq!(l.selected_id(), Some("spk_1"));
        // ...and on an empty list it does nothing rather than panicking.
        let mut e = SpeakerList::default();
        assert!(!e.move_selection(1));
    }

    /// "Attribute that to Dad" arrives as a name, and must resolve to an id.
    #[test]
    fn a_name_resolves_to_an_id_case_insensitively() {
        let l = list();
        assert_eq!(l.id_for_name("Bo"), Some("spk_2"));
        assert_eq!(l.id_for_name("  bo "), Some("spk_2"));
        assert_eq!(l.id_for_name("nobody"), None);
    }

    /// Two people called Sam is a real possibility, and crediting the first
    /// one is how the wrong person ends up on a recording.
    #[test]
    fn an_ambiguous_name_resolves_to_nothing_rather_than_the_first() {
        let l = SpeakerList::from_entries(vec![
            ("spk_1".into(), "Sam".into(), "#fff".into()),
            ("spk_2".into(), "sam".into(), "#000".into()),
        ]);
        assert_eq!(l.id_for_name("Sam"), None);
    }

    /// Registry order, not alphabetical: enrolment order is what people
    /// expect, and sorting makes rows move when someone is renamed.
    #[test]
    fn rows_keep_registry_order() {
        let l = SpeakerList::from_entries(vec![
            ("spk_1".into(), "Zoe".into(), "#fff".into()),
            ("spk_2".into(), "Ada".into(), "#000".into()),
        ]);
        let names: Vec<String> = l.rows().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["Zoe", "Ada"]);
    }

    /// First run is not a failure and should not read like one.
    #[test]
    fn an_empty_list_says_what_to_do() {
        let e = SpeakerList::default();
        let hint = e.empty_hint().expect("a hint");
        assert!(hint.contains("enrolled"), "{hint}");
        assert!(!hint.starts_with("No speakers"), "must not read as an error: {hint}");
        assert!(list().empty_hint().is_none(), "no hint once there are rows");
    }
}
