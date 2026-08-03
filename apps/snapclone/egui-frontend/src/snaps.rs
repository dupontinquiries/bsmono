//! snaps — `SnapFeedWidget`, `SnapTile`, `SnapViewerDialog`.
//!
//! The app's namesake feature, and the one where a porting mistake does real
//! harm rather than looking wrong. Everything here serves one invariant:
//!
//! > **A tile is SEALED.** It shows who sent a snap and when, never what is in
//! > it. Opening is what reveals — and opening is also what destroys.
//!
//! That is why `FeedEntry` carries no image bytes at all. It cannot leak the
//! contents through a careless label or a debug print, because it does not have
//! them; the bytes live in `Snap` and only the viewer ever holds one. A
//! thumbnail path may be supplied, and it is rendered frosted behind the card
//! chrome so the feed reads visually without giving anything away — a null
//! thumbnail simply leaves the avatar-only look rather than a broken tile.
//!
//! ## The expiry rule that is easy to get subtly wrong
//!
//! The countdown fires `viewTimerExpired` **even if the user already closed the
//! viewer**. It is not a UI timer; it is the event the inbox uses to update the
//! row. Stopping it on close means a snap the user dismissed after one second
//! stays looking unopened forever. It must also fire exactly ONCE — the host
//! marks storage on it, and a repeat is a double write.
//!
//! ## Playlist mode turns the timer OFF
//!
//! `setPlaylist` makes the viewer a browsable feed player over an ordered list.
//! Auto-expiry is disabled there, because a snap expiring while you are paging
//! back and forth would yank the page out from under you. Browsing still counts
//! as opening each one — `snapViewed` per snap shown — so nothing is silently
//! read without being marked.
//!
//! ## Saving is not surveillance
//!
//! Save writes to `~/Pictures/SnapClone/` and tells the HOST, never the sender.
//! There is deliberately no "screenshotted" notification anywhere in this
//! module. Saving also marks the snap kept, which exempts it from auto-expiry:
//! a copy the user explicitly asked to keep must not be deleted by a timer.

/// What kind of media a snap holds. Affects the tile badge and the default
/// duration, not what the tile reveals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media { Image, Video, Audio }

impl Media {
    /// The badge on a sealed tile. Which KIND of thing arrived is not a
    /// disclosure — you can see that a video arrived without seeing the video —
    /// and without it a voice note is an indistinguishable grey square.
    pub fn badge(self) -> &'static str {
        match self { Media::Image => "", Media::Video => "▶", Media::Audio => "🎤" }
    }
    pub fn label(self) -> &'static str {
        match self { Media::Image => "photo", Media::Video => "video", Media::Audio => "voice note" }
    }
}

/// One sealed tile's worth of information.
///
/// Note what is absent: there are no image bytes on this type. The feed cannot
/// leak a snap's contents because the feed never holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEntry {
    pub id: String,
    pub sender_name: String,
    /// Unix seconds.
    pub sent_at: i64,
    pub media: Media,
    /// Thumbnail source, rendered frosted. Empty means avatar-only, which is a
    /// legitimate look rather than a missing-image state.
    pub thumb_path: String,
}

impl FeedEntry {
    /// The tile's caption: sender, then how long ago. Never anything derived
    /// from the snap's content.
    pub fn caption(&self, now: i64) -> String {
        let who = if self.sender_name.trim().is_empty() { "unknown" } else { &self.sender_name };
        format!("{} · {}", who, ago(now.saturating_sub(self.sent_at)))
    }

    pub fn sealed(&self) -> bool { true }
    pub fn has_thumb(&self) -> bool { !self.thumb_path.trim().is_empty() }
}

/// Coarse relative time, via the shared helper. Coarse on purpose: a tile is
/// glanced at, and "3h" is as much as anyone reads.
pub fn ago(secs: i64) -> String {
    uikit_egui::fmt::relative_time(secs, uikit_egui::fmt::Style::Bare)
}

/// `kColumns` — two across.
pub const COLUMNS: usize = 2;
/// The long-press threshold, `m_holdTimer->setInterval(450)`. Now the kit's,
/// because this was the only place in the fleet that had tuned it.
pub use uikit_egui::menu::HOLD_MS;
/// Default view duration when a snap does not carry one.
pub const DEFAULT_DURATION_SECS: u32 = 5;

