//! browser — the web surface, which had zero mentions in the port.
//!
//! voiceterm's browser is not a viewport with a URL bar. Its point is
//! `web_interact::detect_widgets`, which scans a page for the things you can
//! *act on* — login forms with their CSRF tokens, chat widgets and their API
//! endpoints, OAuth links — so a voice command can fill a form rather than
//! describing where to click.
//!
//! That reframes what the port owes. A browser you drive by voice needs the
//! **action list** to be the primary surface, not the page.
//!
//! ## The rule that has to hold before anything else
//!
//! A detected login form carries a CSRF token, and submitting without it fails
//! in a way that looks like a wrong password. So a form missing its token is
//! reported as **not submittable** rather than tried and failed — the user
//! retyping a correct password three times is the worst possible outcome here.
//!
//! ## Ordering
//!
//! Actions are ranked by what a person is most likely to want: a login form
//! beats a chat widget beats a bare API endpoint. Within a rank, page order is
//! kept, because the first form on a page is usually the real one and the second
//! is a newsletter signup.

#[path = "../../src/ad_filter.rs"]
#[allow(dead_code)]
pub mod ad_filter;

pub use ad_filter::{FilterMode, FilterStats};

/// What kind of thing was found. Mirrors `web_interact::WebWidget` without
/// mounting it: that module pulls in `ureq` for form submission, and a view
/// does not make network calls.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Login { action: String, method: String, has_csrf: bool },
    Chat { provider: String, endpoint: String },
    Api { url: String, method: String, auth: String, description: String },
    OAuth { provider: String, url: String },
}

impl Action {
    /// How likely a person wants this. Lower sorts first.
    fn rank(&self) -> u8 {
        match self {
            Action::Login { .. } => 0,
            Action::OAuth { .. } => 1,
            Action::Chat { .. } => 2,
            Action::Api { .. } => 3,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Action::Login { action, .. } => format!("Sign in ({action})"),
            Action::OAuth { provider, .. } => format!("Sign in with {provider}"),
            Action::Chat { provider, .. } => format!("Chat ({provider})"),
            Action::Api { method, url, .. } => format!("{method} {url}"),
        }
    }

    /// Can this be acted on right now?
    ///
    /// A login form without its CSRF token submits and fails in a way that looks
    /// like a wrong password — so it is reported as unusable rather than tried.
    /// A user retyping a correct password three times is the worst outcome here.
    pub fn actionable(&self) -> bool {
        match self {
            Action::Login { has_csrf, method, .. } =>
                *has_csrf || method.eq_ignore_ascii_case("get"),
            Action::Api { auth, .. } => auth != "bearer",   // no token to offer yet
            _ => true,
        }
    }

    /// Why not, for the row that explains itself rather than being greyed out.
    pub fn blocked_reason(&self) -> Option<&'static str> {
        if self.actionable() { return None; }
        Some(match self {
            Action::Login { .. } => "this form needs a CSRF token the page did not expose",
            Action::Api { .. } => "needs a bearer token",
            _ => "unavailable",
        })
    }

    /// Is this destructive enough to confirm first? An API endpoint that is not
    /// a GET can change something, and a voice command should not silently POST.
    pub fn needs_confirmation(&self) -> bool {
        matches!(self, Action::Api { method, .. } if !method.eq_ignore_ascii_case("get"))
    }
}

/// The browser page's state.
#[derive(Debug, Clone, Default)]
pub struct Browser {
    /// Ad and tracker filtering. Mounted from the app's own `ad_filter` rather
    /// than reimplemented — its `classify_tracking_wrapper` exempts semantic
    /// tags (article, section, header) on the grounds that they "carry meaning
    /// we shouldn't override", and a second filter that did not know that would
    /// strip real content while claiming to remove ads.
    pub filter_mode: FilterMode,
    pub filter_stats: FilterStats,
    /// Saved URLs. The star reads from here rather than from a flag on the
    /// page, so it stays correct when the same URL is reached twice by
    /// different routes.
    pub bookmarks: crate::bookmarks_view::Bookmarks,
    pub url: String,
    pub title: String,
    pub actions: Vec<Action>,
    pub loading: bool,
    /// Set when a page loaded but nothing was found, which is different from
    /// not having loaded — an empty action list on a blank page is expected and
    /// on a login page is a bug worth seeing.
    pub scanned: bool,
}

impl Browser {
    /// Actions in the order a person wants them, page order kept within a rank
    /// because the first form on a page is usually the real one and the second
    /// is a newsletter signup.
    pub fn ranked(&self) -> Vec<&Action> {
        let mut v: Vec<(usize, &Action)> = self.actions.iter().enumerate().collect();
        v.sort_by_key(|(i, a)| (a.rank(), *i));
        v.into_iter().map(|(_, a)| a).collect()
    }

