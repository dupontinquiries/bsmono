//! hatch-core — the launcher as a headless model.
//!
//! [`Launcher`] owns the scanned entries, the query, favorites and recents,
//! and answers one question: *given this query, which entries, in which
//! order?* Everything a shell does — QML plasmoid or egui window — is paint
//! and input over this. That split is the point: grid feel gets iterated in
//! the shell, ranking/launching gets iterated (and tested) here, and a bug
//! report can be pinned to one side in a sentence.

pub mod entry;
pub mod exec;
pub mod icons;
pub mod rank;
pub mod store;

pub use entry::AppEntry;
pub use store::Config;

use rank::Standing;

/// A system-row action: what to show and what to run.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemAction {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub argv: &'static [&'static str],
}

/// The built-in system actions. `loginctl`/`systemctl` because they exist on
/// every systemd target we ship to; pmOS-without-systemd would override via
/// config in a later pass — the ids are the stable contract.
pub const SYSTEM_ACTIONS: &[SystemAction] = &[
    SystemAction { id: "lock", label: "Lock", icon: "system-lock-screen", argv: &["loginctl", "lock-session"] },
    SystemAction { id: "logout", label: "Log out", icon: "system-log-out", argv: &["loginctl", "terminate-user", ""] },
    SystemAction { id: "reboot", label: "Restart", icon: "system-reboot", argv: &["systemctl", "reboot"] },
    SystemAction { id: "poweroff", label: "Shut down", icon: "system-shutdown", argv: &["systemctl", "poweroff"] },
];

pub struct Launcher {
    pub config: Config,
    entries: Vec<AppEntry>,
    favorites: Vec<String>,
    recents: Vec<(String, u64)>,
    query: String,
    /// Indices into `entries`, in display order for the current query.
    results: Vec<usize>,
}

impl Launcher {
    /// Scan the real XDG dirs. For tests, use [`Launcher::with_entries`].
    pub fn new(config: Config) -> Launcher {
        let entries = entry::scan(&entry::application_dirs());
        Launcher::assemble(config, entries)
    }

    /// A launcher over a fixed entry set — the whole model is testable
    /// without touching the host's applications.
    pub fn with_entries(config: Config, entries: Vec<AppEntry>) -> Launcher {
        Launcher::assemble(config, entries)
    }

    fn assemble(config: Config, entries: Vec<AppEntry>) -> Launcher {
        let mut l = Launcher {
            config,
            entries,
            favorites: store::load_favorites(),
            recents: store::load_recents(),
            query: String::new(),
            results: Vec::new(),
        };
        l.rerank();
        l
    }

    /// Re-scan installed applications (e.g. after an install notification).
    pub fn refresh(&mut self) {
        self.entries = entry::scan(&entry::application_dirs());
        self.rerank();
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, q: &str) {
        if self.query != q {
            self.query = q.to_string();
            self.rerank();
        }
    }

    fn standing(&self, id: &str) -> Standing {
        Standing {
            favorite: self.favorites.iter().any(|f| f == id),
            recent_rank: self.recents.iter().position(|(r, _)| r == id),
        }
    }