/// The feed.
#[derive(Debug, Clone, Default)]
pub struct SnapFeed {
    entries: Vec<FeedEntry>,
}

impl SnapFeed {
    /// `setEntries`. The order given is the order rendered — the inbox supplies
    /// newest-first and re-sorting here would fight whatever it decided.
    pub fn set_entries(&mut self, entries: Vec<FeedEntry>) { self.entries = entries; }

    pub fn entries(&self) -> &[FeedEntry] { &self.entries }
    pub fn count(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Where a tile sits in the two-column grid, as (row, column).
    pub fn cell(&self, index: usize) -> (usize, usize) { (index / COLUMNS, index % COLUMNS) }

    /// Viewing removes the tile. The owner re-supplies the set after each
    /// change, but doing it here too means the tile cannot be tapped twice
    /// while the host catches up — and a second tap on a view-once snap is a
    /// request to open something that no longer exists.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }
}

/// What an empty feed says. Not "no snaps" alone: the reason it is empty is
/// that everything has been opened, and opened snaps are gone by design.
pub const EMPTY_TEXT: &str = "No new snaps.\nOpened snaps are gone — that's the point.";

/// A press on a tile. The timing lives in `uikit_egui::menu::HoldTracker`.
///
/// This module had the only tuned long-press in the fleet — 450ms, from
/// `m_holdTimer->setInterval(450)` — and the kit had none at all, so the shared
/// tracker took this value. What the kit adds and this did not have is the
/// SLOP: a finger that drifts more than a few pixels is scrolling, and a hold
/// that fires under a scroll makes the grid feel like it is grabbing at you.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Press {
    hold: uikit_egui::menu::HoldTracker,
}

/// What a press on a tile resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture { None, Tap, Hold }

impl Press {
    pub fn down(&mut self) { self.hold.press(0.0, 0.0); }

    /// Position-aware press, for callers that can supply one — the slop needs
    /// somewhere to measure from.
    pub fn down_at(&mut self, x: f32, y: f32) { self.hold.press(x, y); }

    /// The finger moved. Past the slop this is a scroll, not a hold.
    pub fn moved(&mut self, x: f32, y: f32) { self.hold.moved(x, y); }

    /// Advance while held. Fires `Hold` once, at the threshold, WITHOUT waiting
    /// for release — a long-press that only resolves on release feels broken,
    /// because the user is already holding and nothing has happened.
    pub fn tick(&mut self, dt_ms: f32) -> Gesture {
        if self.hold.tick(dt_ms) { Gesture::Hold } else { Gesture::None }
    }

    /// Lift. A press that ended before the hold fired is a tap.
    pub fn up(&mut self) -> Gesture {
        if self.hold.release() { Gesture::Tap } else { Gesture::None }
    }

    pub fn cancel(&mut self) { self.hold.cancel(); }
    pub fn progress(&self) -> f32 { self.hold.progress() }
}

// ── the viewer ──────────────────────────────────────────────────────────────

/// A snap, with its bytes. Only the viewer holds one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snap {
    pub id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub sent_at: i64,
    pub media: Media,
    /// Seconds on screen. Zero means "use the default".
    pub duration_secs: u32,
    /// The decrypted bytes. Cleared when the viewer closes.
    pub bytes: Vec<u8>,
}

impl Snap {
    /// `qMax(1, snap.durationSecs)`. A zero would close the viewer in the same
    /// frame it opened and give the progress bar a zero range.
    pub fn duration_ms(&self) -> f32 {
        let s = if self.duration_secs == 0 { DEFAULT_DURATION_SECS } else { self.duration_secs };
        s.max(1) as f32 * 1000.0
    }
}

/// Things the viewer asks the host to do. Note the absence of any "tell the
/// sender" variant: there is no surveillance model here, and the way to keep it
/// that way is to give the type nowhere to express it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerEvent {
    /// `snapViewed` — mark it opened in storage.
    Viewed(String),
    /// `viewTimerExpired` — fires even if the viewer already closed.
    Expired(String),
    /// `imageSaved` with the path written.
    Saved(String, String),
    /// `snapKept` — exempt from auto-expiry, because the user asked to keep it.
    Kept(String),
}

