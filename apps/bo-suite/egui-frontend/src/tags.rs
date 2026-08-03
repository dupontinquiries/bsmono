//! tags — `StarRating`, `TagsPage`, `TagEditor`, `TagDetailPage`.
//!
//! The browse pages ported earlier let you find music by album and artist. Tags
//! are the other axis — the one the user builds themselves — and a rating is
//! the cheapest tag there is.
//!
//! ## The rating is optimistic AND self-healing
//!
//! Tapping a star sets it immediately, without waiting for the write. That is
//! the right default: a rating control that pauses before responding makes the
//! user tap again, and now they have rated it twice.
//!
//! But the backend echoes the CONFIRMED value back, and the widget reconciles.
//! If a write was clamped or rejected, the star settles on the truth instead of
//! showing the user a value nothing agreed to. Optimism without reconciliation
//! is just a lie with a shorter lifespan.
//!
//! The reconcile has to be scoped to the track it names, too. An echo for a
//! different track arriving while you look at this one must not repaint this
//! one's stars — that is the same stale-response race the browse detail pages
//! and the QR scanner both had, in a third costume.
//!
//! ## Tap the star you are on to un-rate
//!
//! The standard gesture, and it needs saying because it is the only way BACK to
//! unrated. Without it, three stars is a decision you cannot undo — every other
//! tap sets some other rating, and none of them is "none".
//!
//! ## A tag with no colour still needs one
//!
//! The tiles are coloured by tag. A tag created without a colour falls back to
//! a default rather than rendering as a hole in the grid — and the fallback is
//! DERIVED from the name, so two uncoloured tags do not become the same tile.

/// Ratings run 1..=5, with 0 meaning unrated.
pub const MAX_STARS: u8 = 5;

/// A five-star rating control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarRating {
    pub track_id: i64,
    /// What is shown. May be ahead of the backend while a write is in flight.
    pub value: u8,
    /// What the backend last confirmed, so a rejected write can be undone.
    confirmed: u8,
    pub interactive: bool,
}

impl StarRating {
    pub fn new(track_id: i64, value: u8) -> Self {
        let v = value.min(MAX_STARS);
        StarRating { track_id, value: v, confirmed: v, interactive: true }
    }

    /// Read-only, for dense lists where you want to show rather than edit.
    pub fn read_only(track_id: i64, value: u8) -> Self {
        StarRating { interactive: false, ..StarRating::new(track_id, value) }
    }

    /// Tap star `n` (1-based). Returns the value to write, or `None` if the tap
    /// did nothing.
    ///
    /// Tapping the star that already equals the rating clears it — the only way
    /// back to unrated, and without it a rating is a decision you cannot undo.
    pub fn tap(&mut self, n: u8) -> Option<u8> {
        if !self.interactive || n == 0 || n > MAX_STARS { return None; }
        self.value = if self.value == n { 0 } else { n };
        Some(self.value)
    }

    /// The backend confirmed a value. Reconciles only when the echo is for THIS
    /// track: an echo for another track arriving while you look at this one
    /// must not repaint these stars.
    pub fn reconcile(&mut self, track_id: i64, value: u8) -> bool {
        if track_id != self.track_id { return false; }
        let v = value.min(MAX_STARS);
        self.confirmed = v;
        // Optimism without reconciliation is a lie with a shorter lifespan.
        self.value = v;
        true
    }

    /// True while the shown value is ahead of what the backend confirmed.
    pub fn pending(&self) -> bool { self.value != self.confirmed }

    /// Whether star `n` is filled.
    pub fn filled(&self, n: u8) -> bool { n <= self.value && n > 0 }

    /// What a screen reader says. "Unrated" rather than "0 stars", which reads
    /// as a rating of zero — a judgement — instead of the absence of one.
    pub fn describe(&self) -> String {
        match self.value {
            0 => "Unrated".into(),
            1 => "1 star".into(),
            n => format!("{n} stars"),
        }
    }
}

// ── tags ────────────────────────────────────────────────────────────────────

/// One tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    /// `#rrggbb`, or empty when the user did not choose one.
    pub color: String,
    pub track_count: u32,
}

