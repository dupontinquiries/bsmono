//! queue — `QueuePage`, `NowPlayingBar`, `TrackRow`.
//!
//! The browse pages ported earlier could open an album and say "play all", and
//! there was nowhere for the tracks to go. This is that somewhere.
//!
//! ## A queue entry is not a track
//!
//! Every entry carries a `queue_id` distinct from the track's own id, because
//! the same track can sit in the queue more than once — you queued it, then
//! queued the album containing it. Removing "that one" has to mean the entry
//! you tapped, not every copy. Keying removal on the track id silently deletes
//! all of them, and the user's only evidence is that the queue got shorter than
//! expected.
//!
//! ## The head is drawn differently because it is a different thing
//!
//! Position 0 is what plays next, and the page renders it larger with a pulsing
//! dot. Everything below is a list; the head is a commitment. A uniform list
//! makes you count rows to find out what happens when the current track ends.
//!
//! ## Four ways to add, two of which do not exist yet
//!
//! `Stack` appends and never interrupts — the default, and the safe one.
//! `Replace` clears and plays now. `Sync` starts a track time-aligned over the
//! current one and `Under` drops it beneath at the playhead; both need the
//! layered player and are marked with a dot in the Qt UI, which reports status
//! when they are chosen rather than pretending.
//!
//! Reproduced with the readiness flag intact. A mode picker that offers four
//! options and silently does two of them wrong is worse than one that says
//! which two are coming — the user attributes the failure to the app rather
//! than to a feature that has not landed.

/// One entry in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    /// Identifies this ENTRY. Two copies of one track have different ids.
    pub queue_id: u64,
    pub track_id: i64,
    pub title: String,
    pub artist: String,
}

impl QueueEntry {
    pub fn label(&self) -> String {
        let t = if self.title.trim().is_empty() { format!("track {}", self.track_id) }
                else { self.title.clone() };
        if self.artist.trim().is_empty() { t } else { format!("{} — {}", t, self.artist) }
    }
}

/// How a newly-added track behaves relative to what is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnqueueMode {
    /// Append. Never interrupts — the default, and the safe one.
    #[default]
    Stack,
    /// Clear the queue and play this now.
    Replace,
    /// Start it time-aligned over the current track. Needs the layered player.
    Sync,
    /// Drop it underneath at the current playhead. Needs the layered player.
    Under,
}

impl EnqueueMode {
    pub const ALL: [EnqueueMode; 4] =
        [EnqueueMode::Stack, EnqueueMode::Replace, EnqueueMode::Sync, EnqueueMode::Under];

    pub fn tag(self) -> &'static str {
        match self {
            EnqueueMode::Stack => "stack",
            EnqueueMode::Replace => "replace",
            EnqueueMode::Sync => "sync",
            EnqueueMode::Under => "under",
        }
    }

    pub fn from_tag(s: &str) -> EnqueueMode {
        match s {
            "replace" => EnqueueMode::Replace,
            "sync" => EnqueueMode::Sync,
            "under" => EnqueueMode::Under,
            // An unknown mode falls back to the one that cannot destroy a
            // queue. Defaulting to Replace on a typo would wipe what is playing.
            _ => EnqueueMode::Stack,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EnqueueMode::Stack => "Stack",
            EnqueueMode::Replace => "Replace",
            EnqueueMode::Sync => "Sync",
            EnqueueMode::Under => "Under",
        }
    }

    /// Whether the player can actually do this yet.
    pub fn ready(self) -> bool {
        matches!(self, EnqueueMode::Stack | EnqueueMode::Replace)
    }

    /// The Qt UI marks the unimplemented modes with a dot rather than hiding
    /// them — the roadmap is visible and the failure is attributable.
    pub fn marker(self) -> &'static str { if self.ready() { "" } else { " ·" } }

    /// What to report when an unready mode is chosen.
    pub fn unready_note(self) -> Option<&'static str> {
        if self.ready() { return None; }
        Some(match self {
            EnqueueMode::Sync => "Sync needs the layered player — queued as Stack",
            _ => "Under needs the layered player — queued as Stack",
        })
    }
}

/// The play queue.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
    entries: Vec<QueueEntry>,
    next_id: u64,
    pub mode: EnqueueMode,
}

impl Queue {
    pub fn entries(&self) -> &[QueueEntry] { &self.entries }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// What plays next. Drawn larger, because the head is a commitment and the
    /// rest is a list.
    pub fn up_next(&self) -> Option<&QueueEntry> { self.entries.first() }