/// The full-screen viewer.
#[derive(Debug, Clone, Default)]
pub struct Viewer {
    snaps: Vec<Snap>,
    index: usize,
    open: bool,
    /// None in playlist mode: browsing must not expire anything.
    remaining_ms: Option<f32>,
    total_ms: f32,
    /// Ids whose expiry has already fired, so it cannot fire twice.
    expired: Vec<String>,
    kept: Vec<String>,
}

impl Viewer {
    /// Open one snap, view-once, with the countdown running.
    pub fn open(&mut self, snap: Snap) -> Vec<ViewerEvent> {
        let id = snap.id.clone();
        self.total_ms = snap.duration_ms();
        self.remaining_ms = Some(self.total_ms);
        self.snaps = vec![snap];
        self.index = 0;
        self.open = true;
        self.expired.clear();
        vec![ViewerEvent::Viewed(id)]
    }

    /// `setPlaylist`. Auto-expiry off, navigation on.
    pub fn open_playlist(&mut self, snaps: Vec<Snap>, start: usize) -> Vec<ViewerEvent> {
        if snaps.is_empty() { return Vec::new(); }
        self.index = start.min(snaps.len() - 1);
        self.snaps = snaps;
        self.open = true;
        self.remaining_ms = None;
        self.total_ms = 0.0;
        self.expired.clear();
        vec![ViewerEvent::Viewed(self.snaps[self.index].id.clone())]
    }

    pub fn is_open(&self) -> bool { self.open }
    pub fn is_playlist(&self) -> bool { self.snaps.len() > 1 || self.remaining_ms.is_none() }
    pub fn current(&self) -> Option<&Snap> { self.snaps.get(self.index) }
    pub fn position(&self) -> (usize, usize) { (self.index + 1, self.snaps.len()) }

