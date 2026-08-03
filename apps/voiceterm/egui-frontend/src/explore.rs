//! explore — `WalkPage` and `SitePage`: the two views over voiceterm's
//! browsing cores.
//!
//! Both engines are included, not re-implemented (`#[path]`, as with the table
//! engine, the row cache and the extractors):
//!
//! * `session_walk` clusters visited pages into sessions, orders the last one,
//!   and does the word-smush. Its own header says the QML side "consumes a wire
//!   string built from these; the camera animation + slider live in main.qml" —
//!   so the PLAYER is exactly what did not come across.
//! * `sitemap` discovers a domain's pages from history, sitemap.xml, robots.txt
//!   and link extraction, each carrying where it came from and what is known
//!   about it.
//!
//! ## The walk is a player, and a player needs to be pausable
//!
//! It flies card to card on a timer. The thing that makes that bearable rather
//! than infuriating is that any interaction PAUSES it — scrubbing, stepping,
//! or just grabbing the slider. An auto-advancing tour that keeps moving while
//! you are trying to read one card is a tour you close.
//!
//! Stepping by hand pauses too, and does not resume on its own. Once the user
//! has taken the wheel, taking it back after a timeout would be the tour
//! deciding it knows better.
//!
//! ## The smush is a fallback that arrives first
//!
//! Word-smush is a cheap no-LLM salient-word view that works instantly. The
//! preview crossfades to it rather than waiting: a local LLM that times out
//! would otherwise leave the card blank for the whole time it is on screen, and
//! the card is only on screen for a few seconds.
//!
//! ## A site map's rows are grouped by what is KNOWN, not by path
//!
//! "Visited" and "404" are different kinds of fact about a page, and sorting
//! them together by path buries the two rows that matter — the page you have
//! actually read, and the link that is broken — among a hundred you have merely
//! heard of.

use crate::session_walk::WalkPage;

/// What is known about a discovered page. Mirrors `sitemap::PageStatus`.
///
/// Mirrored rather than included, and the reason is specific: `sitemap`'s link
/// extractor uses the `scraper` crate, which this frontend does not depend on,
/// and adding html5ever for one function to a build that is already ~97%
/// dependencies is a poor trade. These two types are a dozen lines and carry no
/// behaviour; the ENGINE is not duplicated, only the shape it reports in. If
/// the extractor is ever wanted here, uikit-web's tolerant parser is already in
/// this workspace and would do the job without the new dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStatus {
    /// Found in sitemap/robots/links but not visited.
    Known,
    Visited,
    /// Probed and got a 200.
    Probed,
    NotFound,
    Error,
}

/// A discovered page on a site. Mirrors `sitemap::SitePage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitePage {
    pub path: String,
    pub title: String,
    /// "visited" / "sitemap" / "robots" / "link" / "probe".
    pub source: &'static str,
    pub status: PageStatus,
}

/// How long each card holds before the walk advances.
pub const CARD_MS: f32 = 4200.0;
/// The preview→smush crossfade.
pub const CROSSFADE_MS: f32 = 380.0;

/// The guided walk player.
#[derive(Debug, Clone, Default)]
pub struct Walk {
    pages: Vec<WalkPage>,
    index: usize,
    /// Milliseconds the current card has been showing.
    elapsed: f32,
    pub playing: bool,
}

impl Walk {
    /// Load the ordered pages of the last session.
    pub fn load(&mut self, pages: Vec<WalkPage>) {
        self.pages = pages;
        self.index = 0;
        self.elapsed = 0.0;
        // Does not auto-play. A walk that starts moving the instant it loads
        // gives the user no chance to decide whether they wanted it.
        self.playing = false;
    }

    pub fn pages(&self) -> &[WalkPage] { &self.pages }
    pub fn len(&self) -> usize { self.pages.len() }
    pub fn is_empty(&self) -> bool { self.pages.is_empty() }
    pub fn index(&self) -> usize { self.index }
    pub fn current(&self) -> Option<&WalkPage> { self.pages.get(self.index) }

    /// Position as `3 / 12`, so the user knows how much is left.
    pub fn position(&self) -> (usize, usize) {
        if self.is_empty() { return (0, 0); }
        (self.index + 1, self.pages.len())
    }

    pub fn play(&mut self) { if !self.is_empty() { self.playing = true; } }

    /// Any interaction pauses. An auto-advancing tour that keeps moving while
    /// you are trying to read one card is a tour you close.
    pub fn pause(&mut self) { self.playing = false; }

