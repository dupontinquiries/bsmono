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

    pub fn load(&mut self, url: &str) {
        self.url = url.to_string();
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