impl Tag {
    /// The tile colour. A tag without one gets a DERIVED default rather than a
    /// hole in the grid — derived from the name so two uncoloured tags do not
    /// become the same tile.
    pub fn tile_color(&self) -> [u8; 3] {
        if let Some(rgb) = parse_hex(&self.color) { return rgb; }
        // A cheap stable hash of the name into a hue-ish spread. Deterministic,
        // so a tag does not change colour between launches.
        let mut h: u32 = 2166136261;
        for b in self.name.as_bytes() {
            h ^= *b as u32;
            h = h.wrapping_mul(16777619);
        }
        // Kept mid-bright: fully saturated random colours are unreadable behind
        // white text, and near-black ones vanish into the grid.
        [(120 + (h & 0x5f)) as u8,
         (120 + ((h >> 8) & 0x5f)) as u8,
         (120 + ((h >> 16) & 0x5f)) as u8]
    }

    /// `Ambient · 12 tracks`.
    pub fn subtitle(&self) -> String {
        if self.track_count == 1 { "1 track".into() } else { format!("{} tracks", self.track_count) }
    }
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let t = s.trim().trim_start_matches('#');
    if t.len() != 6 { return None; }
    Some([u8::from_str_radix(&t[0..2], 16).ok()?,
          u8::from_str_radix(&t[2..4], 16).ok()?,
          u8::from_str_radix(&t[4..6], 16).ok()?])
}

/// The tag manager for one track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagEditor {
    pub track_id: Option<i64>,
    pub track_title: String,
    /// Tags on this track.
    attached: Vec<Tag>,
    /// Everything else in the library.
    available: Vec<Tag>,
    /// The new-tag input.
    pub draft_name: String,
    pub draft_color: String,
}

/// What the editor asks the host to do. The editor owns no storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagAction {
    Attach(i64, i64),
    Detach(i64, i64),
    Create { name: String, color: String },
}

impl TagEditor {
    /// `open(track)`.
    pub fn open(&mut self, track_id: i64, title: &str) {
        self.track_id = Some(track_id);
        self.track_title = title.to_string();
        self.attached.clear();
        self.available.clear();
        self.draft_name.clear();
        self.draft_color.clear();
    }

    pub fn is_open(&self) -> bool { self.track_id.is_some() }
    pub fn close(&mut self) { self.track_id = None; }

    /// Supply the two lists. Split here rather than by the caller so the
    /// partition cannot disagree with itself — a tag in both lists would show
    /// as attachable AND detachable at once.
    pub fn set_tags(&mut self, all: Vec<Tag>, attached_ids: &[i64]) {
        let (a, b): (Vec<Tag>, Vec<Tag>) = all.into_iter()
            .partition(|t| attached_ids.contains(&t.id));
        self.attached = a;
        self.available = b;
    }

    pub fn attached(&self) -> &[Tag] { &self.attached }
    pub fn available(&self) -> &[Tag] { &self.available }

    /// Tap an attached tag to detach it.
    pub fn detach(&self, tag_id: i64) -> Option<TagAction> {
        let t = self.track_id?;
        if !self.attached.iter().any(|x| x.id == tag_id) { return None; }
        Some(TagAction::Detach(t, tag_id))
    }

    /// Tap an available tag to attach it.
    pub fn attach(&self, tag_id: i64) -> Option<TagAction> {
        let t = self.track_id?;
        if !self.available.iter().any(|x| x.id == tag_id) { return None; }
        Some(TagAction::Attach(t, tag_id))
    }

    /// Create from the input row. Refuses a blank name — an unnamed tag cannot
    /// be found again by the only means the UI offers, which is reading it.
    pub fn create(&mut self) -> Option<TagAction> {
        let name = self.draft_name.trim().to_string();
        if name.is_empty() { return None; }
        // A tag whose name already exists is not created twice; the library
        // would then have two tiles the user cannot tell apart.
        if self.attached.iter().chain(self.available.iter())
            .any(|t| t.name.eq_ignore_ascii_case(&name)) {
            return None;
        }
        let color = self.draft_color.trim().to_string();
        self.draft_name.clear();
        self.draft_color.clear();
        Some(TagAction::Create { name, color })
    }
}

