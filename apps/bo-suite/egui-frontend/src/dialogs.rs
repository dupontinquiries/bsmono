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

/// Draws the add-to-playlist dialog when `d.is_open()`. Returns
/// `(track_id, playlist_name)` the frame a playlist row is tapped — the
/// same pair `d.add()` returns, so callers resolve the real playlist id
/// out of it the same way regardless of how the dialog got its answer.
///
/// A floating `egui::Window` rather than inline content: this has to
/// appear above whatever page is showing, not just above the track row
/// that opened it, and it needs to stay open across an "add to Chill,
/// add to Focus, add to Loud" sequence in one sitting — `add()`'s own doc
/// on why the dialog doesn't close after one add.
pub fn add_to_playlist_ui(ui: &mut egui::Ui, d: &mut AddToPlaylist, c: &uikit_egui::PaletteColors)
    -> Option<(String, String)>
{
    let mut result = None;
    let mut still_open = d.is_open();
    egui::Window::new("Add to playlist")
        .id(egui::Id::new("add_to_playlist_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .show(ui.ctx(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut d.query)
                .hint_text("search playlists").id_salt("add_to_playlist-query"));
            if let Some(name) = &d.just_added {
                ui.colored_label(c.accent, format!("added to {name}"));
            }
            let visible: Vec<String> = d.visible().into_iter().map(str::to_string).collect();
            if visible.is_empty() {
                ui.colored_label(c.text_weak,
                    if d.playlists.is_empty() { "no playlists yet — create one first" }
                    else { "no playlists match" });
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for name in visible {
                    if ui.button(&name).clicked() {
                        if let Some(pair) = d.add(&name) { result = Some(pair); }
                    }
                }
            });
        });
    // The Window's own [x] sets `still_open` false; a track picked from a
    // list that then vanishes reads as a bug, so an explicit close is the
    // only thing that actually clears the subject.
    if !still_open { d.close(); }
    result
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

/// What `edit_track_ui` hands back when Save is pressed — `d.payload()`'s
/// pairs, resolved into typed fields (`year` parsed and range-checked,
/// same rule `problems()` already enforces so this can't fail here) plus
/// the track id, since the dialog only tracked it as a bare `String`.
pub struct EditSave {
    pub track_id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u16>,
}

/// Draws the edit-track dialog when `d.is_open()`. Returns `Some` the
/// frame Save is pressed (never while `can_save()` is false — the button
/// is disabled, and Enter is not wired to bypass it). A floating
/// `egui::Window`, same reasoning as `add_to_playlist_ui`: has to sit
/// above whichever page opened it.
pub fn edit_track_ui(ui: &mut egui::Ui, d: &mut EditTrack, c: &uikit_egui::PaletteColors)
    -> Option<EditSave>
{
    let mut result = None;
    let mut still_open = d.is_open();
    let mut cancel_pressed = false;
    let Some(track_id) = d.track.clone() else { return None };
    egui::Window::new("Edit track")
        .id(egui::Id::new("edit_track_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .show(ui.ctx(), |ui| {
            egui::Grid::new("edit_track_fields").num_columns(2).show(ui, |ui| {
                ui.colored_label(c.text_weak, "Title"); ui.text_edit_singleline(&mut d.draft.title); ui.end_row();
                ui.colored_label(c.text_weak, "Artist"); ui.text_edit_singleline(&mut d.draft.artist); ui.end_row();
                ui.colored_label(c.text_weak, "Album"); ui.text_edit_singleline(&mut d.draft.album); ui.end_row();
                ui.colored_label(c.text_weak, "Year"); ui.text_edit_singleline(&mut d.draft.year); ui.end_row();
            });
            for p in d.problems() {
                ui.colored_label(egui::Color32::from_rgb(200, 80, 80), p);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(d.can_save(), egui::Button::new("save")).clicked() {
                    let payload = d.payload();
                    let get = |key: &str| payload.iter().find(|(k, _)| *k == key)
                        .map(|(_, v)| v.clone());
                    result = Some(EditSave {
                        track_id: track_id.clone(),
                        title: get("title"),
                        artist: get("artist"),
                        album: get("album"),
                        // Guarded by can_save()/problems() above — an
                        // out-of-range or non-numeric year never reaches
                        // here, so a parse failure just omits the field
                        // rather than silently writing something wrong.
                        year: get("year").and_then(|y| y.parse::<u16>().ok()),
                    });
                }
                if ui.button("cancel").clicked() { cancel_pressed = true; }
            });
        });
    if !still_open || cancel_pressed { d.close(); }
    result
}

