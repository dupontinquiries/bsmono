//! sources — `SourceTile` and `PlaylistsPage`.
//!
//! Two index surfaces: what you can play FROM, and what you have collected.
//!
//! ## The tile picks its glyph from the source TYPE, by substring
//!
//! Not from an enum, because the device reports whatever string it likes:
//! "Line In", "line-in", "AUX 1", "TuneIn Radio", "Spotify Connect". A match on
//! equality would fall through to the default for nearly everything real, and a
//! page of identical ♫ tiles tells the user nothing about which is which.
//!
//! Order matters in the ladder. "tunein" contains neither "radio" nor "net" but
//! belongs with radio; "spotify" would otherwise fall to the generic note. The
//! rules are checked most-specific first and the default is a music note rather
//! than a blank, because an unrecognised source is still a source.
//!
//! ## The playlist MODEL is not here
//!
//! `library.rs` already has `Playlist`, with reorder-as-move, the no-duplicates
//! rule and the name requirement. I checked before writing this time — the last
//! session I wrote a second sink picker without looking, and the existing one
//! was better. This module is only the INDEX page: the list, the counts, and
//! the inline create card.
//!
//! ## Create is an inline card, not a dialog
//!
//! It expands in place at the top of the list. A modal for one text field puts
//! a scrim over the list you are adding to, and closing it is a second gesture
//! for a job that took one.

/// A source the device can play from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    /// Whatever the device calls it. Matched by substring, not equality.
    pub kind: String,
}

impl Source {
    /// The tile glyph, most-specific rule first.
    pub fn glyph(&self) -> &'static str {
        let s = self.kind.to_lowercase();
        let has = |n: &str| s.contains(n);
        if has("line") || has("aux") { return "▦"; }
        if has("blue") { return "✦"; }
        if has("spotify") || has("music") { return "♪"; }
        if has("radio") || has("tunein") { return "📻"; }
        if has("tv") || has("hdmi") { return "▭"; }
        if has("airplay") { return "◬"; }
        if has("dlna") || has("net") { return "≋"; }
        // An unrecognised source is still a source.
        "♫"
    }

    /// Never blank: a tile with no label cannot be chosen deliberately.
    pub fn label(&self) -> String {
        if self.name.trim().is_empty() {
            if self.kind.trim().is_empty() { "Unknown source".into() } else { self.kind.clone() }
        } else {
            self.name.clone()
        }
    }
}

/// The sources page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sources {
    sources: Vec<Source>,
    /// The active source's name, as the device reports it.
    active: String,
}

impl Sources {
    pub fn set(&mut self, sources: Vec<Source>, active: &str) {
        self.sources = sources;
        self.active = active.to_string();
    }

    pub fn all(&self) -> &[Source] { &self.sources }
    pub fn is_active(&self, name: &str) -> bool { self.active == name }
    pub fn is_empty(&self) -> bool { self.sources.is_empty() }

    /// Choose a source. Returns the name for the host to switch to, or `None`
    /// when it is already active — re-selecting the current source would
    /// restart it, which is not what a tap on the highlighted tile means.
    pub fn choose(&self, name: &str) -> Option<String> {
        if !self.sources.iter().any(|s| s.name == name) { return None; }
        if self.is_active(name) { return None; }
        Some(name.to_string())
    }
}

// ── playlists index ─────────────────────────────────────────────────────────

/// A row on the playlists page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub track_count: u32,
    /// Unix seconds.
    pub updated_at: i64,
}

impl PlaylistSummary {
    /// `12 tracks · 3d ago`.
    pub fn subtitle(&self, now: i64) -> String {
        let tracks = if self.track_count == 1 { "1 track".to_string() }
                     else { format!("{} tracks", self.track_count) };
        format!("{} · {}", tracks, ago(now.saturating_sub(self.updated_at)))
    }
}

/// Coarse relative time, as elsewhere in the port.
pub fn ago(secs: i64) -> String {
    match secs {
        s if s < 0 => "just now".into(),
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// The inline create card.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateCard {
    pub expanded: bool,
    pub name: String,
    pub description: String,
}

impl CreateCard {
    pub fn expand(&mut self) { self.expanded = true; }

    /// Collapse and discard. Used by cancel, and after a successful create.
    pub fn reset(&mut self) {
        self.expanded = false;
        self.name.clear();
        self.description.clear();
    }

    /// Submit. `None` when the name is blank — a playlist with no name cannot
    /// be found again by the only means the page offers, which is reading it.
    pub fn submit(&mut self) -> Option<(String, String)> {
        let name = self.name.trim().to_string();
        if name.is_empty() { return None; }
        let desc = self.description.trim().to_string();
        self.reset();
        Some((name, desc))
    }
}

/// The playlists index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaylistsPage {
    playlists: Vec<PlaylistSummary>,
    pub create: CreateCard,
    /// The playlist created most recently, so the page can scroll to it — a new
    /// row appearing off-screen looks like nothing happened.
    pub focus_id: Option<i64>,
}

