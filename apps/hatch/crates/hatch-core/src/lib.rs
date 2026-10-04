//! hatch-core — the launcher as a headless model.
//!
//! [`Launcher`] owns the scanned entries, the query, favorites and recents,
//! and answers one question: *given this query, which entries, in which
//! order?* Everything a shell does — QML plasmoid or egui window — is paint
//! and input over this. That split is the point: grid feel gets iterated in
//! the shell, ranking/launching gets iterated (and tested) here, and a bug
//! report can be pinned to one side in a sentence.

pub mod anim;
pub mod grid;
pub mod entry;
pub mod exec;
pub mod icons;
pub mod icontheme;
pub mod plasma;
pub mod rank;
pub mod store;
pub mod watch;
pub mod notices;
pub mod status;   // the line from the boxes at home, off the DHT (feature status-dht)

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

/// How far a virtual entry's score is pushed down, so an app's own NAME hit (10 000 + prefix) always outranks its actions.
const VIRTUAL_PENALTY: i64 = 3_000;

/// The built-in system actions. `loginctl`/`systemctl` because they exist on
/// every systemd target we ship to; pmOS-without-systemd would override via
/// config in a later pass — the ids are the stable contract.
pub const SYSTEM_ACTIONS: &[SystemAction] = &[
    SystemAction { id: "lock", label: "Lock", icon: "system-lock-screen", argv: &["loginctl", "lock-session"] },
    SystemAction { id: "logout", label: "Log out", icon: "system-log-out", argv: &["loginctl", "terminate-user", ""] },
    SystemAction { id: "reboot", label: "Restart", icon: "system-reboot", argv: &["systemctl", "reboot"] },
    SystemAction { id: "poweroff", label: "Shut down", icon: "system-shutdown", argv: &["systemctl", "poweroff"] },
];

/// Test support for anything that reads or writes the user's config.
///
/// `XDG_CONFIG_HOME` is process-global and cargo runs tests in parallel, so
/// three modules each pointing it at their own temp dir is a race that
/// happens to pass — until it doesn't, at 2am, on one machine. Every test
/// that touches config takes this guard: it serialises them, redirects config
/// at a private directory, and cleans up on drop even if the test panics.
#[cfg(test)]
pub(crate) mod testenv {
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    pub struct ConfigHome {
        pub path: PathBuf,
        // Held for the lifetime of the guard; poisoning is irrelevant here
        // because the data protected is the environment, not a value.
        _lock: MutexGuard<'static, ()>,
    }

