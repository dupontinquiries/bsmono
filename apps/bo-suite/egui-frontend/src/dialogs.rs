//! dialogs — add-to-playlist, edit-track, and the scan picker.
//!
//! Three modals in one module because they share a lifecycle: `open(t)` binds a
//! subject, `close()` clears it, and everything between is editing that subject.
//! The QML makes the lifecycle explicit — `close()` sets `track = null` as well
//! as hiding — and that is the part worth porting carefully, because a modal
//! that hides without clearing shows the *previous* track for one frame the next
//! time it opens.
//!
//! ## AddToPlaylistDialog
//!
//! `justAdded` exists so the dialog can confirm without closing: you add a track
//! to three playlists in a row, and each needs feedback. A dialog that closed on
//! the first add would make the second two a chore.
//!
//! ## EditTrackDialog
//!
//! `buildPayload()` sends only what CHANGED. Sending every field means an edit
//! to the title also rewrites the artist with whatever was in the box, which
//! silently reverts a correction someone else made.
//!
//! ## ScanPickerDialog
//!
//! Suggested paths with checkboxes plus a custom path. `gatherSelectedPaths()`
//! merges them, and the merge has to deduplicate: a custom path that is also a
//! suggestion would otherwise be scanned twice.

// ── add to playlist ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct AddToPlaylist {
    /// The subject. `None` = closed, mirroring the QML's `track = null`.
    pub track: Option<String>,
    pub playlists: Vec<String>,
    /// Last playlist added to, for the inline confirmation.
    pub just_added: Option<String>,
    pub query: String,
}

impl AddToPlaylist {
    pub fn open(&mut self, track_id: &str) {
        self.track = Some(track_id.to_string());
        // Cleared on OPEN as well as close: a stale "added to Chill" over a
        // different track is worse than no feedback.
        self.just_added = None;
        self.query.clear();
    }

    pub fn close(&mut self) { self.track = None; self.just_added = None; }
    pub fn is_open(&self) -> bool { self.track.is_some() }

    /// Add. Returns the pair to send, or `None` when there is nothing to add.
    ///
    /// The dialog stays open — you add a track to three playlists in a row, and
    /// closing on the first would make the other two a chore.
    pub fn add(&mut self, playlist: &str) -> Option<(String, String)> {
        let t = self.track.clone()?;
        if !self.playlists.iter().any(|p| p == playlist) { return None; }
        self.just_added = Some(playlist.to_string());
        Some((t, playlist.to_string()))
    }

    pub fn visible(&self) -> Vec<&str> {
        let q = self.query.trim().to_lowercase();
        self.playlists.iter().map(|s| s.as_str())
            .filter(|p| q.is_empty() || p.to_lowercase().contains(&q)).collect()
    }
}

// ── edit track ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackFields {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
}

#[derive(Debug, Clone, Default)]
pub struct EditTrack {
    pub track: Option<String>,
    pub draft: TrackFields,
    original: TrackFields,
}

impl EditTrack {
    pub fn open(&mut self, track_id: &str, fields: TrackFields) {
        self.track = Some(track_id.to_string());
        self.draft = fields.clone();
        self.original = fields;
    }

    pub fn close(&mut self) {
        // Clearing the subject AND the drafts: a modal that only hides shows the
        // previous track for one frame the next time it opens.
        self.track = None;
        self.draft = TrackFields::default();
        self.original = TrackFields::default();
    }

    pub fn is_open(&self) -> bool { self.track.is_some() }
    pub fn is_dirty(&self) -> bool { self.draft != self.original }

    /// `buildPayload()` — only the fields that CHANGED.
    ///
    /// Sending everything means an edit to the title also rewrites the artist
    /// with whatever was in the box, silently reverting a correction someone
    /// else made between opening the dialog and saving it.
    pub fn payload(&self) -> Vec<(&'static str, String)> {
        let mut v = Vec::new();
        if self.draft.title != self.original.title { v.push(("title", self.draft.title.clone())); }
        if self.draft.artist != self.original.artist { v.push(("artist", self.draft.artist.clone())); }
        if self.draft.album != self.original.album { v.push(("album", self.draft.album.clone())); }
        if self.draft.year != self.original.year { v.push(("year", self.draft.year.clone())); }
        v
    }

    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        let y = self.draft.year.trim();
        // A year that is not a year silently becomes 0 in most backends, which
        // then sorts the track to the beginning of every list.
        if !y.is_empty() && y.parse::<u32>().map(|n| !(1000..=2999).contains(&n)).unwrap_or(true) {
            v.push(format!("{y:?} is not a year"));
        }
        v
    }

    pub fn can_save(&self) -> bool { self.is_dirty() && self.problems().is_empty() }
}

// ── scan picker ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ScanPicker {
    pub open: bool,
    pub suggestions: Vec<String>,
    selected: std::collections::BTreeSet<String>,
    pub custom_path: String,
}

impl ScanPicker {
    pub fn open(&mut self, suggestions: Vec<String>) {
        self.open = true;
        self.suggestions = suggestions;
        self.selected.clear();
        self.custom_path.clear();
    }

    pub fn close(&mut self) { self.open = false; }

    pub fn toggle(&mut self, path: &str) {
        if !self.selected.remove(path) { self.selected.insert(path.to_string()); }
    }