pub const EMPTY_TEXT: &str = "No playlists yet. Create one to start collecting tracks.";

impl PlaylistsPage {
    /// Replace the list. The host re-publishes after every create, delete or
    /// track change, so this is the only way the page learns anything.
    pub fn set(&mut self, playlists: Vec<PlaylistSummary>) {
        // A focus on a playlist that is gone would scroll to nothing.
        if let Some(f) = self.focus_id {
            if !playlists.iter().any(|p| p.id == f) { self.focus_id = None; }
        }
        self.playlists = playlists;
    }

    pub fn all(&self) -> &[PlaylistSummary] { &self.playlists }
    pub fn is_empty(&self) -> bool { self.playlists.is_empty() }

    /// Note a newly created playlist so the page can bring it into view.
    pub fn focus(&mut self, id: i64) { self.focus_id = Some(id); }
}

/// What the pages ask the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaylistAction {
    Create { name: String, description: String },
    Open(i64),
    Delete(i64),
}

/// Draw the sources grid. Returns the source to switch to.
pub fn sources_ui(ui: &mut egui::Ui, s: &Sources,
                  c: &uikit_egui::PaletteColors) -> Option<String> {
    if s.is_empty() {
        ui.colored_label(c.text_weak, "No sources reported by this device.");
        return None;
    }
    let mut chosen = None;
    egui::ScrollArea::vertical().id_salt("sources").show(ui, |ui| {
        for row in s.all().chunks(2) {
            ui.horizontal(|ui| {
                for src in row {
                    let active = s.is_active(&src.name);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width().min(150.0), 60.0),
                        egui::Sense::click());
                    // Springs on press, consistent with the StepKey language.
                    let scale = if resp.is_pointer_button_down_on() { 0.95 } else { 1.0 };
                    let r = egui::Rect::from_center_size(rect.center(), rect.size() * scale);
                    ui.painter().rect_filled(r, 8.0, if active { c.accent } else { c.bg_card });
                    ui.painter().rect_stroke(r, 8.0,
                        egui::Stroke::new(1.0, if active { c.accent } else { c.border }),
                        egui::StrokeKind::Inside);
                    let ink = if active { c.bg_base } else { c.accent };
                    ui.painter().text(r.left_center() + egui::vec2(12.0, 0.0),
                        egui::Align2::LEFT_CENTER, src.glyph(),
                        egui::FontId::proportional(18.0), ink);
                    ui.painter().text(r.left_center() + egui::vec2(38.0, 0.0),
                        egui::Align2::LEFT_CENTER, src.label(),
                        egui::FontId::proportional(13.0),
                        if active { c.bg_base } else { c.text });
                    if resp.clicked() { chosen = s.choose(&src.name); }
                }
            });
        }
    });
    chosen
}