/// Draw the star row. Returns the value to write when one is chosen.
pub fn stars_ui(ui: &mut egui::Ui, r: &mut StarRating,
                c: &uikit_egui::PaletteColors) -> Option<u8> {
    let mut out = None;
    ui.horizontal(|ui| {
        for n in 1..=MAX_STARS {
            let glyph = if r.filled(n) { "★" } else { "☆" };
            let col = if r.filled(n) { c.accent } else { c.text_weak };
            let text = egui::RichText::new(glyph).color(col);
            if r.interactive {
                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                    out = r.tap(n);
                }
            } else {
                ui.label(text);
            }
        }
        // A pending write is shown, faintly. A control that looks settled while
        // a write is in flight cannot explain itself if the write is rejected.
        if r.pending() { ui.colored_label(c.text_weak, "·"); }
    });
    out
}

/// Draw the tag grid. Returns a tag id when one is tapped.
pub fn tags_ui(ui: &mut egui::Ui, tags: &[Tag],
               c: &uikit_egui::PaletteColors) -> Option<i64> {
    if tags.is_empty() {
        ui.colored_label(c.text_weak, "No tags yet — add one from a track's tag editor.");
        return None;
    }
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("tag_grid").show(ui, |ui| {
        for row in tags.chunks(2) {
            ui.horizontal(|ui| {
                for t in row {
                    let [r, g, b] = t.tile_color();
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width().min(150.0), 56.0),
                        egui::Sense::click());
                    ui.painter().rect_filled(rect, 6.0, egui::Color32::from_rgb(r, g, b));
                    ui.painter().text(rect.left_top() + egui::vec2(8.0, 8.0),
                        egui::Align2::LEFT_TOP, &t.name,
                        egui::FontId::proportional(13.0), egui::Color32::WHITE);
                    ui.painter().text(rect.left_bottom() + egui::vec2(8.0, -8.0),
                        egui::Align2::LEFT_BOTTOM, t.subtitle(),
                        egui::FontId::proportional(10.0), egui::Color32::from_white_alpha(180));
                    if resp.clicked() { picked = Some(t.id); }
                }
            });
        }
    });
    picked
}