    pub fn is_selected(&self, path: &str) -> bool { self.selected.contains(path) }

    /// `gatherSelectedPaths()` — the ticked suggestions plus the custom path.
    ///
    /// Deduplicated, because a custom path that is also a suggestion would be
    /// scanned twice. Sorted so the same choices always produce the same scan
    /// order, which makes a slow scan resumable in a predictable place.
    pub fn gather(&self) -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> = self.selected.clone();
        let c = self.custom_path.trim();
        if !c.is_empty() { set.insert(c.to_string()); }
        set.into_iter().collect()
    }

    pub fn can_scan(&self) -> bool { !self.gather().is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atp() -> AddToPlaylist {
        AddToPlaylist { playlists: vec!["Chill".into(), "Focus".into(), "Loud".into()],
                        ..Default::default() }
    }

    #[test]
    fn adding_confirms_without_closing() {
        // You add a track to three playlists in a row; closing on the first
        // makes the other two a chore.
        let mut d = atp();
        d.open("t1");
        assert_eq!(d.add("Chill"), Some(("t1".into(), "Chill".into())));
        assert!(d.is_open(), "still open");
        assert_eq!(d.just_added.as_deref(), Some("Chill"));
        assert_eq!(d.add("Focus").unwrap().1, "Focus");
        assert_eq!(d.just_added.as_deref(), Some("Focus"));
    }

    #[test]
    fn the_confirmation_does_not_survive_a_new_subject() {
        // A stale "added to Chill" over a different track is worse than none.
        let mut d = atp();
        d.open("t1");
        d.add("Chill");
        d.open("t2");
        assert_eq!(d.just_added, None);
    }

    #[test]
    fn adding_with_nothing_open_or_to_an_unknown_playlist_does_nothing() {
        let mut d = atp();
        assert_eq!(d.add("Chill"), None, "no subject");
        d.open("t1");
        assert_eq!(d.add("Nope"), None, "unknown playlist");
        assert_eq!(d.just_added, None);
    }

    #[test]
    fn closing_clears_the_subject_not_just_the_visibility() {
        // A modal that only hides shows the previous track for one frame next
        // time it opens.
        let mut d = atp();
        d.open("t1");
        d.close();
        assert!(!d.is_open());
        assert_eq!(d.track, None);
    }

    #[test]
    fn the_edit_payload_carries_only_what_changed() {
        // Sending everything rewrites the artist with whatever was in the box,
        // silently reverting a correction someone else made.
        let mut e = EditTrack::default();
        e.open("t1", TrackFields { title: "Old".into(), artist: "A".into(),
                                   album: "B".into(), year: "1999".into() });
        assert!(e.payload().is_empty(), "nothing changed yet");
        assert!(!e.can_save());
        e.draft.title = "New".into();
        assert_eq!(e.payload(), vec![("title", "New".to_string())]);
        assert!(e.can_save());
        e.draft.year = "2001".into();
        assert_eq!(e.payload().len(), 2, "and only those two");
    }

    #[test]
    fn a_year_that_is_not_a_year_blocks_the_save() {
        // It silently becomes 0 in most backends, which sorts the track to the
        // beginning of every list.
        let mut e = EditTrack::default();
        e.open("t1", TrackFields::default());
        e.draft.year = "nineteen".into();
        assert!(!e.problems().is_empty());
        assert!(!e.can_save());
        e.draft.year = "1999".into();
        assert!(e.problems().is_empty());
        // an EMPTY year is fine — plenty of tracks have none
        e.draft.year = "".into();
        assert!(e.problems().is_empty());
    }

    #[test]
    fn reopening_the_editor_does_not_carry_the_previous_draft() {
        let mut e = EditTrack::default();
        e.open("t1", TrackFields { title: "One".into(), ..Default::default() });
        e.draft.title = "Edited".into();
        e.close();
        e.open("t2", TrackFields { title: "Two".into(), ..Default::default() });
        assert_eq!(e.draft.title, "Two");
        assert!(!e.is_dirty());
    }

    #[test]
    fn the_scan_picker_deduplicates_a_custom_path_that_is_also_a_suggestion() {
        // Otherwise it is scanned twice.
        let mut s = ScanPicker::default();
        s.open(vec!["/music".into(), "/media/usb".into()]);
        s.toggle("/music");
        s.custom_path = "/music".into();
        assert_eq!(s.gather(), vec!["/music"], "once, not twice");
        s.custom_path = "/extra".into();
        assert_eq!(s.gather(), vec!["/extra", "/music"], "sorted, so a scan resumes predictably");
    }

    #[test]
    fn a_blank_custom_path_is_not_a_path() {
        let mut s = ScanPicker::default();
        s.open(vec!["/music".into()]);
        s.custom_path = "   ".into();
        assert!(!s.can_scan(), "nothing ticked and nothing typed");
        s.toggle("/music");
        assert!(s.can_scan());
        s.toggle("/music");
        assert!(!s.can_scan(), "un-ticking removes it again");
    }

    #[test]
    fn reopening_the_scan_picker_starts_clean() {
        let mut s = ScanPicker::default();
        s.open(vec!["/a".into()]);
        s.toggle("/a");
        s.custom_path = "/b".into();
        s.open(vec!["/a".into()]);
        assert!(!s.can_scan(), "previous choices are not carried over");
    }
}