    /// Add a track under the current mode. Returns any note the UI should show.
    ///
    /// An unready mode degrades to Stack and SAYS SO, rather than silently
    /// doing something else — the user needs to know the difference between
    /// "the app ignored me" and "that mode is not built yet".
    pub fn add(&mut self, track_id: i64, title: &str, artist: &str) -> Option<&'static str> {
        let note = self.mode.unready_note();
        let effective = if self.mode.ready() { self.mode } else { EnqueueMode::Stack };
        if effective == EnqueueMode::Replace { self.entries.clear(); }
        self.next_id += 1;
        self.entries.push(QueueEntry {
            queue_id: self.next_id, track_id,
            title: title.to_string(), artist: artist.to_string(),
        });
        note
    }

    /// Add several in order — what "play all" on an album does.
    pub fn add_all(&mut self, tracks: &[(i64, String, String)]) {
        for (i, (id, title, artist)) in tracks.iter().enumerate() {
            // Only the FIRST honours Replace. Otherwise queueing an album under
            // Replace clears the queue on every track and leaves exactly one.
            if i == 1 && self.mode == EnqueueMode::Replace { self.mode = EnqueueMode::Stack; }
            self.add(*id, title, artist);
        }
        self.mode = EnqueueMode::Stack;
    }

    /// Remove one ENTRY, by its queue id.
    pub fn remove(&mut self, queue_id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.queue_id != queue_id);
        self.entries.len() != before
    }

    /// `queue_clear`.
    pub fn clear(&mut self) { self.entries.clear(); }

    /// Tapping a row means "play now" — jump the queue to it, dropping what was
    /// ahead. The user chose this explicitly; keeping the skipped tracks would
    /// make the queue grow every time someone changed their mind.
    pub fn play_now(&mut self, queue_id: u64) -> Option<QueueEntry> {
        let i = self.entries.iter().position(|e| e.queue_id == queue_id)?;
        let entry = self.entries[i].clone();
        self.entries.drain(..=i);
        Some(entry)
    }

    /// Pop the head, when a track finishes on its own.
    pub fn advance(&mut self) -> Option<QueueEntry> {
        if self.entries.is_empty() { return None; }
        Some(self.entries.remove(0))
    }
}

/// What the now-playing strip shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    /// Set for line-in and radio, where there is no track to name.
    pub source: String,
    pub playing: bool,
}

impl NowPlaying {
    /// The strip's two lines.
    ///
    /// A source with no track shows the SOURCE name alone — "Line in" — rather
    /// than an empty title over an empty artist, which reads as a bug rather
    /// than as an input with nothing to say about itself.
    pub fn lines(&self) -> (String, String) {
        if self.title.trim().is_empty() && !self.source.trim().is_empty() {
            return (self.source.clone(), String::new());
        }
        if self.title.trim().is_empty() { return ("Nothing playing".into(), String::new()); }
        (self.title.clone(), self.artist.clone())
    }

    /// The little equalizer animates only while playing — a quiet sign of life,
    /// and a still one beside a paused track would be a lie about it.
    pub fn show_equalizer(&self) -> bool { self.playing && !self.title.trim().is_empty() }

    /// Bar heights for the equalizer, 0..=1.
    ///
    /// The three bars are phase-OFFSET taps on one shared oscillator — three
    /// bars rising in unison is a progress bar, not an equalizer. `Breath::at`
    /// exists for exactly this.
    pub fn equalizer_bars(&self, breath: &uikit_egui::motion::Breath) -> [f32; 3] {
        if !self.show_equalizer() { return [0.0; 3]; }
        [breath.at(0.0), breath.at(0.33), breath.at(0.66)]
    }
}

/// What the page asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAction {
    PlayNow(u64),
    Remove(u64),
    Clear,
    SetMode(EnqueueMode),
}

/// Draw the queue page.
pub fn queue_ui(ui: &mut egui::Ui, q: &Queue,
                c: &uikit_egui::PaletteColors) -> Option<QueueAction> {
    let mut act = None;
    ui.horizontal(|ui| {
        ui.strong("Queue");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("clear").clicked() { act = Some(QueueAction::Clear); }
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(c.text_weak, "ADD AS");
        for m in EnqueueMode::ALL {
            let label = format!("{}{}", m.label(), m.marker());
            let r = ui.selectable_label(q.mode == m, label);
            if let Some(note) = m.unready_note() { r.clone().on_hover_text(note); }
            if r.clicked() { act = Some(QueueAction::SetMode(m)); }
        }
    });
    ui.separator();
    if q.is_empty() {
        ui.colored_label(c.text_weak, "Nothing queued.");
        return act;
    }
    egui::ScrollArea::vertical().id_salt("queue").show(ui, |ui| {
        for (i, e) in q.entries().iter().enumerate() {
            ui.horizontal(|ui| {
                // The head is a commitment; the rest is a list.
                if i == 0 {
                    ui.colored_label(c.accent, "●");
                    if ui.add(egui::Button::new(egui::RichText::new(e.label()).strong())
                        .frame(false)).clicked() {
                        act = Some(QueueAction::PlayNow(e.queue_id));
                    }
                } else if ui.add(egui::Button::new(
                    egui::RichText::new(e.label()).color(c.text_weak)).frame(false)).clicked() {
                    act = Some(QueueAction::PlayNow(e.queue_id));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").clicked() { act = Some(QueueAction::Remove(e.queue_id)); }
                });
            });
        }
    });
    act
}