    fn rerank(&mut self) {
        let mut scored: Vec<(i64, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| rank::score(&self.query, e, self.standing(&e.id)).map(|s| (s, i)))
            .collect();
        // Stable tie-break on name so equal scores don't shuffle between
        // keystrokes — a grid that reorders under a still finger reads as
        // broken even when the ordering is "correct".
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| self.entries[a.1].name.to_lowercase().cmp(&self.entries[b.1].name.to_lowercase()))
        });
        self.results = scored.into_iter().map(|(_, i)| i).collect();
    }

    /// Entries in display order for the current query.
    pub fn results(&self) -> impl Iterator<Item = &AppEntry> {
        self.results.iter().map(|&i| &self.entries[i])
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    pub fn result_at(&self, n: usize) -> Option<&AppEntry> {
        self.results.get(n).map(|&i| &self.entries[i])
    }

    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|f| f == id)
    }

    /// Add or remove a favorite. Deliberately does NOT reorder the current
    /// results: favoriting happens by long-pressing a tile, and resorting the
    /// grid under a finger that is still down moves the target mid-gesture —
    /// visually jarring, and it makes the release land on a different app.
    /// The new order takes effect at the next natural settling point (a query
    /// change, a re-summon, a refresh) via [`Launcher::resettle`].
    pub fn toggle_favorite(&mut self, id: &str) {
        if let Some(pos) = self.favorites.iter().position(|f| f == id) {
            self.favorites.remove(pos);
        } else {
            self.favorites.push(id.to_string());
        }
        let _ = store::save_favorites(&self.favorites);
    }

    /// Re-sort the results for the current query and standings. Call at a
    /// moment when the grid may move without surprising anyone.
    pub fn resettle(&mut self) {
        self.rerank();
    }

    /// Launch by id: spawn detached, record the recent, rerank.
    pub fn activate(&mut self, id: &str) -> std::io::Result<u32> {
        let e = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unknown entry"))?
            .clone();
        let argv = exec::split_exec(&e.exec);
        let pid = exec::spawn(&argv, e.terminal, &self.config.terminal_wrap)?;
        let _ = store::record_recent(id);
        self.recents = store::load_recents();
        self.rerank();
        Ok(pid)
    }

    /// The configured system actions, resolved against the built-in table.
    pub fn system_actions(&self) -> Vec<&'static SystemAction> {
        self.config
            .system_actions
            .iter()
            .filter_map(|id| SYSTEM_ACTIONS.iter().find(|a| a.id == id))
            .collect()
    }

    pub fn run_system_action(&self, id: &str) -> std::io::Result<u32> {
        let a = SYSTEM_ACTIONS
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unknown action"))?;
        let mut argv: Vec<String> = a.argv.iter().map(|s| s.to_string()).collect();
        // logout's empty arg is the current user.
        if a.id == "logout" {
            let user = std::env::var("USER").unwrap_or_default();
            if let Some(slot) = argv.iter_mut().find(|s| s.is_empty()) {
                *slot = user;
            }
        }
        exec::spawn(&argv, false, &[])
    }

    /// Resolve an entry's icon to a file, honouring the configured theme.
    pub fn icon_path(&self, e: &AppEntry) -> Option<std::path::PathBuf> {
        icons::resolve(&icons::icon_roots(), &self.config.icon_theme, &e.icon, self.config.icon_px)
    }

    pub fn icon_path_for_name(&self, name: &str) -> Option<std::path::PathBuf> {
        icons::resolve(&icons::icon_roots(), &self.config.icon_theme, name, self.config.icon_px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn e(name: &str) -> AppEntry {
        AppEntry {
            id: name.to_lowercase(),
            name: name.into(),
            description: String::new(),
            icon: String::new(),
            exec: "true".into(),
            terminal: false,
            path: PathBuf::new(),
        }
    }

    fn fresh() -> Launcher {
        // Isolate persistence: launcher tests must not read a dev machine's
        // real favorites (they would change ordering assertions).
        let tmp = std::env::temp_dir().join(format!("hatch-lib-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        Launcher::with_entries(
            Config::default(),
            vec![e("Files"), e("Firefox"), e("Terminal"), e("Camera")],
        )
    }

    #[test]
    fn an_empty_query_lists_everything_alphabetically() {
        let l = fresh();
        let names: Vec<_> = l.results().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Camera", "Files", "Firefox", "Terminal"]);
    }

    #[test]
    fn typing_narrows_and_ranks() {
        let mut l = fresh();
        l.set_query("fi");
        let names: Vec<_> = l.results().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Files", "Firefox"], "prefix hits only, alpha tie-break");
        l.set_query("");
        assert_eq!(l.result_count(), 4, "clearing restores the full grid");
    }

    #[test]
    fn a_favorite_floats_to_the_front_but_only_once_the_grid_settles() {
        let mut l = fresh();
        l.toggle_favorite("terminal");
        assert_eq!(
            l.result_at(0).unwrap().name,
            "Camera",
            "the grid must not move under the finger that just favorited"
        );
        assert!(l.is_favorite("terminal"), "though the star is on immediately");

        l.resettle();
        assert_eq!(l.result_at(0).unwrap().name, "Terminal");

        l.toggle_favorite("terminal");
        l.resettle();
        assert_eq!(l.result_at(0).unwrap().name, "Camera", "untoggling restores alpha order");
    }

    #[test]
    fn activating_records_a_recent_and_floats_it() {
        let mut l = fresh();
        l.activate("camera").unwrap(); // Exec=true — spawns harmlessly
        // Camera was already first alphabetically; launch Firefox to see a move.
        l.activate("firefox").unwrap();
        assert_eq!(l.result_at(0).unwrap().name, "Firefox");
    }

    #[test]
    fn system_actions_resolve_in_configured_order() {
        let mut cfg = Config::default();
        cfg.system_actions = vec!["poweroff".into(), "lock".into(), "bogus".into()];
        let l = Launcher::with_entries(cfg, vec![]);
        let ids: Vec<_> = l.system_actions().iter().map(|a| a.id).collect();
        assert_eq!(ids, vec!["poweroff", "lock"], "unknown ids are dropped, order kept");
    }
}