    pub fn scoped_config_home(tag: &str) -> ConfigHome {
        let lock = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = std::env::temp_dir().join(format!("hatch-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp config home");
        std::env::set_var("XDG_CONFIG_HOME", &path);
        ConfigHome { path, _lock: lock }
    }

    impl Drop for ConfigHome {
        fn drop(&mut self) {
            std::env::remove_var("XDG_CONFIG_HOME");
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

pub struct Launcher {
    pub config: Config,
    entries: Vec<AppEntry>,
    /// Snapshot of the application directories as of the last scan, so a
    /// re-summoned launcher can notice an app installed while it was hidden.
    watch: watch::DirWatch,
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
        let entries = Launcher::with_virtual(entries);
        let mut l = Launcher {
            config,
            entries,
            watch: watch::DirWatch::new(&entry::application_dirs()),
            favorites: store::load_favorites(),
            recents: store::load_recents(),
            query: String::new(),
            results: Vec::new(),
        };
        l.rerank();
        l
    }

    /// The apps plus the searchable non-app entries made from them (see `entry::virtual_entries`).
    fn with_virtual(mut apps: Vec<AppEntry>) -> Vec<AppEntry> {
        let v = entry::virtual_entries(&apps);
        apps.extend(v);
        apps
    }

    /// Re-scan installed applications unconditionally.
    pub fn refresh(&mut self) {
        let dirs = entry::application_dirs();
        self.entries = Launcher::with_virtual(entry::scan(&dirs));
        let _ = self.watch.changed(&dirs); // resynchronise, don't re-report
        self.rerank();
    }

    /// Re-scan only if the application directories changed since the last
    /// scan. Returns true if a scan happened — the caller's icon cache is
    /// then stale and should be dropped.
    ///
    /// Called when the window is summoned: it is the one moment the user is
    /// already waiting, and the one moment a stale grid is visible.
    pub fn refresh_if_changed(&mut self) -> bool {
        let dirs = entry::application_dirs();
        if !self.watch.changed(&dirs) {
            return false;
        }
        self.entries = entry::scan(&dirs);
        self.rerank();
        true
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
            // virtual entries (an app's actions, Lock) only show for a search, and rank below an app's own name hit
            .filter(|(_, e)| e.kind == entry::EntryKind::App || !self.query.trim().is_empty())
            .filter_map(|(i, e)| {
                let penalty = if e.kind == entry::EntryKind::App { 0 } else { VIRTUAL_PENALTY };
                rank::score(&self.query, e, self.standing(&e.id)).map(|s| (s - penalty, i))
            })
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

    /// Every entry the launcher knows, apps AND the virtual ones (an app's actions, Lock), whatever the query. For a
    /// caller with its own search box (Tideshell's palette) that wants to match them itself.
    pub fn all_entries(&self) -> impl Iterator<Item = &AppEntry> {
        self.entries.iter()
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

    /// Any known entry by id, regardless of the current query filter.
    ///
    /// `results()`/`result_at()` only see what the current query matched —
    /// exactly wrong for a shell painting a tile mid-exit-fade after a query
    /// change filtered it OUT of the result set. The tile is still leaving
    /// the screen; the shell still needs its name and icon for those last
    /// frames, and `results()` has already forgotten it existed.
    pub fn entry(&self, id: &str) -> Option<&AppEntry> {
        self.entries.iter().find(|e| e.id == id)
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
        // only apps are favourites: a virtual entry's id would be saved and never resolved
        if !self.entries.iter().any(|e| e.id == id && e.kind == entry::EntryKind::App) { return; }
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
        // a search result that is not an app: run what it stands for, through the same paths as the menus
        match &e.kind {
            entry::EntryKind::Action { parent, action } => return self.activate_action(parent, action),
            entry::EntryKind::System(sys) => return self.run_system_action(sys),
            entry::EntryKind::App => {}
        }
        let argv = exec::split_exec(&e.exec);
        let pid = exec::spawn_in(&argv, e.terminal, &self.config.terminal_wrap, e.workdir.as_deref())?;
        let _ = store::record_recent(id);
        self.recents = store::load_recents();
        self.rerank();
        Ok(pid)
    }

    /// Launch one of an entry's `[Desktop Action …]` items ("New Window").
    /// Recorded as a recent like a normal launch: from the user's side they
    /// opened the app, and it should float accordingly.
    pub fn activate_action(&mut self, id: &str, action_id: &str) -> std::io::Result<u32> {
        let e = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unknown entry"))?
            .clone();
        let a = e
            .actions
            .iter()
            .find(|a| a.id == action_id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unknown action"))?;
        let argv = exec::split_exec(&a.exec);
        let pid = exec::spawn_in(&argv, e.terminal, &self.config.terminal_wrap, e.workdir.as_deref())?;
        let _ = store::record_recent(id);
        self.recents = store::load_recents();
        Ok(pid)
    }

    /// The actions an entry declares, for a context menu.
    pub fn actions_for(&self, id: &str) -> &[entry::DesktopAction] {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.actions.as_slice())
            .unwrap_or(&[])
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

    fn e(name: &str) -> AppEntry {
        AppEntry {
            id: name.to_lowercase(),
            name: name.into(),
            description: String::new(),
            icon: String::new(),
            exec: "true".into(),
            terminal: false,
            ..Default::default()
        }
    }

    /// A launcher over fixed entries with config redirected at a private
    /// directory — these tests must not read a dev machine's real favorites,
    /// which would change the ordering assertions. The returned guard must be
    /// held for the duration of the test.
    fn fresh() -> (Launcher, crate::testenv::ConfigHome) {
        let guard = crate::testenv::scoped_config_home("lib");
        let l = Launcher::with_entries(
            Config::default(),
            vec![e("Files"), e("Firefox"), e("Terminal"), e("Camera")],
        );
        (l, guard)
    }

    #[test]
    fn an_empty_query_lists_everything_alphabetically() {
        let (l, _guard) = fresh();
        let names: Vec<_> = l.results().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Camera", "Files", "Firefox", "Terminal"]);
    }

    #[test]
    fn typing_narrows_and_ranks() {
        let (mut l, _guard) = fresh();
        l.set_query("fi");
        let names: Vec<_> = l.results().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Files", "Firefox"], "prefix hits only, alpha tie-break");
        l.set_query("");
        assert_eq!(l.result_count(), 4, "clearing restores the full grid");
    }

    #[test]
    fn a_favorite_floats_to_the_front_but_only_once_the_grid_settles() {
        let (mut l, _guard) = fresh();
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
        let (mut l, _guard) = fresh();
        l.activate("camera").unwrap(); // Exec=true — spawns harmlessly
        // Camera was already first alphabetically; launch Firefox to see a move.
        l.activate("firefox").unwrap();
        assert_eq!(l.result_at(0).unwrap().name, "Firefox");
    }

    #[test]
    fn a_summon_picks_up_an_app_installed_while_the_launcher_was_hidden() {
        // The whole point of refresh_if_changed: hatch stays resident between
        // summons, so without this an app installed at 10:00 is missing from
        // the grid until the process restarts.
        let guard = crate::testenv::scoped_config_home("refresh");
        let apps = guard.path.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("early.desktop"),
            "[Desktop Entry]\nType=Application\nName=Early\nExec=true\n",
        )
        .unwrap();
        // application_dirs() reads XDG_DATA_HOME; point it at the same place.
        std::env::set_var("XDG_DATA_HOME", &guard.path);
        std::env::set_var("XDG_DATA_DIRS", guard.path.join("empty"));

        let mut l = Launcher::new(Config::default());
        assert_eq!(l.result_count(), 1);
        assert!(!l.refresh_if_changed(), "nothing installed yet");

        std::fs::write(
            apps.join("late.desktop"),
            "[Desktop Entry]\nType=Application\nName=Late\nExec=true\n",
        )
        .unwrap();
        assert!(l.refresh_if_changed(), "the install is noticed");
        assert_eq!(l.result_count(), 2);
        assert!(!l.refresh_if_changed(), "and only once");

        std::env::remove_var("XDG_DATA_HOME");
        std::env::remove_var("XDG_DATA_DIRS");
    }

    #[test]
    fn entry_finds_by_id_even_when_the_query_has_filtered_it_out() {
        let (mut l, _guard) = fresh();
        l.set_query("fi"); // narrows to Files, Firefox — Camera and Terminal drop out
        assert_eq!(l.result_count(), 2, "sanity: the filter actually narrowed");
        assert!(l.entry("camera").is_some(), "still findable — a shell mid-exit-fade needs this");
        assert_eq!(l.entry("camera").unwrap().name, "Camera");
        assert!(l.entry("no-such-id").is_none());
    }

    #[test]
    fn system_actions_resolve_in_configured_order() {
        // Guarded even though this test ignores favorites: `with_entries`
        // loads them, and an unguarded read races the guarded writers.
        let _guard = crate::testenv::scoped_config_home("sysactions");
        let mut cfg = Config::default();
        cfg.system_actions = vec!["poweroff".into(), "lock".into(), "bogus".into()];
        let l = Launcher::with_entries(cfg, vec![]);
        let ids: Vec<_> = l.system_actions().iter().map(|a| a.id).collect();
        assert_eq!(ids, vec!["poweroff", "lock"], "unknown ids are dropped, order kept");
    }

    // ---- searching actions and functions, not only app names ----

    fn firefox() -> AppEntry {
        let mut f = e("Firefox");
        f.description = "Web Browser".into();
        f.exec = "true".into();
        f.actions = vec![
            entry::DesktopAction { id: "new-window".into(), name: "New Window".into(), exec: "true".into() },
            entry::DesktopAction { id: "new-private-window".into(), name: "New Private Window".into(), exec: "true".into() },
        ];
        f
    }

    fn names(l: &Launcher) -> Vec<String> { l.results().map(|r| r.name.clone()).collect() }

    #[test]
    fn a_search_finds_an_apps_actions_and_lock_but_browsing_still_shows_only_apps() {
        let _g = crate::testenv::scoped_config_home("virtual-search");
        let mut l = Launcher::with_entries(Config::default(), vec![firefox(), e("Files")]);
        assert_eq!(names(&l), vec!["Files", "Firefox"], "no query: only the apps, no actions, no Lock");
        l.set_query("private");
        assert_eq!(names(&l).first().map(String::as_str), Some("New Private Window \u{b7} Firefox"), "{:?}", names(&l));
        l.set_query("lock");
        assert!(names(&l).iter().any(|n| n == "Lock screen"), "{:?}", names(&l));
        l.set_query("");
        assert_eq!(names(&l).len(), 2, "back to apps only once the query is cleared");
    }

    #[test]
    fn an_apps_own_name_outranks_its_actions() {
        let _g = crate::testenv::scoped_config_home("virtual-rank");
        let mut l = Launcher::with_entries(Config::default(), vec![firefox()]);
        l.set_query("fire");
        let n = names(&l);
        assert_eq!(n[0], "Firefox", "{n:?}");
        assert!(n.len() > 1, "its actions are still findable: {n:?}");
    }

    #[test]
    fn power_functions_are_not_searchable_so_a_partial_query_and_enter_cannot_reboot() {
        let _g = crate::testenv::scoped_config_home("virtual-power");
        let mut l = Launcher::with_entries(Config::default(), vec![firefox()]);
        for q in ["restart", "reboot", "shut", "shutdown", "power off", "log out", "logout", "poweroff"] {
            l.set_query(q);
            assert!(l.results().all(|r| !matches!(r.kind, entry::EntryKind::System(_))), "{q:?} must not surface a system function: {:?}", names(&l));
            assert!(l.results().all(|r| !r.exec.contains("systemctl") && !r.exec.contains("terminate-user")), "{q:?}");
        }
    }

    #[test]
    fn activating_a_virtual_entry_runs_the_apps_action_and_it_is_never_a_favourite() {
        let _g = crate::testenv::scoped_config_home("virtual-activate");
        let mut l = Launcher::with_entries(Config::default(), vec![firefox()]);
        l.set_query("private");
        let id = l.result_at(0).unwrap().id.clone();
        assert_eq!(id, "firefox#new-private-window");
        assert!(l.activate(&id).is_ok(), "ran the action through activate_action");
        l.toggle_favorite(&id);
        assert!(!l.is_favorite(&id), "a virtual id would be saved and never resolved");
        l.toggle_favorite("firefox");
        assert!(l.is_favorite("firefox"), "an app still can be");
        assert!(l.activate("nope#nothing").is_err());
    }

    #[test]
    fn all_entries_includes_the_virtual_ones_that_browsing_hides() {
        let _g = crate::testenv::scoped_config_home("all-entries");
        let l = Launcher::with_entries(Config::default(), vec![firefox()]);
        assert_eq!(l.results().count(), 1, "browsing: the app only");
        let kinds: Vec<_> = l.all_entries().map(|e| e.kind.clone()).collect();
        assert!(kinds.iter().any(|k| matches!(k, entry::EntryKind::Action { .. })), "its actions are there: {kinds:?}");
        assert!(kinds.iter().any(|k| matches!(k, entry::EntryKind::System(_))), "and Lock");
        assert_eq!(kinds.iter().filter(|k| **k == entry::EntryKind::App).count(), 1);
    }
}