    /// Step by hand. Pauses, and deliberately does not resume: once the user
    /// has taken the wheel, taking it back would be the tour deciding it knows
    /// better.
    pub fn step(&mut self, delta: i32) -> bool {
        if self.is_empty() { return false; }
        self.pause();
        let last = self.pages.len() as i32 - 1;
        let next = (self.index as i32 + delta).clamp(0, last) as usize;
        if next == self.index { return false; }
        self.index = next;
        self.elapsed = 0.0;
        true
    }

    /// Scrub to a card. Also pauses.
    pub fn seek(&mut self, i: usize) -> bool {
        if i >= self.pages.len() { return false; }
        self.pause();
        self.index = i;
        self.elapsed = 0.0;
        true
    }

    /// Advance the timer. Returns true on the frame it moves to a new card.
    pub fn tick(&mut self, dt_ms: f32) -> bool {
        if !self.playing || self.is_empty() { return false; }
        self.elapsed += dt_ms;
        if self.elapsed < CARD_MS { return false; }
        self.elapsed = 0.0;
        if self.index + 1 >= self.pages.len() {
            // The end is a stop, not a loop. A tour that silently restarts
            // leaves the user unsure whether they have seen everything.
            self.playing = false;
            return false;
        }
        self.index += 1;
        true
    }

    /// How far the card is through its hold, 0..=1 — the progress ring.
    pub fn card_t(&self) -> f32 {
        if !self.playing { return 0.0; }
        (self.elapsed / CARD_MS).clamp(0.0, 1.0)
    }

    /// The preview→smush crossfade, 0 = preview, 1 = smush.
    ///
    /// Runs from the moment the card appears rather than waiting for anything:
    /// a local LLM that times out would otherwise leave the card blank for the
    /// whole few seconds it is on screen.
    pub fn smush_mix(&self) -> f32 { (self.elapsed / CROSSFADE_MS).clamp(0.0, 1.0) }

    pub fn at_end(&self) -> bool { !self.is_empty() && self.index + 1 == self.pages.len() }
}

// ── site map ────────────────────────────────────────────────────────────────

/// How a site map row is grouped.
///
/// Ordered so the rows that carry the most information come first: a page you
/// have read, then one that is broken, then ones merely heard of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group { Visited, Broken, Probed, Known }

impl Group {
    pub fn of(status: PageStatus) -> Group {
        match status {
            PageStatus::Visited => Group::Visited,
            PageStatus::NotFound | PageStatus::Error => Group::Broken,
            PageStatus::Probed => Group::Probed,
            PageStatus::Known => Group::Known,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Visited => "Visited",
            Group::Broken => "Broken",
            Group::Probed => "Reachable",
            Group::Known => "Known",
        }
    }
}

/// A row in the site map view.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteRow {
    pub path: String,
    pub title: String,
    pub source: &'static str,
    pub group: Group,
}

impl SiteRow {
    pub fn of(p: &SitePage) -> Self {
        SiteRow {
            // A path is never blank in practice, but "/" is the root and would
            // otherwise render as an empty row.
            path: if p.path.trim().is_empty() { "/".into() } else { p.path.clone() },
            // An untitled page shows its path rather than nothing: the path is
            // what the user would recognise anyway.
            title: if p.title.trim().is_empty() { p.path.clone() } else { p.title.clone() },
            source: p.source,
            group: Group::of(p.status),
        }
    }
}

/// Group and order rows for display.
///
/// Grouped by what is KNOWN rather than sorted by path: "visited" and "404" are
/// different kinds of fact, and sorting them together buries the two rows that
/// matter among a hundred you have merely heard of. Within a group, path order,
/// because that is how a site is shaped.
pub fn group_rows(pages: &[SitePage]) -> Vec<(Group, Vec<SiteRow>)> {
    let mut rows: Vec<SiteRow> = pages.iter().map(SiteRow::of).collect();
    rows.sort_by(|a, b| a.group.cmp(&b.group).then(a.path.cmp(&b.path)));
    let mut out: Vec<(Group, Vec<SiteRow>)> = Vec::new();
    for r in rows {
        match out.last_mut() {
            Some((g, v)) if *g == r.group => v.push(r),
            _ => out.push((r.group, vec![r])),
        }
    }
    out
}