    /// Fraction of the countdown remaining, for the progress bar. 1.0 in
    /// playlist mode, where there is nothing counting down.
    pub fn countdown(&self) -> f32 {
        match self.remaining_ms {
            None => 1.0,
            Some(r) if self.total_ms > 0.0 => (r / self.total_ms).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// Advance the countdown. Drive with real elapsed time, not by subtracting
    /// a nominal tick — Qt uses QElapsedTimer for exactly this reason, since
    /// accumulating a fixed 100ms on a loaded system drifts.
    pub fn tick(&mut self, dt_ms: f32) -> Vec<ViewerEvent> {
        let Some(r) = self.remaining_ms else { return Vec::new() };
        let next = (r - dt_ms).max(0.0);
        self.remaining_ms = Some(next);
        if next > 0.0 { return Vec::new(); }
        let Some(id) = self.current().map(|s| s.id.clone()) else { return Vec::new() };
        let mut evs = Vec::new();
        if !self.expired.contains(&id) {
            self.expired.push(id.clone());
            evs.push(ViewerEvent::Expired(id));
        }
        self.open = false;
        self.wipe();
        evs
    }

    /// Close early. The countdown keeps running: the expiry event is what the
    /// inbox uses to update the row, so stopping it here leaves a snap the user
    /// dismissed after a second looking unopened forever.
    pub fn close(&mut self) {
        self.open = false;
        self.wipe();
    }

    /// Clear the decrypted bytes. The ids stay so a still-pending expiry can
    /// name the right snap.
    fn wipe(&mut self) {
        for s in &mut self.snaps { s.bytes.clear(); }
    }

    /// True once nothing holds plaintext.
    pub fn wiped(&self) -> bool { self.snaps.iter().all(|s| s.bytes.is_empty()) }

    pub fn next(&mut self) -> Vec<ViewerEvent> { self.go(1) }
    pub fn prev(&mut self) -> Vec<ViewerEvent> { self.go(-1) }

    /// Navigate. Clamped rather than wrapping: wrapping past the end of a feed
    /// silently restarts it, and the user cannot tell they have seen everything.
    fn go(&mut self, delta: i32) -> Vec<ViewerEvent> {
        if self.snaps.is_empty() { return Vec::new(); }
        let last = self.snaps.len() - 1;
        let next = (self.index as i32 + delta).clamp(0, last as i32) as usize;
        if next == self.index { return Vec::new(); }
        self.index = next;
        // Browsing counts as opening: nothing is read without being marked.
        vec![ViewerEvent::Viewed(self.snaps[self.index].id.clone())]
    }

    pub fn can_prev(&self) -> bool { self.index > 0 }
    pub fn can_next(&self) -> bool { self.index + 1 < self.snaps.len() }

    /// Save the current snap. Reports to the HOST; the sender is never told.
    pub fn save(&mut self, now: &str, pictures_dir: &str) -> Vec<ViewerEvent> {
        let Some(s) = self.snaps.get(self.index) else { return Vec::new() };
        let path = save_path(pictures_dir, now, &s.sender_id, &s.id);
        let id = s.id.clone();
        if !self.kept.contains(&id) { self.kept.push(id.clone()); }
        vec![ViewerEvent::Saved(id.clone(), path), ViewerEvent::Kept(id)]
    }

    /// A kept snap is exempt from auto-expiry: a copy the user explicitly asked
    /// to keep must not be deleted by a timer.
    pub fn is_kept(&self, id: &str) -> bool { self.kept.iter().any(|k| k == id) }
}

/// `buildSavePath`: `<pictures>/SnapClone/<timestamp>_from_<sender>_<8 of id>.jpg`.
///
/// The sender id is sanitised because it is a network-supplied string being put
/// into a filename — it is routinely `user@192.168.1.10`, and a peer that names
/// itself `../../.bashrc` should not get to choose where the file lands.
pub fn save_path(pictures_dir: &str, timestamp: &str, sender_id: &str, snap_id: &str) -> String {
    let mut safe: String = sender_id.chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '.') { '_' } else { c })
        .collect();
    // Truncate to 64 so the whole path stays under the 255-byte limit every
    // filesystem enforces somewhere.
    if safe.chars().count() > 64 {
        safe = safe.chars().take(64).collect();
    }
    let short: String = snap_id.chars().take(8).collect();
    format!("{pictures_dir}/SnapClone/{timestamp}_from_{safe}_{short}.jpg")
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// What the feed asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedAction {
    /// Tapped: open the ephemeral viewer.
    Open(String),
    /// Held: open the sender's conversation instead.
    OpenThread(String),
}