/// Draw the tag editor sheet.
pub fn tag_editor_ui(ui: &mut egui::Ui, e: &mut TagEditor,
                     c: &uikit_egui::PaletteColors) -> Option<TagAction> {
    if !e.is_open() { return None; }
    let mut act = None;
    ui.strong(format!("Tags — {}", e.track_title));

    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut e.draft_name)
            .hint_text("New tag").desired_width(140.0));
        ui.add(egui::TextEdit::singleline(&mut e.draft_color)
            .hint_text("#colour").desired_width(70.0));
        if ui.button("Add").clicked() { act = e.create(); }
    });

    ui.colored_label(c.text_weak, "On this track — tap to remove");
    ui.horizontal_wrapped(|ui| {
        for t in e.attached() {
            if ui.selectable_label(true, &t.name).clicked() { act = e.detach(t.id); }
        }
        if e.attached().is_empty() { ui.colored_label(c.text_weak, "none"); }
    });

    ui.colored_label(c.text_weak, "Library — tap to add");
    ui.horizontal_wrapped(|ui| {
        for t in e.available() {
            if ui.selectable_label(false, &t.name).clicked() { act = e.attach(t.id); }
        }
    });
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(id: i64, name: &str, color: &str, n: u32) -> Tag {
        Tag { id, name: name.into(), color: color.into(), track_count: n }
    }

    #[test]
    fn tapping_a_star_sets_the_rating_immediately() {
        // A rating control that pauses before responding makes the user tap
        // again, and now they have rated it twice.
        let mut r = StarRating::new(1, 0);
        assert_eq!(r.tap(4), Some(4));
        assert_eq!(r.value, 4);
        assert!(r.pending(), "shown ahead of what the backend has confirmed");
    }

    #[test]
    fn tapping_the_current_star_clears_the_rating() {
        // The only way BACK to unrated. Without it, three stars is a decision
        // you cannot undo: every other tap sets some other rating and none of
        // them is "none".
        let mut r = StarRating::new(1, 3);
        assert_eq!(r.tap(3), Some(0));
        assert_eq!(r.value, 0);
    }

    #[test]
    fn a_confirmed_echo_settles_the_control() {
        let mut r = StarRating::new(1, 0);
        r.tap(4);
        assert!(r.reconcile(1, 4));
        assert!(!r.pending());
    }

    #[test]
    fn a_clamped_or_rejected_write_pulls_the_stars_back_to_the_truth() {
        // Optimism without reconciliation is a lie with a shorter lifespan.
        let mut r = StarRating::new(1, 2);
        r.tap(5);
        assert_eq!(r.value, 5);
        r.reconcile(1, 3);       // the backend said no, it is 3
        assert_eq!(r.value, 3);
        assert!(!r.pending());
    }

    #[test]
    fn an_echo_for_another_track_does_not_repaint_these_stars() {
        // The same stale-response race the browse detail pages and the QR
        // scanner both had, in a third costume.
        let mut r = StarRating::new(7, 4);
        assert!(!r.reconcile(9, 1));
        assert_eq!(r.value, 4);
    }

    #[test]
    fn a_read_only_rating_ignores_taps() {
        let mut r = StarRating::read_only(1, 3);
        assert_eq!(r.tap(5), None);
        assert_eq!(r.value, 3);
    }

    #[test]
    fn a_tap_outside_the_range_does_nothing() {
        let mut r = StarRating::new(1, 3);
        assert_eq!(r.tap(0), None);
        assert_eq!(r.tap(9), None);
        assert_eq!(r.value, 3);
    }

    #[test]
    fn a_rating_above_five_is_clamped_on_the_way_in() {
        assert_eq!(StarRating::new(1, 99).value, MAX_STARS);
        let mut r = StarRating::new(1, 0);
        r.reconcile(1, 200);
        assert_eq!(r.value, MAX_STARS);
    }

    #[test]
    fn the_filled_stars_are_the_ones_up_to_the_value() {
        let r = StarRating::new(1, 3);
        assert!(r.filled(1) && r.filled(3));
        assert!(!r.filled(4));
        assert!(!r.filled(0), "there is no zeroth star");
    }

    #[test]
    fn unrated_is_described_as_absence_not_as_a_score_of_zero() {
        // "0 stars" reads as a judgement; "Unrated" reads as the absence of one.
        assert_eq!(StarRating::new(1, 0).describe(), "Unrated");
        assert_eq!(StarRating::new(1, 1).describe(), "1 star");
        assert_eq!(StarRating::new(1, 4).describe(), "4 stars");
    }

    // ── tags ────────────────────────────────────────────────────────────────

    #[test]
    fn a_tag_with_a_colour_uses_it() {
        assert_eq!(tag(1, "Ambient", "#3366cc", 4).tile_color(), [0x33, 0x66, 0xcc]);
        assert_eq!(tag(1, "Ambient", "3366cc", 4).tile_color(), [0x33, 0x66, 0xcc],
                   "with or without the hash");
    }

    #[test]
    fn a_tag_without_a_colour_gets_a_derived_one_rather_than_a_hole() {
        let t = tag(1, "Ambient", "", 4);
        let c = t.tile_color();
        assert_ne!(c, [0, 0, 0]);
        // Derived from the NAME, so two uncoloured tags do not become the same
        // tile.
        assert_ne!(c, tag(2, "Focus", "", 1).tile_color());
    }

    #[test]
    fn a_derived_colour_is_stable_across_runs() {
        // A tag that changes colour between launches is a tag you cannot learn
        // to recognise.
        assert_eq!(tag(1, "Ambient", "", 4).tile_color(),
                   tag(1, "Ambient", "", 9).tile_color());
    }

    #[test]
    fn a_derived_colour_stays_readable() {
        // Fully saturated random colours are unreadable behind white text and
        // near-black ones vanish into the grid.
        for name in ["Ambient", "Focus", "Late night", "x", "zzzzzzzz"] {
            let [r, g, b] = tag(1, name, "", 1).tile_color();
            for ch in [r, g, b] {
                assert!(ch >= 120, "{name} produced a dark channel: {ch}");
            }
        }
    }

    #[test]
    fn a_malformed_colour_falls_back_rather_than_panicking() {
        for bad in ["#zzz", "#12345", "not a colour", "#"] {
            let c = tag(1, "Ambient", bad, 1).tile_color();
            assert!(c.iter().all(|v| *v >= 120), "{bad} should fall back");
        }
    }

    #[test]
    fn the_subtitle_is_pluralised() {
        assert_eq!(tag(1, "A", "", 1).subtitle(), "1 track");
        assert_eq!(tag(1, "A", "", 0).subtitle(), "0 tracks");
        assert_eq!(tag(1, "A", "", 12).subtitle(), "12 tracks");
    }

    // ── editor ──────────────────────────────────────────────────────────────

    fn editor() -> TagEditor {
        let mut e = TagEditor::default();
        e.open(42, "Opener");
        e.set_tags(vec![tag(1, "Ambient", "", 3), tag(2, "Focus", "", 5),
                        tag(3, "Late night", "", 1)], &[2]);
        e
    }

    #[test]
    fn the_two_lists_partition_the_library_and_cannot_overlap() {
        // A tag in both lists would show as attachable AND detachable at once.
        let e = editor();
        assert_eq!(e.attached().len(), 1);
        assert_eq!(e.available().len(), 2);
        for a in e.attached() {
            assert!(!e.available().iter().any(|b| b.id == a.id));
        }
    }

    #[test]
    fn attaching_and_detaching_name_the_track_they_belong_to() {
        let e = editor();
        assert_eq!(e.attach(1), Some(TagAction::Attach(42, 1)));
        assert_eq!(e.detach(2), Some(TagAction::Detach(42, 2)));
    }

    #[test]
    fn you_cannot_attach_something_already_attached() {
        let e = editor();
        assert_eq!(e.attach(2), None, "tag 2 is already on the track");
        assert_eq!(e.detach(1), None, "tag 1 is not");
    }

    #[test]
    fn nothing_happens_when_no_track_is_open() {
        let e = TagEditor::default();
        assert_eq!(e.attach(1), None);
        assert_eq!(e.detach(1), None);
    }

    #[test]
    fn a_blank_tag_name_is_refused() {
        // An unnamed tag cannot be found again by the only means the UI offers,
        // which is reading it.
        let mut e = editor();
        e.draft_name = "   ".into();
        assert_eq!(e.create(), None);
    }

    #[test]
    fn a_duplicate_tag_name_is_refused_whatever_its_case() {
        // Two tiles the user cannot tell apart.
        let mut e = editor();
        e.draft_name = "ambient".into();
        assert_eq!(e.create(), None);
        e.draft_name = "AMBIENT".into();
        assert_eq!(e.create(), None);
    }

    #[test]
    fn creating_a_tag_clears_the_input_so_it_is_not_added_twice() {
        let mut e = editor();
        e.draft_name = "  Drive  ".into();
        e.draft_color = "#ff0000".into();
        assert_eq!(e.create(),
                   Some(TagAction::Create { name: "Drive".into(), color: "#ff0000".into() }));
        assert!(e.draft_name.is_empty());
        assert!(e.draft_color.is_empty());
        assert_eq!(e.create(), None, "a second press adds nothing");
    }

    #[test]
    fn opening_a_track_clears_the_previous_ones_state() {
        // Otherwise the editor opens showing the last track's tags for a frame,
        // which reads as the wrong track opening.
        let mut e = editor();
        e.draft_name = "half typed".into();
        e.open(99, "Closer");
        assert!(e.attached().is_empty());
        assert!(e.available().is_empty());
        assert!(e.draft_name.is_empty());
        assert_eq!(e.track_id, Some(99));
    }
}