/// Draw the playlists index.
pub fn playlists_ui(ui: &mut egui::Ui, p: &mut PlaylistsPage, now: i64,
                    c: &uikit_egui::PaletteColors) -> Option<PlaylistAction> {
    let mut act = None;
    // The create card expands IN PLACE. A modal for one text field puts a scrim
    // over the list you are adding to.
    if p.create.expanded {
        ui.group(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.create.name)
                .hint_text("Playlist name").desired_width(200.0));
            ui.add(egui::TextEdit::singleline(&mut p.create.description)
                .hint_text("Description (optional)").desired_width(200.0));
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    if let Some((name, description)) = p.create.submit() {
                        act = Some(PlaylistAction::Create { name, description });
                    }
                }
                if ui.button("Cancel").clicked() { p.create.reset(); }
            });
        });
    } else if ui.button("+ New playlist").clicked() {
        p.create.expand();
    }

    if p.is_empty() {
        ui.colored_label(c.text_weak, EMPTY_TEXT);
        return act;
    }
    egui::ScrollArea::vertical().id_salt("playlists").show(ui, |ui| {
        for pl in p.all() {
            let fresh = p.focus_id == Some(pl.id);
            ui.horizontal(|ui| {
                let text = egui::RichText::new(&pl.name)
                    .color(if fresh { c.accent } else { c.text });
                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                    act = Some(PlaylistAction::Open(pl.id));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(c.text_weak, pl.subtitle(now));
                });
            });
        }
    });
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str, kind: &str) -> Source { Source { name: name.into(), kind: kind.into() } }

    #[test]
    fn glyphs_are_matched_by_substring_because_devices_report_free_text() {
        // A match on equality falls through to the default for nearly
        // everything real, and a page of identical notes tells the user nothing
        // about which tile is which.
        assert_eq!(src("A", "Line In").glyph(), "▦");
        assert_eq!(src("A", "line-in").glyph(), "▦");
        assert_eq!(src("A", "AUX 1").glyph(), "▦");
        assert_eq!(src("A", "Bluetooth A2DP").glyph(), "✦");
        assert_eq!(src("A", "Spotify Connect").glyph(), "♪");
        assert_eq!(src("A", "TuneIn Radio").glyph(), "📻");
        assert_eq!(src("A", "HDMI ARC").glyph(), "▭");
        assert_eq!(src("A", "AirPlay 2").glyph(), "◬");
        assert_eq!(src("A", "DLNA renderer").glyph(), "≋");
    }

    #[test]
    fn the_ladder_is_ordered_most_specific_first() {
        // "tunein" contains neither "radio" nor "net" but belongs with radio;
        // "spotify" would otherwise fall through to the generic note.
        assert_eq!(src("A", "tunein").glyph(), "📻");
        assert_eq!(src("A", "spotify").glyph(), "♪");
        // "Line" wins over a name that also mentions music.
        assert_eq!(src("A", "Line In (music)").glyph(), "▦");
    }

    #[test]
    fn an_unrecognised_source_still_gets_a_glyph() {
        assert_eq!(src("A", "Whatever").glyph(), "♫");
        assert_eq!(src("A", "").glyph(), "♫");
    }

    #[test]
    fn a_tile_never_renders_blank() {
        // A tile with no label cannot be chosen deliberately.
        assert_eq!(src("", "Line In").label(), "Line In");
        assert_eq!(src("  ", "  ").label(), "Unknown source");
        assert_eq!(src("Turntable", "line").label(), "Turntable");
    }

    #[test]
    fn choosing_the_active_source_does_nothing() {
        // Re-selecting the current source would restart it, which is not what a
        // tap on the highlighted tile means.
        let mut s = Sources::default();
        s.set(vec![src("Line In", "line"), src("Spotify", "spotify")], "Line In");
        assert_eq!(s.choose("Line In"), None);
        assert_eq!(s.choose("Spotify"), Some("Spotify".into()));
    }

    #[test]
    fn choosing_a_source_that_is_not_there_does_nothing() {
        let mut s = Sources::default();
        s.set(vec![src("Line In", "line")], "Line In");
        assert_eq!(s.choose("Ghost"), None);
    }

    // ── playlists ───────────────────────────────────────────────────────────

    fn pl(id: i64, name: &str, n: u32, updated: i64) -> PlaylistSummary {
        PlaylistSummary { id, name: name.into(), description: String::new(),
                          track_count: n, updated_at: updated }
    }

    #[test]
    fn a_row_names_its_size_and_its_age() {
        let now = 1_000_000;
        assert_eq!(pl(1, "Drive", 12, now - 3 * 86_400).subtitle(now), "12 tracks · 3d ago");
        assert_eq!(pl(1, "Drive", 1, now - 30).subtitle(now), "1 track · just now");
    }

    #[test]
    fn a_future_timestamp_reads_as_just_now_rather_than_negative() {
        // A device with a skewed clock should not produce "-4m ago".
        assert_eq!(ago(-500), "just now");
    }

    #[test]
    fn creating_needs_a_name() {
        // A playlist with no name cannot be found again by the only means the
        // page offers, which is reading it.
        let mut c = CreateCard::default();
        c.expand();
        assert_eq!(c.submit(), None);
        c.name = "   ".into();
        assert_eq!(c.submit(), None);
        assert!(c.expanded, "and the card stays open to be corrected");
    }

    #[test]
    fn creating_trims_and_then_clears_the_card() {
        let mut c = CreateCard::default();
        c.expand();
        c.name = "  Drive  ".into();
        c.description = "  long roads  ".into();
        assert_eq!(c.submit(), Some(("Drive".into(), "long roads".into())));
        assert!(!c.expanded, "collapsed");
        assert!(c.name.is_empty(), "and cleared, so a second press adds nothing");
        assert_eq!(c.submit(), None);
    }

    #[test]
    fn a_description_is_optional() {
        let mut c = CreateCard::default();
        c.name = "Drive".into();
        assert_eq!(c.submit(), Some(("Drive".into(), String::new())));
    }

    #[test]
    fn cancelling_discards_what_was_typed() {
        let mut c = CreateCard::default();
        c.expand();
        c.name = "half typed".into();
        c.reset();
        assert!(!c.expanded);
        assert!(c.name.is_empty());
    }

    #[test]
    fn a_new_playlist_is_focused_so_it_can_be_scrolled_to() {
        // A new row appearing off-screen looks like nothing happened.
        let mut p = PlaylistsPage::default();
        p.set(vec![pl(1, "A", 0, 0), pl(2, "B", 0, 0)]);
        p.focus(2);
        assert_eq!(p.focus_id, Some(2));
    }

    #[test]
    fn a_focus_on_a_playlist_that_is_gone_is_dropped() {
        // Otherwise the page scrolls to nothing.
        let mut p = PlaylistsPage::default();
        p.set(vec![pl(1, "A", 0, 0), pl(2, "B", 0, 0)]);
        p.focus(2);
        p.set(vec![pl(1, "A", 0, 0)]);
        assert_eq!(p.focus_id, None);
    }

    #[test]
    fn an_empty_page_says_what_to_do_next() {
        let p = PlaylistsPage::default();
        assert!(p.is_empty());
        assert!(EMPTY_TEXT.contains("Create one"));
    }
}