// ── scan picker ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ScanPicker {
    pub open: bool,
    pub suggestions: Vec<String>,
    selected: std::collections::BTreeSet<String>,
    pub custom_path: String,
    /// Real filesystem navigation, alongside the suggestion checkboxes
    /// and manual path field above — the shared, fuzzy-filterable widget
    /// from `uikit_egui::path_picker` rather than a second bespoke
    /// browser. `None` until "browse…" is pressed, since most scans just
    /// tick a suggestion and never need it.
    pub browse: Option<uikit_egui::path_picker::PathPicker>,
}

impl ScanPicker {
    pub fn open(&mut self, suggestions: Vec<String>) {
        self.open = true;
        self.suggestions = suggestions;
        self.selected.clear();
        self.custom_path.clear();
        self.browse = None;
    }

    pub fn close(&mut self) { self.open = false; }

    pub fn toggle(&mut self, path: &str) {
        if !self.selected.remove(path) { self.selected.insert(path.to_string()); }
    }

    pub fn is_selected(&self, path: &str) -> bool { self.selected.contains(path) }

    /// A folder chosen through `browse` — added to both the selection AND
    /// the suggestion list, so it shows up ticked rather than as a
    /// selected-but-invisible path the person can't see confirmed.
    pub fn add_browsed(&mut self, path: String) {
        if !self.suggestions.contains(&path) { self.suggestions.push(path.clone()); }
        self.selected.insert(path);
    }

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

/// Draws the scan-picker dialog when `d.open`. Returns the gathered paths
/// the frame "scan" is pressed. A floating `egui::Window`, same reasoning
/// as the other two dialogs in this module.
pub fn scan_picker_ui(ui: &mut egui::Ui, d: &mut ScanPicker, c: &uikit_egui::PaletteColors)
    -> Option<Vec<String>>
{
    let mut result = None;
    let mut still_open = d.open;
    let mut cancel_pressed = false;
    egui::Window::new("Choose folders to scan")
        .id(egui::Id::new("scan_picker_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .show(ui.ctx(), |ui| {
            if d.suggestions.is_empty() {
                ui.colored_label(c.text_weak, "no suggested folders — type a path below");
            }
            for path in d.suggestions.clone() {
                let mut checked = d.is_selected(&path);
                if ui.checkbox(&mut checked, &path).changed() { d.toggle(&path); }
            }
            ui.add(egui::TextEdit::singleline(&mut d.custom_path)
                .hint_text("custom path").id_salt("scan_picker-custom_path"));
            ui.horizontal(|ui| {
                let label = if d.browse.is_some() { "hide browser" } else { "browse…" };
                if ui.button(label).clicked() {
                    d.browse = if d.browse.is_some() { None } else {
                        // Start from $HOME when the shell has one — a
                        // browser that opens at "nowhere yet" (PathPicker's
                        // own idle state) makes someone press "up" or type
                        // a root before it's useful for anything.
                        let mut p = uikit_egui::path_picker::PathPicker::new();
                        if let Some(home) = std::env::var_os("HOME").and_then(|h| h.into_string().ok()) {
                            p.open(&home, uikit_egui::path_picker::list_local);
                        }
                        Some(p)
                    };
                }
            });
            if let Some(browse) = d.browse.as_mut() {
                ui.separator();
                match uikit_egui::path_picker::picker_ui(ui, browse, c, 6) {
                    Some(uikit_egui::path_picker::Action::Enter(p)) =>
                        browse.open(&p, uikit_egui::path_picker::list_local),
                    Some(uikit_egui::path_picker::Action::Up) =>
                        browse.up(uikit_egui::path_picker::list_local),
                    Some(uikit_egui::path_picker::Action::ToggleHidden) => {
                        browse.show_hidden = !browse.show_hidden;
                        let cur = browse.current().map(str::to_string);
                        if let Some(cur) = cur { browse.open(&cur, uikit_egui::path_picker::list_local); }
                    }
                    Some(uikit_egui::path_picker::Action::Choose(p)) => {
                        d.add_browsed(p);
                        d.browse = None;
                    }
                    None => {}
                }
                ui.separator();
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(d.can_scan(), egui::Button::new("scan")).clicked() {
                    result = Some(d.gather());
                }
                if ui.button("cancel").clicked() { cancel_pressed = true; }
            });
        });
    if !still_open || cancel_pressed { d.close(); }
    result
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    fn draw(d: &mut AddToPlaylist) -> (Vec<String>, Option<(String, String)>) {
        let ctx = egui::Context::default();
        let c = uikit_egui::palette_colors("amber");
        let mut result = None;
        let run = |ctx: &egui::Context, d: &mut AddToPlaylist, result: &mut Option<(String, String)>| {
            ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                                                            egui::vec2(500.0, 700.0))),
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *result = add_to_playlist_ui(ui, d, &c);
                });
            })
        };
        // A floating Window auto-sizes from its previous frame's content —
        // the first pass has no prior size to lay out against, so nothing
        // is painted yet. Two passes is the same thing a real app gets for
        // free by simply running more than one frame.
        run(&ctx, d, &mut result);
        let out = run(&ctx, d, &mut result);
        let texts = out.shapes.iter().filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect();
        (texts, result)
    }

    #[test]
    fn closed_draws_nothing() {
        let mut d = AddToPlaylist::default();
        let (texts, result) = draw(&mut d);
        assert!(texts.is_empty(), "{texts:?}");
        assert_eq!(result, None);
    }

    #[test]
    fn open_shows_the_playlists() {
        let mut d = AddToPlaylist { playlists: vec!["Chill".into(), "Focus".into()],
                                     ..Default::default() };
        d.open("t1");
        let (texts, _) = draw(&mut d);
        let joined = texts.join(" | ");
        assert!(joined.contains("Chill") && joined.contains("Focus"), "{joined}");
    }

    #[test]
    fn no_playlists_says_so_rather_than_showing_an_empty_list() {
        let mut d = AddToPlaylist::default();
        d.open("t1");
        let (texts, _) = draw(&mut d);
        assert!(texts.join(" | ").contains("create one first"), "{texts:?}");
    }

    fn draw_edit(d: &mut EditTrack) -> (Vec<String>, Option<EditSave>) {
        let ctx = egui::Context::default();
        let c = uikit_egui::palette_colors("amber");
        let mut result = None;
        let run = |ctx: &egui::Context, d: &mut EditTrack, result: &mut Option<EditSave>| {
            ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                                                            egui::vec2(500.0, 700.0))),
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *result = edit_track_ui(ui, d, &c);
                });
            })
        };
        run(&ctx, d, &mut result);
        let out = run(&ctx, d, &mut result);
        let texts = out.shapes.iter().filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect();
        (texts, result)
    }

    #[test]
    fn closed_edit_draws_nothing() {
        let mut d = EditTrack::default();
        let (texts, result) = draw_edit(&mut d);
        assert!(texts.is_empty(), "{texts:?}");
        assert!(result.is_none());
    }

    #[test]
    fn open_edit_shows_the_current_fields() {
        let mut d = EditTrack::default();
        d.open("t1", TrackFields { title: "Old Title".into(), artist: "Some Artist".into(),
                                    album: "".into(), year: "".into() });
        let (texts, _) = draw_edit(&mut d);
        let joined = texts.join(" | ");
        assert!(joined.contains("Old Title") && joined.contains("Some Artist"), "{joined}");
    }

    #[test]
    fn a_bad_year_shows_a_problem() {
        let mut d = EditTrack::default();
        d.open("t1", TrackFields::default());
        d.draft.year = "nineteen".into();
        let (texts, _) = draw_edit(&mut d);
        assert!(texts.join(" | ").contains("not a year"), "{texts:?}");
        assert!(!d.can_save(), "the save button should be disabled for this draft");
    }

    fn draw_scan(d: &mut ScanPicker) -> (Vec<String>, Option<Vec<String>>) {
        let ctx = egui::Context::default();
        let c = uikit_egui::palette_colors("amber");
        let mut result = None;
        let run = |ctx: &egui::Context, d: &mut ScanPicker, result: &mut Option<Vec<String>>| {
            ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                                                            egui::vec2(500.0, 700.0))),
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *result = scan_picker_ui(ui, d, &c);
                });
            })
        };
        run(&ctx, d, &mut result);
        let out = run(&ctx, d, &mut result);
        let texts = out.shapes.iter().filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect();
        (texts, result)
    }

    #[test]
    fn closed_scan_picker_draws_nothing() {
        let mut d = ScanPicker::default();
        let (texts, result) = draw_scan(&mut d);
        assert!(texts.is_empty(), "{texts:?}");
        assert!(result.is_none());
    }

    #[test]
    fn open_scan_picker_shows_the_suggestions() {
        let mut d = ScanPicker::default();
        d.open(vec!["/home/x/Music".into(), "/media/usb".into()]);
        let (texts, _) = draw_scan(&mut d);
        let joined = texts.join(" | ");
        assert!(joined.contains("/home/x/Music") && joined.contains("/media/usb"), "{joined}");
    }

    #[test]
    fn no_suggestions_says_so() {
        let mut d = ScanPicker::default();
        d.open(vec![]);
        let (texts, _) = draw_scan(&mut d);
        assert!(texts.join(" | ").contains("type a path below"), "{texts:?}");
    }

    #[test]
    fn a_browse_toggle_is_offered_and_its_label_reflects_state() {
        let mut d = ScanPicker::default();
        d.open(vec!["/music".into()]);
        let (texts, _) = draw_scan(&mut d);
        assert!(texts.join(" | ").contains("browse…"), "{texts:?}");
        d.browse = Some(uikit_egui::path_picker::PathPicker::new());
        let (texts, _) = draw_scan(&mut d);
        assert!(texts.join(" | ").contains("hide browser"), "{texts:?}");
    }
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

    #[test]
    fn a_browsed_folder_is_both_ticked_and_visible_in_the_list() {
        // add_browsed's whole point: a path chosen via the shared
        // path_picker has to show up ticked, not just be selected somewhere
        // the person can't see confirmed.
        let mut s = ScanPicker::default();
        s.open(vec!["/music".into()]);
        s.add_browsed("/media/usb/podcasts".into());
        assert!(s.suggestions.contains(&"/media/usb/podcasts".to_string()));
        assert!(s.is_selected("/media/usb/podcasts"));
        assert!(s.gather().contains(&"/media/usb/podcasts".to_string()));
    }

    #[test]
    fn browsing_to_an_existing_suggestion_does_not_duplicate_the_row() {
        let mut s = ScanPicker::default();
        s.open(vec!["/music".into()]);
        s.add_browsed("/music".into());
        assert_eq!(s.suggestions.iter().filter(|p| *p == "/music").count(), 1);
    }

    #[test]
    fn reopening_the_scan_picker_also_closes_any_open_browser() {
        let mut s = ScanPicker::default();
        s.open(vec!["/a".into()]);
        s.browse = Some(uikit_egui::path_picker::PathPicker::new());
        s.open(vec!["/a".into()]);
        assert!(s.browse.is_none(), "a stale browser from the last open would show the wrong folder");
    }
}