/// Draw the walk player. Returns a page index when the user picks one.
pub fn walk_ui(ui: &mut egui::Ui, w: &mut Walk,
               c: &uikit_egui::PaletteColors) -> Option<usize> {
    w.tick(ui.input(|i| i.stable_dt) * 1000.0);
    if w.is_empty() {
        ui.colored_label(c.text_weak, "No pages in the last session to walk.");
        return None;
    }
    let mut picked = None;
    let (n, total) = w.position();
    ui.horizontal(|ui| {
        if ui.button(if w.playing { "pause" } else { "play" }).clicked() {
            if w.playing { w.pause(); } else { w.play(); }
        }
        if ui.button("‹").clicked() { w.step(-1); }
        if ui.button("›").clicked() { w.step(1); }
        ui.colored_label(c.text_weak, format!("{n} / {total}"));
        // The hold ring: how long this card has left.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(60.0, 4.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, c.bg_raised);
        let mut fill = rect;
        fill.set_width(rect.width() * w.card_t());
        ui.painter().rect_filled(fill, 2.0, c.accent);
    });
    // Scrubbing pauses, like every other interaction.
    let mut idx = w.index();
    if ui.add(egui::Slider::new(&mut idx, 0..=w.len().saturating_sub(1))
        .show_value(false)).changed() {
        w.seek(idx);
        picked = Some(idx);
    }
    picked
}