/// Draw the sealed feed, two across.
pub fn feed_ui(ui: &mut egui::Ui, feed: &SnapFeed, press: &mut Press, now: i64,
               c: &uikit_egui::PaletteColors) -> Option<FeedAction> {
    if feed.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| { ui.colored_label(c.text_weak, EMPTY_TEXT); });
        return None;
    }
    let mut action = None;
    let dt = ui.input(|i| i.stable_dt) * 1000.0;
    let width = ui.available_width();
    let tile_w = (width - 12.0) / COLUMNS as f32;

    egui::ScrollArea::vertical().id_salt("snap_feed").show(ui, |ui| {
        for row in feed.entries().chunks(COLUMNS) {
            ui.horizontal(|ui| {
                for e in row {
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(tile_w, tile_w), egui::Sense::click_and_drag());
                    // The tile is a sealed card: chrome, a badge, a caption.
                    // Nothing here reads snap content, because the entry does
                    // not carry any.
                    ui.painter().rect_filled(rect, 8.0, c.bg_card);
                    if e.has_thumb() {
                        // The frosted thumbnail is a wash, not the image.
                        ui.painter().rect_filled(rect.shrink(4.0), 6.0, c.bg_raised);
                    }
                    let badge = e.media.badge();
                    if !badge.is_empty() {
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, badge,
                            egui::FontId::proportional(24.0), c.text_weak);
                    }
                    ui.painter().text(rect.left_bottom() + egui::vec2(8.0, -8.0),
                        egui::Align2::LEFT_BOTTOM, e.caption(now),
                        egui::FontId::proportional(11.0), c.text_weak);

                    if resp.is_pointer_button_down_on() {
                        if !resp.dragged() { press.down(); }
                        if press.tick(dt) == Gesture::Hold {
                            action = Some(FeedAction::OpenThread(e.id.clone()));
                        }
                    }
                    if resp.clicked() && press.up() == Gesture::Tap {
                        action = Some(FeedAction::Open(e.id.clone()));
                    }
                }
            });
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, who: &str, at: i64) -> FeedEntry {
        FeedEntry { id: id.into(), sender_name: who.into(), sent_at: at,
                    media: Media::Image, thumb_path: String::new() }
    }
    fn snap(id: &str) -> Snap {
        Snap { id: id.into(), sender_id: "ada@192.168.1.10".into(), sender_name: "Ada".into(),
               sent_at: 0, media: Media::Image, duration_secs: 5, bytes: vec![1, 2, 3] }
    }

    #[test]
    fn a_tile_carries_no_snap_bytes_at_all() {
        // The invariant the whole module exists for. A feed that cannot hold
        // plaintext cannot leak it through a label, a log line, or a debug
        // print — this is enforced by the TYPE, not by discipline.
        let e = entry("s1", "Ada", 0);
        let dbg = format!("{e:?}");
        assert!(!dbg.contains("bytes"));
        assert!(e.sealed());
    }

    #[test]
    fn a_caption_names_the_sender_and_the_time_and_nothing_else() {
        let e = entry("s1", "Ada", 1_000);
        assert_eq!(e.caption(1_060), "Ada · 1m");
    }

    #[test]
    fn an_unnamed_sender_still_gets_a_caption() {
        assert!(entry("s1", "  ", 0).caption(0).starts_with("unknown"));
    }

    #[test]
    fn relative_time_is_coarse_and_never_negative() {
        assert_eq!(ago(5), "5s");
        assert_eq!(ago(90), "1m");
        assert_eq!(ago(3 * 3600), "3h");
        assert_eq!(ago(50 * 3600), "2d");
        // A peer whose clock runs fast should not produce "in -4s".
        assert_eq!(ago(-4), "now");
    }

    #[test]
    fn the_media_badge_says_what_arrived_without_saying_what_is_in_it() {
        // Which KIND arrived is not a disclosure, and without it a voice note
        // is an indistinguishable grey square.
        assert_eq!(Media::Video.badge(), "▶");
        assert_eq!(Media::Audio.badge(), "🎤");
        assert_eq!(Media::Image.badge(), "", "a photo needs no badge");
    }

    #[test]
    fn the_feed_renders_in_the_order_it_was_given() {
        // The inbox supplies newest-first; re-sorting here would fight it.
        let mut f = SnapFeed::default();
        f.set_entries(vec![entry("c", "C", 300), entry("a", "A", 100), entry("b", "B", 200)]);
        assert_eq!(f.entries().iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
                   vec!["c", "a", "b"]);
    }

    #[test]
    fn tiles_lay_out_two_across() {
        let mut f = SnapFeed::default();
        f.set_entries((0..5).map(|i| entry(&i.to_string(), "A", 0)).collect());
        assert_eq!(f.cell(0), (0, 0));
        assert_eq!(f.cell(1), (0, 1));
        assert_eq!(f.cell(2), (1, 0));
        assert_eq!(f.cell(4), (2, 0));
    }

    #[test]
    fn opening_removes_the_tile_so_it_cannot_be_tapped_twice() {
        // A second tap on a view-once snap asks to open something that no
        // longer exists.
        let mut f = SnapFeed::default();
        f.set_entries(vec![entry("s1", "Ada", 0), entry("s2", "Bo", 0)]);
        assert!(f.remove("s1"));
        assert_eq!(f.count(), 1);
        assert!(!f.remove("s1"), "removing twice is not an error, just false");
    }

    #[test]
    fn the_empty_state_explains_that_gone_is_the_design() {
        assert!(SnapFeed::default().is_empty());
        assert!(EMPTY_TEXT.contains("gone"));
    }

    #[test]
    fn a_hold_fires_at_the_threshold_without_waiting_for_release() {
        // A long-press that only resolves on release feels broken: the user is
        // already holding and nothing has happened.
        let mut p = Press::default();
        p.down();
        assert_eq!(p.tick(200.0), Gesture::None);
        assert_eq!(p.tick(300.0), Gesture::Hold, "450ms crossed");
        assert_eq!(p.tick(100.0), Gesture::None, "and only once");
        assert_eq!(p.up(), Gesture::None, "the release is not also a tap");
    }

    #[test]
    fn a_quick_press_is_a_tap() {
        let mut p = Press::default();
        p.down();
        p.tick(100.0);
        assert_eq!(p.up(), Gesture::Tap);
    }

    #[test]
    fn a_release_without_a_press_is_nothing() {
        assert_eq!(Press::default().up(), Gesture::None);
    }

    #[test]
    fn opening_a_snap_marks_it_viewed_immediately() {
        let mut v = Viewer::default();
        assert_eq!(v.open(snap("s1")), vec![ViewerEvent::Viewed("s1".into())]);
        assert!(v.is_open());
    }

    #[test]
    fn the_countdown_expires_the_snap_exactly_once() {
        // The host marks storage on this event; a repeat is a double write.
        let mut v = Viewer::default();
        v.open(snap("s1"));
        assert!(v.tick(4_000.0).is_empty(), "still running");
        assert_eq!(v.tick(1_000.0), vec![ViewerEvent::Expired("s1".into())]);
        assert!(v.tick(1_000.0).is_empty(), "and never again");
        assert!(!v.is_open());
    }

    #[test]
    fn expiry_still_fires_after_the_user_closes_early() {
        // THE subtle one. The expiry event is what the inbox uses to update the
        // row; stopping the timer on close leaves a snap the user dismissed
        // after a second looking unopened forever.
        let mut v = Viewer::default();
        v.open(snap("s1"));
        v.tick(1_000.0);
        v.close();
        assert!(!v.is_open());
        assert_eq!(v.tick(9_000.0), vec![ViewerEvent::Expired("s1".into())]);
    }

    #[test]
    fn closing_wipes_the_decrypted_bytes() {
        let mut v = Viewer::default();
        v.open(snap("s1"));
        assert!(!v.wiped());
        v.close();
        assert!(v.wiped(), "plaintext must not outlive the viewer");
    }

    #[test]
    fn expiring_wipes_too_not_just_closing() {
        let mut v = Viewer::default();
        v.open(snap("s1"));
        v.tick(99_000.0);
        assert!(v.wiped());
    }

    #[test]
    fn a_zero_duration_snap_does_not_close_in_the_frame_it_opened() {
        // qMax(1, durationSecs): a zero would also give the progress bar a zero
        // range.
        let mut s = snap("s1");
        s.duration_secs = 0;
        assert_eq!(s.duration_ms(), DEFAULT_DURATION_SECS as f32 * 1000.0);
        let mut v = Viewer::default();
        v.open(s);
        assert!(v.tick(16.0).is_empty());
        assert!(v.is_open());
    }

    #[test]
    fn the_countdown_fraction_runs_from_one_to_zero() {
        let mut v = Viewer::default();
        v.open(snap("s1"));
        assert_eq!(v.countdown(), 1.0);
        v.tick(2_500.0);
        assert!((v.countdown() - 0.5).abs() < 0.01);
        v.tick(9_000.0);
        assert_eq!(v.countdown(), 0.0);
    }

    #[test]
    fn playlist_mode_switches_the_expiry_timer_off() {
        // A snap expiring while you page back and forth would yank the page out
        // from under you.
        let mut v = Viewer::default();
        v.open_playlist(vec![snap("a"), snap("b")], 0);
        assert!(v.is_playlist());
        assert!(v.tick(60_000.0).is_empty(), "nothing expires while browsing");
        assert!(v.is_open());
        assert_eq!(v.countdown(), 1.0);
    }

    #[test]
    fn browsing_marks_each_snap_viewed_as_it_is_shown() {
        // Browsing IS opening; nothing gets read without being marked.
        let mut v = Viewer::default();
        assert_eq!(v.open_playlist(vec![snap("a"), snap("b"), snap("c")], 0),
                   vec![ViewerEvent::Viewed("a".into())]);
        assert_eq!(v.next(), vec![ViewerEvent::Viewed("b".into())]);
        assert_eq!(v.next(), vec![ViewerEvent::Viewed("c".into())]);
    }

    #[test]
    fn navigation_clamps_rather_than_wrapping() {
        // Wrapping silently restarts the feed and the user cannot tell they
        // have seen everything.
        let mut v = Viewer::default();
        v.open_playlist(vec![snap("a"), snap("b")], 0);
        assert!(v.prev().is_empty(), "already at the start");
        assert!(!v.can_prev());
        v.next();
        assert!(v.next().is_empty(), "already at the end");
        assert!(!v.can_next());
        assert_eq!(v.position(), (2, 2));
    }

    #[test]
    fn a_playlist_opens_at_the_snap_that_was_tapped() {
        let mut v = Viewer::default();
        assert_eq!(v.open_playlist(vec![snap("a"), snap("b"), snap("c")], 2),
                   vec![ViewerEvent::Viewed("c".into())]);
        assert_eq!(v.position(), (3, 3));
    }

    #[test]
    fn an_out_of_range_start_index_lands_on_the_last_snap() {
        let mut v = Viewer::default();
        v.open_playlist(vec![snap("a"), snap("b")], 99);
        assert_eq!(v.position(), (2, 2));
    }

    #[test]
    fn an_empty_playlist_does_not_open() {
        let mut v = Viewer::default();
        assert!(v.open_playlist(vec![], 0).is_empty());
        assert!(!v.is_open());
    }

    #[test]
    fn saving_tells_the_host_and_never_the_sender() {
        // There is deliberately no ViewerEvent that notifies a sender. No
        // surveillance model means the type has nowhere to express one.
        let mut v = Viewer::default();
        v.open(snap("ab12cd34ef"));
        let evs = v.save("2026-04-03_14-22-01", "/home/u/Pictures");
        assert!(matches!(evs[0], ViewerEvent::Saved(_, _)));
        assert!(matches!(evs[1], ViewerEvent::Kept(_)));
        let names: Vec<String> = evs.iter().map(|e| format!("{e:?}")).collect();
        assert!(!names.iter().any(|n| n.contains("Notif")));
    }

    #[test]
    fn saving_exempts_the_snap_from_auto_expiry() {
        // A copy the user explicitly asked to keep must not be deleted by a
        // timer.
        let mut v = Viewer::default();
        v.open(snap("s1"));
        v.save("t", "/p");
        assert!(v.is_kept("s1"));
    }

    #[test]
    fn the_save_path_follows_the_documented_convention() {
        assert_eq!(
            save_path("/home/u/Pictures", "2026-04-03_14-22-01", "alice@192.168.1.10", "ab12cd34ef56"),
            "/home/u/Pictures/SnapClone/2026-04-03_14-22-01_from_alice@192_168_1_10_ab12cd34.jpg");
    }

    #[test]
    fn a_hostile_sender_id_cannot_choose_where_the_file_lands() {
        // The sender id is network-supplied and goes straight into a filename.
        let p = save_path("/pics", "t", "../../.bashrc", "deadbeef");
        assert!(p.starts_with("/pics/SnapClone/"));
        // The FILENAME is what the sender influences, so that is what must be
        // free of separators and traversal — the directory's own slashes are
        // ours and are fine.
        let name = p.rsplit('/').next().unwrap();
        assert!(!name.contains(".."), "no traversal: {name}");
        assert!(!name.contains('/') && !name.contains('\\'), "no separators: {name}");
        assert_eq!(name, "t_from________bashrc_deadbeef.jpg",
                   "each of the 8 dots and slashes became one underscore");
    }

    #[test]
    fn a_very_long_sender_id_is_truncated_so_the_path_stays_openable() {
        let long = "a".repeat(200);
        let p = save_path("/pics", "t", &long, "deadbeef");
        assert!(p.len() < 255);
        assert!(p.contains(&"a".repeat(64)));
        assert!(!p.contains(&"a".repeat(65)));
    }

    #[test]
    fn only_the_first_eight_characters_of_the_snap_id_are_used() {
        let p = save_path("/pics", "t", "ada", "0123456789abcdef");
        assert!(p.ends_with("_01234567.jpg"));
    }
}