    pub fn actionable(&self) -> Vec<&Action> {
        self.ranked().into_iter().filter(|a| a.actionable()).collect()
    }

    /// What to say when there is nothing to do.
    pub fn empty_reason(&self) -> Option<&'static str> {
        if !self.actions.is_empty() { return None; }
        if self.loading { return Some("loading…"); }
        if !self.scanned { return Some("nothing loaded"); }
        Some("nothing to act on here")
    }

    /// What the filter did on this page, for the row that reports it.
    ///
    /// Reported rather than silent: a filter that removes things without saying
    /// so is indistinguishable from a broken page, and the first thing anyone
    /// does when a site looks wrong is turn the filter off — so it has to be
    /// obvious that it was on.
    pub fn filter_summary(&self) -> Option<String> {
        if self.filter_mode == FilterMode::Off { return None; }
        Some(self.filter_stats.summary())
    }

    /// Is the current page saved? Drives the star's filled state.
    pub fn is_bookmarked(&self) -> bool { self.bookmarks.is_saved(&self.url) }

    /// Toggle the star. Returns true when it is now saved.
    ///
    /// The page TITLE is passed rather than a derived fallback, and the merge
    /// rule protects a user-edited one — so starring a page twice cannot rename
    /// a bookmark someone named.
    pub fn toggle_bookmark(&mut self, now_ms: u64) -> bool {
        if self.is_bookmarked() {
            self.bookmarks.remove(&self.url);
            false
        } else {
            self.bookmarks.save(&self.url, &self.title, &[], now_ms);
            true
        }
    }

    pub fn load(&mut self, url: &str) {
        self.url = url.to_string();
        // Per-page, not cumulative: "412 trackers blocked" across a session
        // tells you nothing about the page in front of you.
        self.filter_stats = FilterStats::default();
        self.loading = true;
        // Cleared on load, not on arrival: leaving the previous page's login
        // form visible while a new page loads invites acting on the wrong site,
        // which for a login form is a credential sent to a stranger.
        self.actions.clear();
        self.scanned = false;
        self.title.clear();
    }

    pub fn loaded(&mut self, title: &str, actions: Vec<Action>) {
        self.loading = false;
        self.scanned = true;
        self.title = title.to_string();
        self.actions = actions;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(csrf: bool) -> Action {
        Action::Login { action: "/session".into(), method: "POST".into(), has_csrf: csrf }
    }

    #[test]
    fn a_login_form_without_a_csrf_token_is_not_offered() {
        // Submitting fails in a way that looks like a wrong password, and a user
        // retyping a correct password three times is the worst outcome here.
        assert!(!login(false).actionable());
        assert!(login(true).actionable());
        assert!(login(false).blocked_reason().unwrap().contains("CSRF"));
        assert_eq!(login(true).blocked_reason(), None);
    }

    #[test]
    fn a_get_form_needs_no_token() {
        // A GET login is a search box in disguise; requiring CSRF there would
        // hide a usable action.
        let g = Action::Login { action: "/find".into(), method: "GET".into(), has_csrf: false };
        assert!(g.actionable());
    }

    #[test]
    fn actions_are_ranked_by_what_a_person_wants() {
        let b = Browser {
            actions: vec![
                Action::Api { url: "/v1/x".into(), method: "GET".into(),
                              auth: "none".into(), description: "".into() },
                Action::Chat { provider: "intercom".into(), endpoint: "/c".into() },
                login(true),
            ],
            ..Default::default()
        };
        let labels: Vec<String> = b.ranked().iter().map(|a| a.label()).collect();
        assert!(labels[0].starts_with("Sign in"), "{labels:?}");
        assert!(labels[1].starts_with("Chat"));
    }

    #[test]
    fn page_order_is_kept_within_a_rank() {
        // The first form on a page is usually the real one; the second is a
        // newsletter signup.
        let b = Browser {
            actions: vec![
                Action::Login { action: "/real".into(), method: "POST".into(), has_csrf: true },
                Action::Login { action: "/newsletter".into(), method: "POST".into(), has_csrf: true },
            ],
            ..Default::default()
        };
        assert!(b.ranked()[0].label().contains("/real"));
    }

    #[test]
    fn the_star_reflects_the_saved_set_not_a_page_flag() {
        // A flag on the page goes stale when the same URL is reached twice by
        // different routes — a link, then the history — and the star then lies.
        let mut b = Browser::default();
        b.load("https://x.test/a");
        b.loaded("A", vec![]);
        assert!(!b.is_bookmarked());
        assert!(b.toggle_bookmark(100));
        assert!(b.is_bookmarked());
        // arrive again by another route
        b.load("https://x.test/a");
        assert!(b.is_bookmarked(), "still saved");
        assert!(!b.toggle_bookmark(200), "and unstars");
    }

    #[test]
    fn the_filter_reports_what_it_did() {
        // A filter that removes things silently is indistinguishable from a
        // broken page, and the first thing anyone does when a site looks wrong
        // is turn the filter off — so it has to be obvious it was on.
        let mut b = Browser { filter_mode: FilterMode::Standard, ..Default::default() };
        assert!(b.filter_summary().is_some());
        b.filter_mode = FilterMode::Off;
        assert_eq!(b.filter_summary(), None, "nothing to report when it is off");
    }

    #[test]
    fn filter_stats_are_per_page_not_cumulative() {
        // "412 trackers blocked" across a session tells you nothing about the
        // page in front of you.
        let mut b = Browser { filter_mode: FilterMode::Standard, ..Default::default() };
        b.filter_stats.filtered_elements = 17;
        b.load("https://elsewhere.test");
        assert_eq!(b.filter_stats.filtered_elements, 0, "reset with the page");
    }

    #[test]
    fn a_tracking_wrapper_is_unwrapped_rather_than_hidden() {
        // The distinction the mounted filter draws, and the reason to mount it
        // rather than write a second one: a div with tracking attributes AND
        // children is a wrapper AROUND real content, so it is unwrapped —
        // children rendered directly. Hiding it would take the article with it.
        // Only a CHILDLESS one is a tracking pixel and gets dropped.
        let attrs = [("data-analytics-id", "x")];
        let (hide, unwrap) = ad_filter::classify_tracking_wrapper("div", &attrs, true);
        assert!(!hide, "the content inside must survive");
        assert!(unwrap, "but the wrapper does not");
        let (hide_pixel, _) = ad_filter::classify_tracking_wrapper("div", &attrs, false);
        assert!(hide_pixel, "a childless tracker is a pixel and goes");
    }

    #[test]
    fn semantic_tags_are_exempt_whatever_attributes_they_carry() {
        // article/section/header "carry meaning we shouldn't override" — a
        // second filter that did not know that would strip real content while
        // claiming to remove ads.
        let attrs = [("data-analytics-id", "x")];
        for tag in ["article", "section", "header", "main"] {
            let (hide, unwrap) = ad_filter::classify_tracking_wrapper(tag, &attrs, false);
            assert!(!hide && !unwrap, "{tag} was touched");
        }
    }

    #[test]
    fn an_empty_list_says_which_empty_it_is() {
        // Nothing found on a blank page is expected; nothing found on a login
        // page is a bug worth seeing.
        let mut b = Browser::default();
        assert_eq!(b.empty_reason(), Some("nothing loaded"));
        b.load("https://x.test");
        assert_eq!(b.empty_reason(), Some("loading…"));
        b.loaded("X", vec![]);
        assert_eq!(b.empty_reason(), Some("nothing to act on here"));
        b.loaded("X", vec![login(true)]);
        assert_eq!(b.empty_reason(), None);
    }

    #[test]
    fn loading_a_new_page_clears_the_old_actions_immediately() {
        // Leaving the previous page's login form visible while a new page loads
        // invites acting on the wrong site — for a login form that is a
        // credential sent to a stranger.
        let mut b = Browser::default();
        b.loaded("Bank", vec![login(true)]);
        assert_eq!(b.actions.len(), 1);
        b.load("https://elsewhere.test");
        assert!(b.actions.is_empty(), "cleared on load, not on arrival");
        assert!(b.title.is_empty());
    }

    #[test]
    fn a_non_get_api_call_asks_before_firing() {
        // A voice command should not silently POST.
        let post = Action::Api { url: "/v1/delete".into(), method: "POST".into(),
                                 auth: "none".into(), description: "".into() };
        assert!(post.needs_confirmation());
        let get = Action::Api { url: "/v1/list".into(), method: "GET".into(),
                                auth: "none".into(), description: "".into() };
        assert!(!get.needs_confirmation());
        assert!(!login(true).needs_confirmation(), "a login is not a silent mutation");
    }

    #[test]
    fn an_endpoint_needing_a_bearer_token_is_listed_but_not_offered() {
        // Listing it tells the user the capability exists; offering it would
        // fail with an opaque 401.
        let a = Action::Api { url: "/v1/me".into(), method: "GET".into(),
                              auth: "bearer".into(), description: "".into() };
        assert!(!a.actionable());
        assert!(a.blocked_reason().unwrap().contains("bearer"));
        let b = Browser { actions: vec![a], scanned: true, ..Default::default() };
        assert_eq!(b.ranked().len(), 1, "still shown");
        assert!(b.actionable().is_empty(), "but not offered");
    }
}