/// Draw the site map. Returns a path when a row is tapped.
pub fn sitemap_ui(ui: &mut egui::Ui, groups: &[(Group, Vec<SiteRow>)],
                  c: &uikit_egui::PaletteColors) -> Option<String> {
    if groups.is_empty() {
        ui.colored_label(c.text_weak, "Nothing discovered for this site yet.");
        return None;
    }
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("sitemap").show(ui, |ui| {
        for (g, rows) in groups {
            ui.horizontal(|ui| {
                let col = match g { Group::Broken => c.danger,
                                    Group::Visited => c.accent,
                                    _ => c.text_weak };
                ui.colored_label(col, format!("{}  ({})", g.label(), rows.len()));
            });
            for r in rows {
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(&r.title).frame(false)).clicked() {
                        picked = Some(r.path.clone());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Where it came from, because "known" from robots.txt
                        // and "known" from a link are different confidences.
                        ui.colored_label(c.text_weak, r.source);
                    });
                });
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str) -> WalkPage {
        WalkPage { id: id.to_string(), title: id.to_string(), domain: "x".into(),
                   url: format!("http://x/{id}"),
                   preview: "some preview text here".into(), created_at: 0 }
    }

    fn walk() -> Walk {
        let mut w = Walk::default();
        w.load(vec![page("a"), page("b"), page("c")]);
        w
    }

    #[test]
    fn a_walk_does_not_start_moving_the_instant_it_loads() {
        // Otherwise the user gets no chance to decide whether they wanted it.
        let w = walk();
        assert!(!w.playing);
        assert_eq!(w.position(), (1, 3));
    }

    #[test]
    fn playing_advances_after_the_card_hold() {
        let mut w = walk();
        w.play();
        assert!(!w.tick(CARD_MS - 1.0));
        assert!(w.tick(2.0));
        assert_eq!(w.index(), 1);
    }

    #[test]
    fn stepping_by_hand_pauses_and_does_not_resume() {
        // Once the user has taken the wheel, taking it back would be the tour
        // deciding it knows better.
        let mut w = walk();
        w.play();
        assert!(w.step(1));
        assert!(!w.playing);
        w.tick(CARD_MS * 3.0);
        assert_eq!(w.index(), 1, "it stayed where the user put it");
    }

    #[test]
    fn scrubbing_pauses_too() {
        let mut w = walk();
        w.play();
        assert!(w.seek(2));
        assert!(!w.playing);
        assert_eq!(w.index(), 2);
    }

    #[test]
    fn stepping_past_the_ends_is_refused_rather_than_wrapping() {
        // Wrapping in a guided tour makes it impossible to tell you have
        // reached the end.
        let mut w = walk();
        assert!(!w.step(-1));
        assert_eq!(w.index(), 0);
        w.seek(2);
        assert!(!w.step(1));
        assert_eq!(w.index(), 2);
    }

    #[test]
    fn the_walk_stops_at_the_end_rather_than_looping() {
        // A tour that silently restarts leaves the user unsure whether they
        // have seen everything.
        let mut w = walk();
        w.seek(2);
        w.play();
        assert!(!w.tick(CARD_MS + 1.0));
        assert!(!w.playing);
        assert!(w.at_end());
        assert_eq!(w.index(), 2);
    }

    #[test]
    fn the_hold_ring_only_runs_while_playing() {
        // A ring that keeps filling while paused says the card is about to
        // change when it is not.
        let mut w = walk();
        w.play();
        w.tick(CARD_MS / 2.0);
        assert!(w.card_t() > 0.4 && w.card_t() < 0.6);
        w.pause();
        assert_eq!(w.card_t(), 0.0);
    }

    #[test]
    fn the_smush_crossfade_starts_immediately_rather_than_waiting() {
        // A local LLM that times out would otherwise leave the card blank for
        // the whole few seconds it is on screen.
        let mut w = walk();
        w.play();
        assert_eq!(w.smush_mix(), 0.0);
        w.tick(CROSSFADE_MS / 2.0);
        assert!(w.smush_mix() > 0.4 && w.smush_mix() < 0.6);
        w.tick(CROSSFADE_MS);
        assert_eq!(w.smush_mix(), 1.0);
    }

    #[test]
    fn moving_to_a_new_card_restarts_its_crossfade() {
        let mut w = walk();
        w.play();
        w.tick(CARD_MS + 1.0);
        assert_eq!(w.smush_mix(), 0.0, "the new card starts on its preview");
    }

    #[test]
    fn an_empty_walk_is_harmless() {
        let mut w = Walk::default();
        w.load(vec![]);
        assert!(w.is_empty());
        assert_eq!(w.position(), (0, 0));
        w.play();
        assert!(!w.playing, "there is nothing to play");
        assert!(!w.tick(CARD_MS * 10.0));
        assert!(!w.step(1));
        assert!(!w.seek(0));
    }

    #[test]
    fn loading_a_new_session_returns_to_the_first_card() {
        let mut w = walk();
        w.seek(2);
        w.load(vec![page("x"), page("y")]);
        assert_eq!(w.index(), 0);
        assert_eq!(w.position(), (1, 2));
    }

    // ── site map ────────────────────────────────────────────────────────────

    fn sp(path: &str, title: &str, source: &'static str, status: PageStatus) -> SitePage {
        SitePage { path: path.into(), title: title.into(), source, status }
    }

    #[test]
    fn rows_are_grouped_by_what_is_known_not_sorted_by_path() {
        // "Visited" and "404" are different kinds of fact; sorting them
        // together buries the two rows that matter.
        let pages = vec![
            sp("/a", "A", "sitemap", PageStatus::Known),
            sp("/b", "B", "visited", PageStatus::Visited),
            sp("/c", "C", "probe", PageStatus::NotFound),
        ];
        let g = group_rows(&pages);
        assert_eq!(g[0].0, Group::Visited);
        assert_eq!(g[1].0, Group::Broken);
        assert_eq!(g[2].0, Group::Known);
    }

    #[test]
    fn both_kinds_of_failure_group_together_as_broken() {
        // A 404 and a connection error are the same thing to someone deciding
        // whether to click.
        let pages = vec![sp("/a", "A", "probe", PageStatus::NotFound),
                         sp("/b", "B", "probe", PageStatus::Error)];
        let g = group_rows(&pages);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].0, Group::Broken);
        assert_eq!(g[0].1.len(), 2);
    }

    #[test]
    fn within_a_group_rows_are_in_path_order_because_that_is_how_a_site_is_shaped() {
        let pages = vec![sp("/z", "Z", "visited", PageStatus::Visited),
                         sp("/a", "A", "visited", PageStatus::Visited)];
        let g = group_rows(&pages);
        assert_eq!(g[0].1.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), vec!["/a", "/z"]);
    }

    #[test]
    fn an_untitled_page_shows_its_path() {
        // The path is what the user would recognise anyway; a blank row is
        // unclickable.
        let r = SiteRow::of(&sp("/docs/intro", "  ", "link", PageStatus::Known));
        assert_eq!(r.title, "/docs/intro");
    }

    #[test]
    fn a_blank_path_renders_as_the_root() {
        let r = SiteRow::of(&sp("", "Home", "visited", PageStatus::Visited));
        assert_eq!(r.path, "/");
    }

    #[test]
    fn the_source_is_kept_because_known_from_robots_is_not_known_from_a_link() {
        // Different confidences, and the user is the one who should weigh them.
        let r = SiteRow::of(&sp("/a", "A", "robots", PageStatus::Known));
        assert_eq!(r.source, "robots");
    }

    #[test]
    fn an_empty_site_produces_no_groups() {
        assert!(group_rows(&[]).is_empty());
    }

    #[test]
    fn the_group_order_puts_the_most_informative_first() {
        assert!(Group::Visited < Group::Broken);
        assert!(Group::Broken < Group::Probed);
        assert!(Group::Probed < Group::Known);
    }
}
