//! workspaces_view — `WorkspacesPage`, ported.
//!
//! The Qt side renders this as **markdown pushed through `manpages::to_qml`**,
//! the same pipeline as sessions, recipes and bookmarks. That made sense when
//! every page was a document; it makes less sense here, because the workspaces
//! page is the one page in that set whose rows are *actionable* — you open a
//! workspace, you do not read about it.
//!
//! So the port keeps the data and the ordering exactly, and drops the markdown
//! round-trip: rows are values, and the view renders them. A string
//! intermediate would mean parsing our own output to know which row was tapped.
//!
//! ## Live state sits above the persisted list
//!
//! `LiveSession` exists because what is on disk is not what is happening. The
//! Qt version shows a "Current session" block first, and only when there are
//! **two or more tabs** — a single-tab session is not a workspace worth
//! showing, which matches how the tab strip hides itself. That rule is
//! preserved, because changing it would make the page disagree with the strip
//! beside it.

/// A workspace as the page needs it — a projection of `workspaces::Workspace`
/// rather than a copy of it, so the data layer stays the single owner.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceRow {
    pub name: String,
    pub speaker: String,
    pub browser_tabs: usize,
    pub terminal_sessions: usize,
    pub updated_ms: u64,
}

/// The live browser session, when there is one worth showing.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LiveSession {
    pub speaker_display: String,
    /// `(url, opened_ms)` per tab.
    pub tabs: Vec<(String, u64)>,
    pub active_tab: usize,
}

impl LiveSession {
    /// Should the "Current session" block appear?
    ///
    /// Two or more tabs, matching the tab strip's own visibility rule. A page
    /// that showed a one-tab session while the strip hid it would look like one
    /// of the two was broken.
    pub fn is_showable(&self) -> bool { self.tabs.len() >= 2 }

    /// Active index, clamped. A stale index arriving from the backend after a
    /// tab closed is ordinary, and indexing on it would panic in the draw loop
    /// — the worst place to find out.
    pub fn active(&self) -> usize {
        if self.tabs.is_empty() { 0 } else { self.active_tab.min(self.tabs.len() - 1) }
    }
}

/// The whole page.
#[derive(Clone, Debug, Default)]
pub struct WorkspacesView {
    pub live: Option<LiveSession>,
    rows: Vec<WorkspaceRow>,
    filter: String,
}

impl WorkspacesView {
    pub fn set_rows(&mut self, rows: Vec<WorkspaceRow>) { self.rows = rows; }
    pub fn set_filter(&mut self, f: &str) { self.filter = f.to_string(); }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }

    /// Rows in display order: **most recently updated first**, ties broken by
    /// name then speaker.
    ///
    /// The tie-break matters more than it looks. Workspaces saved in the same
    /// second are common — a batch save writes them all — and without a
    /// deterministic order the list reshuffles between frames, which on a touch
    /// screen means opening the wrong workspace.
    pub fn ordered(&self) -> Vec<&WorkspaceRow> {
        let needle = self.filter.trim().to_lowercase();
        let mut v: Vec<&WorkspaceRow> = self.rows.iter()
            .filter(|r| needle.is_empty()
                || r.name.to_lowercase().contains(&needle)
                || r.speaker.to_lowercase().contains(&needle))
            .collect();
        v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.speaker.cmp(&b.speaker)));
        v
    }

    /// Speakers present, for a cross-speaker grouping header.
    ///
    /// Sorted and deduplicated rather than in encounter order: this is a
    /// cross-speaker view, and a list whose headings move as rows arrive is
    /// harder to scan than one that is simply alphabetical.
    pub fn speakers(&self) -> Vec<String> {
        let mut s: Vec<String> = self.rows.iter().map(|r| r.speaker.clone()).collect();
        s.sort();
        s.dedup();
        s
    }
}

/// How a workspace's age reads.
///
/// Coarse on purpose. "3 days ago" is what a user needs to decide whether a
/// workspace is stale; a timestamp is what they would have to decode.
pub fn age_label(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=59 => "just now".into(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        86_400..=604_799 => format!("{}d ago", secs / 86_400),
        _ => format!("{}w ago", secs / 604_800),
    }
}

/// A tab's label: the host, or the whole URL when there is no host.
///
/// A column of full URLs is unreadable at phone width, and the host is what
/// distinguishes tabs in practice.
pub fn tab_label(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    if host.is_empty() { url.to_string() } else { host.to_string() }
}