/// Draw the now-playing strip.
pub fn now_playing_ui(ui: &mut egui::Ui, n: &NowPlaying,
                      breath: &uikit_egui::motion::Breath,
                      c: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        if n.show_equalizer() {
            for h in n.equalizer_bars(breath) {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, 14.0), egui::Sense::hover());
                let mut bar = rect;
                bar.set_top(rect.bottom() - rect.height() * h);
                ui.painter().rect_filled(bar, 1.0, c.accent);
            }
            ui.add_space(4.0);
        }
        let (a, b) = n.lines();
        ui.colored_label(c.text, a);
        if !b.is_empty() { ui.colored_label(c.text_weak, b); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued() -> Queue {
        let mut q = Queue::default();
        q.add(1, "One", "Ada");
        q.add(2, "Two", "Bo");
        q.add(3, "Three", "Cy");
        q
    }

    #[test]
    fn the_same_track_queued_twice_can_be_removed_one_copy_at_a_time() {
        // THE reason a queue entry is not a track. You queued it, then queued
        // the album containing it; removing "that one" must mean the entry you
        // tapped. Keying on the track id deletes every copy and the user's only
        // evidence is a queue shorter than expected.
        let mut q = Queue::default();
        q.add(7, "Same", "Ada");
        q.add(7, "Same", "Ada");
        assert_eq!(q.len(), 2);
        let first = q.entries()[0].queue_id;
        assert!(q.remove(first));
        assert_eq!(q.len(), 1);
        assert_eq!(q.entries()[0].track_id, 7, "the other copy survives");
    }

    #[test]
    fn queue_ids_are_unique_even_for_identical_tracks() {
        let mut q = Queue::default();
        q.add(7, "Same", "Ada");
        q.add(7, "Same", "Ada");
        assert_ne!(q.entries()[0].queue_id, q.entries()[1].queue_id);
    }

    #[test]
    fn stack_appends_and_never_interrupts() {
        let q = queued();
        assert_eq!(q.mode, EnqueueMode::Stack, "the safe default");
        assert_eq!(q.entries().iter().map(|e| e.track_id).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn replace_clears_the_queue_and_leaves_only_the_new_track() {
        let mut q = queued();
        q.mode = EnqueueMode::Replace;
        q.add(9, "New", "Dee");
        assert_eq!(q.len(), 1);
        assert_eq!(q.entries()[0].track_id, 9);
    }

    #[test]
    fn queueing_an_album_under_replace_clears_once_not_once_per_track() {
        // Otherwise the queue is cleared on every track and exactly one
        // survives — an album that vanishes as you add it.
        let mut q = queued();
        q.mode = EnqueueMode::Replace;
        q.add_all(&[(10, "A".into(), "X".into()),
                    (11, "B".into(), "X".into()),
                    (12, "C".into(), "X".into())]);
        assert_eq!(q.entries().iter().map(|e| e.track_id).collect::<Vec<_>>(), vec![10, 11, 12]);
    }

    #[test]
    fn an_unready_mode_degrades_to_stack_and_says_so() {
        // A picker that offers four options and silently does two of them wrong
        // is worse than one that says which two are coming: the user blames the
        // app rather than a feature that has not landed.
        let mut q = queued();
        q.mode = EnqueueMode::Sync;
        let note = q.add(9, "New", "Dee").expect("a note");
        assert!(note.contains("Sync"));
        assert!(note.contains("Stack"), "it says what happened instead");
        assert_eq!(q.len(), 4, "appended, not replaced");
    }

    #[test]
    fn the_unimplemented_modes_are_marked_rather_than_hidden() {
        // The roadmap stays visible and the failure stays attributable.
        assert!(EnqueueMode::Stack.ready() && EnqueueMode::Replace.ready());
        assert!(!EnqueueMode::Sync.ready() && !EnqueueMode::Under.ready());
        assert_eq!(EnqueueMode::Stack.marker(), "");
        assert_eq!(EnqueueMode::Sync.marker(), " ·");
        assert!(EnqueueMode::Under.unready_note().is_some());
        assert!(EnqueueMode::Stack.unready_note().is_none());
    }

    #[test]
    fn an_unknown_mode_tag_falls_back_to_the_one_that_cannot_destroy_a_queue() {
        // Defaulting to Replace on a typo would wipe what is playing.
        assert_eq!(EnqueueMode::from_tag("nonsense"), EnqueueMode::Stack);
        for m in EnqueueMode::ALL { assert_eq!(EnqueueMode::from_tag(m.tag()), m); }
    }

    #[test]
    fn the_head_of_the_queue_is_what_plays_next() {
        let q = queued();
        assert_eq!(q.up_next().unwrap().track_id, 1);
        assert!(Queue::default().up_next().is_none());
    }

    #[test]
    fn playing_a_row_now_drops_everything_ahead_of_it() {
        // The user chose this explicitly. Keeping the skipped tracks would make
        // the queue grow every time someone changed their mind.
        let mut q = queued();
        let third = q.entries()[2].queue_id;
        let played = q.play_now(third).expect("that row");
        assert_eq!(played.track_id, 3);
        assert!(q.is_empty(), "nothing was left behind it either");
    }

    #[test]
    fn playing_the_middle_row_keeps_what_came_after() {
        let mut q = queued();
        let second = q.entries()[1].queue_id;
        q.play_now(second);
        assert_eq!(q.entries().iter().map(|e| e.track_id).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn playing_a_row_that_is_gone_does_nothing() {
        let mut q = queued();
        assert!(q.play_now(9999).is_none());
        assert_eq!(q.len(), 3, "and does not disturb the queue");
    }

    #[test]
    fn advancing_pops_the_head_only() {
        let mut q = queued();
        assert_eq!(q.advance().unwrap().track_id, 1);
        assert_eq!(q.len(), 2);
        q.clear();
        assert!(q.advance().is_none(), "an empty queue advances to nothing");
    }

    #[test]
    fn removing_something_that_is_not_there_is_reported_not_pretended() {
        let mut q = queued();
        assert!(!q.remove(9999));
    }

    #[test]
    fn an_entry_with_no_title_still_has_a_label() {
        let e = QueueEntry { queue_id: 1, track_id: 42, title: "  ".into(), artist: "".into() };
        assert_eq!(e.label(), "track 42");
    }

    // ── now playing ─────────────────────────────────────────────────────────

    #[test]
    fn a_source_with_no_track_shows_the_source_name() {
        // An empty title over an empty artist reads as a bug rather than as an
        // input with nothing to say about itself.
        let n = NowPlaying { source: "Line in".into(), playing: true, ..Default::default() };
        assert_eq!(n.lines(), ("Line in".into(), String::new()));
    }

    #[test]
    fn nothing_playing_says_so() {
        assert_eq!(NowPlaying::default().lines(), ("Nothing playing".into(), String::new()));
    }

    #[test]
    fn a_track_shows_its_title_and_artist() {
        let n = NowPlaying { title: "One".into(), artist: "Ada".into(),
                             playing: true, ..Default::default() };
        assert_eq!(n.lines(), ("One".into(), "Ada".into()));
    }

    #[test]
    fn the_equalizer_animates_only_while_playing() {
        // A still equalizer beside a paused track would be a lie about it.
        let mut n = NowPlaying { title: "One".into(), playing: true, ..Default::default() };
        assert!(n.show_equalizer());
        n.playing = false;
        assert!(!n.show_equalizer());
        let b = uikit_egui::motion::Breath::new(1000.0, 0.35, 0.8);
        assert_eq!(n.equalizer_bars(&b), [0.0; 3]);
    }

    #[test]
    fn a_line_in_source_gets_no_equalizer_because_there_is_no_track() {
        let n = NowPlaying { source: "Radio".into(), playing: true, ..Default::default() };
        assert!(!n.show_equalizer());
    }

    #[test]
    fn the_equalizer_bars_stay_in_range_and_do_not_move_together() {
        // Three bars rising and falling in unison is a progress bar, not an
        // equalizer.
        let n = NowPlaying { title: "One".into(), playing: true, ..Default::default() };
        let mut breath = uikit_egui::motion::Breath::new(1000.0, 0.35, 0.8);
        let mut ever_differed = false;
        for _ in 0..100 {
            breath.tick(20.0);
            let b = n.equalizer_bars(&breath);
            for h in b { assert!((0.0..=1.0).contains(&h), "bar out of range: {h}"); }
            if (b[0] - b[1]).abs() > 0.05 { ever_differed = true; }
        }
        assert!(ever_differed);
    }
}