/// Summary line for a row: what a user checks before opening.
pub fn row_summary(r: &WorkspaceRow) -> String {
    let tabs = match r.browser_tabs {
        0 => "no tabs".to_string(),
        1 => "1 tab".to_string(),
        n => format!("{n} tabs"),
    };
    let terms = match r.terminal_sessions {
        0 => "no terminals".to_string(),
        1 => "1 terminal".to_string(),
        n => format!("{n} terminals"),
    };
    format!("{tabs} · {terms}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, speaker: &str, tabs: usize, terms: usize, updated: u64) -> WorkspaceRow {
        WorkspaceRow { name: name.into(), speaker: speaker.into(),
                       browser_tabs: tabs, terminal_sessions: terms, updated_ms: updated }
    }

    #[test]
    fn a_single_tab_session_is_not_shown_matching_the_tab_strip() {
        // The page and the strip must agree, or one of them looks broken.
        let one = LiveSession { speaker_display: "Ada".into(),
                                tabs: vec![("https://a.com".into(), 0)], active_tab: 0 };
        assert!(!one.is_showable());
        let two = LiveSession { speaker_display: "Ada".into(),
                                tabs: vec![("https://a.com".into(), 0),
                                           ("https://b.com".into(), 0)], active_tab: 0 };
        assert!(two.is_showable());
    }

    #[test]
    fn a_stale_active_index_is_clamped_rather_than_panicking() {
        // A backend index arriving after a tab closed is ordinary, and indexing
        // on it would panic inside the draw loop — the worst place to find out.
        let s = LiveSession { speaker_display: "Ada".into(),
                              tabs: vec![("https://a.com".into(), 0)], active_tab: 7 };
        assert_eq!(s.active(), 0);
        assert_eq!(LiveSession::default().active(), 0, "no tabs at all");
    }

    #[test]
    fn rows_are_newest_first_and_stable_when_timestamps_tie() {
        // Batch saves write several workspaces in the same second. Without a
        // tie-break the list reshuffles between frames, which on a touch screen
        // means opening the wrong workspace.
        let mut v = WorkspacesView::default();
        v.set_rows(vec![row("beta", "Ada", 1, 1, 500),
                        row("alpha", "Ada", 1, 1, 500),
                        row("newest", "Bob", 1, 1, 900)]);
        let names: Vec<&str> = v.ordered().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["newest", "alpha", "beta"]);
        assert_eq!(v.ordered().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), names);
    }

    #[test]
    fn the_filter_matches_a_speaker_as_well_as_a_name() {
        // This is a CROSS-SPEAKER view, so finding "everything Bob saved" is as
        // natural a search as finding a workspace by name.
        let mut v = WorkspacesView::default();
        v.set_rows(vec![row("work", "Ada", 1, 1, 1), row("play", "Bob", 1, 1, 2)]);
        v.set_filter("bob");
        assert_eq!(v.ordered().len(), 1);
        assert_eq!(v.ordered()[0].name, "play");
        v.set_filter("work");
        assert_eq!(v.ordered()[0].speaker, "Ada");
    }

    #[test]
    fn an_empty_filter_shows_everything_and_whitespace_counts_as_empty() {
        // A stray space left by a phone keyboard must not empty the list.
        let mut v = WorkspacesView::default();
        v.set_rows(vec![row("a", "Ada", 1, 1, 1), row("b", "Bob", 1, 1, 2)]);
        v.set_filter("   ");
        assert_eq!(v.ordered().len(), 2);
    }

    #[test]
    fn speakers_are_alphabetical_and_deduplicated() {
        // Headings that move as rows arrive are harder to scan than headings
        // that are simply alphabetical.
        let mut v = WorkspacesView::default();
        v.set_rows(vec![row("x", "Bob", 1, 1, 1), row("y", "Ada", 1, 1, 2),
                        row("z", "Bob", 1, 1, 3)]);
        assert_eq!(v.speakers(), vec!["Ada".to_string(), "Bob".to_string()]);
    }

    #[test]
    fn counts_read_as_english_including_the_zero_and_singular_cases() {
        // "1 tabs" is the tell that nobody looked at an empty workspace.
        assert_eq!(row_summary(&row("w", "A", 0, 0, 0)), "no tabs · no terminals");
        assert_eq!(row_summary(&row("w", "A", 1, 1, 0)), "1 tab · 1 terminal");
        assert_eq!(row_summary(&row("w", "A", 4, 2, 0)), "4 tabs · 2 terminals");
    }

    #[test]
    fn age_is_coarse_because_a_timestamp_is_something_to_decode() {
        assert_eq!(age_label(1_000, 1_000), "just now");
        assert_eq!(age_label(600_000, 0), "10m ago");
        assert_eq!(age_label(7_200_000, 0), "2h ago");
        assert_eq!(age_label(259_200_000, 0), "3d ago");
        assert_eq!(age_label(1_209_600_000, 0), "2w ago");
    }

    #[test]
    fn a_clock_that_went_backwards_reads_as_just_now() {
        // Saturating rather than underflowing: an NTP correction must not make
        // a workspace saved seconds ago look 500 million weeks old.
        assert_eq!(age_label(0, 5_000), "just now");
    }

    #[test]
    fn a_tab_shows_its_host_because_full_urls_do_not_fit_a_phone() {
        assert_eq!(tab_label("https://example.com/a/very/long/path?q=1"), "example.com");
        assert_eq!(tab_label("http://localhost:8080/x"), "localhost:8080");
        assert_eq!(tab_label("about:blank"), "about:blank", "no host, so show it all");
        assert_eq!(tab_label(""), "");
    }

    #[test]
    fn an_empty_page_is_empty_rather_than_a_row_of_nothing() {
        let v = WorkspacesView::default();
        assert!(v.is_empty());
        assert!(v.ordered().is_empty());
        assert!(v.speakers().is_empty());
    }
}
