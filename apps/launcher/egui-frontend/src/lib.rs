//! launcher-egui — the tile launcher's egui-primary port.
//!
//! Layering (see UI_STRATEGY.md):
//!
//! * [`desktop`] — `.desktop` parse (pure) + scan (fs seam).
//! * [`grid`] — geometry and the slide-in patterns, pure numbers.
//! * [`anim`] — the tile [`anim::Board`]: pose state machine, explicit tick.
//! * [`Launcher`] (here) — query → fuzzy-ranked order → board retarget; the
//!   launch decision as data ([`LaunchSpec`]); the tap-vs-close rule
//!   ([`TapOutcome`]) that fixes the plasmoid's original touch bug in logic
//!   rather than in event-handler archaeology.
//! * [`shell`] — the egui painter over all of the above; the only module that
//!   touches egui types beyond colors.
//!
//! Nothing above [`shell`] imports egui's UI, so the whole behavior surface —
//! filtering, restack convergence, what a tap does — runs under `cargo test`
//! headless.

pub mod anim;
pub mod desktop;
pub mod grid;
pub mod shell;

use anim::{Board, BoardConfig};
use desktop::AppEntry;
use grid::GridSpec;
use uikit_egui::fuzzy;

/// What to execute, decided by the core, spawned by the shell.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchSpec {
    pub argv: Vec<String>,
    pub terminal: bool,
}

/// What a completed tap means. The rule that fixes "taps close the window":
/// only a *clean* tap (finger stayed within slop) on *empty background* closes;
/// a clean tap on a tile launches; anything that moved is a drag and does
/// neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapOutcome {
    Launch(usize),
    Close,
    Nothing,
}

/// Decide a tap. `moved` is the pointer's total travel; `slop` the touch
/// threshold; `hit` the tile index under BOTH press and release (None if they
/// disagree or neither is on a tile — a press that slides off a tile must not
/// launch it, and must not close the launcher either).
pub fn tap_outcome(moved: f32, slop: f32, hit: Option<usize>) -> TapOutcome {
    if moved > slop {
        return TapOutcome::Nothing;
    }
    match hit {
        Some(i) => TapOutcome::Launch(i),
        None => TapOutcome::Close,
    }
}

pub struct Launcher {
    pub apps: Vec<AppEntry>,
    pub query: String,
    /// Indices into `apps`, in display order for the current query.
    pub filtered: Vec<usize>,
    pub board: Board,
}

impl Launcher {
    pub fn new(apps: Vec<AppEntry>, config: BoardConfig) -> Launcher {
        let filtered = (0..apps.len()).collect();
        Launcher { apps, query: String::new(), filtered, board: Board::new(config) }
    }

    /// Load from the XDG application dirs.
    pub fn from_system(config: BoardConfig) -> Launcher {
        Launcher::new(desktop::scan(&desktop::application_dirs()), config)
    }

    /// Set the query, recompute the order, retarget the board. This is the
    /// whole search loop: everything the restack animation shows follows from
    /// the order this produces.
    pub fn set_query(&mut self, q: &str, spec: &GridSpec, now_ms: f64) {
        if q == self.query {
            return;
        }
        self.query = q.to_string();
        self.filtered = Self::rank(&self.apps, q);
        self.retarget(spec, now_ms);
    }

    /// Re-send the current order to the board (viewport size changed, or
    /// first layout).
    pub fn retarget(&mut self, spec: &GridSpec, now_ms: f64) {
        let order: Vec<&str> = self.filtered.iter().map(|&i| self.apps[i].id.as_str()).collect();
        self.board.retarget(&order, spec, now_ms);
    }

    /// Fuzzy rank: best score over each entry's haystacks; ties break by
    /// name. Empty query = everything, name order (the scan's resting order).
    fn rank(apps: &[AppEntry], q: &str) -> Vec<usize> {
        let q = q.trim().to_lowercase();
        if q.is_empty() {
            return (0..apps.len()).collect();
        }
        let mut scored: Vec<(i32, usize)> = apps
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                a.haystacks()
                    .filter_map(|h| fuzzy::score(&q, &h))
                    .max()
                    .map(|s| (s, i))
            })
            .collect();
        scored.sort_by(|(sa, ia), (sb, ib)| {
            sb.cmp(sa).then_with(|| apps[*ia].name.to_lowercase().cmp(&apps[*ib].name.to_lowercase()))
        });
        scored.into_iter().map(|(_, i)| i).collect()
    }

    /// The app at display position `pos` (index into `filtered`).
    pub fn app_at(&self, pos: usize) -> Option<&AppEntry> {
        self.filtered.get(pos).map(|&i| &self.apps[i])
    }

    /// What launching display position `pos` means. The shell spawns it (and
    /// wraps `terminal` entries in the user's terminal); the core only decides.
    pub fn launch_spec(&self, pos: usize) -> Option<LaunchSpec> {
        let app = self.app_at(pos)?;
        let argv = app.argv();
        if argv.is_empty() {
            return None;
        }
        Some(LaunchSpec { argv, terminal: app.terminal })
    }
}

/// The XDG app namespace this launcher's own prefs live under — deliberately
/// its own id, distinct from the reverse-DNS `APP_ID` used for the window/
/// `.desktop` matching below.
const CONFIG_APP: &str = "launcher-egui";
/// Reverse-DNS id for window/.desktop matching (see `uikit_egui::desktop`).
const APP_ID: &str = "org.c4.launcher-egui";

/// Board timings + pattern + palette, loaded from `config-kit`'s flat INI
/// under `[launcher]`. Any missing/unparsable key falls back to the default —
/// deleting the file (or a key) is always a safe reset.
fn board_config_from_prefs(ini: &config_kit::Ini) -> BoardConfig {
    let mut c = BoardConfig::default();
    if let Some(v) = ini.get("launcher", "slide_pattern").and_then(|s| s.parse::<usize>().ok()) {
        c.pattern = grid::SlidePattern::from_index(v);
    }
    macro_rules! ms {
        ($key:literal, $field:ident) => {
            if let Some(v) = ini.get("launcher", $key).and_then(|s| s.parse::<f32>().ok()) {
                c.$field = v;
            }
        };
    }
    ms!("enter_ms", enter_ms);
    ms!("stagger_ms", stagger_ms);
    ms!("restack_ms", restack_ms);
    ms!("restack_stagger_ms", restack_stagger_ms);
    ms!("exit_ms", exit_ms);
    c
}

/// The deployable build: fullscreen, undecorated, reads `[launcher]` prefs
/// (`palette`, `slide_pattern`, `*_ms`) from config-kit, scans installed apps,
/// runs until the launcher closes itself (tap-on-background, swipe-up fling,
/// Escape, or a successful launch).
#[cfg(feature = "app")]
pub fn run() -> eframe::Result<()> {
    let ini = config_kit::load(CONFIG_APP, "config.ini")
        .map(|s| config_kit::Ini::parse(&s))
        .unwrap_or_default();
    let palette_id = ini.get("launcher", "palette").unwrap_or("dark");
    let palette = uikit_egui::PaletteColors::from_palette(snapclone_theme::palette_by_id(palette_id));
    let board_config = board_config_from_prefs(&ini);

    let shell = shell::Shell::new(Launcher::from_system(board_config), palette);

    let viewport = uikit_egui::desktop::viewport(APP_ID, "Launcher", [1080.0, 2340.0])
        .with_fullscreen(true)
        .with_decorations(false);
    let opts = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Launcher", opts, Box::new(|_cc| Ok(Box::new(shell))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, exec: &str) -> AppEntry {
        AppEntry {
            id: id.into(),
            name: name.into(),
            generic: String::new(),
            exec: exec.into(),
            icon: String::new(),
            keywords: vec![],
            terminal: false,
        }
    }

    fn launcher() -> Launcher {
        Launcher::new(
            vec![
                app("org.mozilla.firefox", "Firefox", "firefox"),
                app("org.kde.dolphin", "Dolphin", "dolphin"),
                app("org.kde.konsole", "Konsole", "konsole"),
                app("files", "Files", "nautilus"),
            ],
            BoardConfig::default(),
        )
    }

    fn spec() -> GridSpec {
        GridSpec { cols: 4, rows_visible: 3, cell: 100.0 }
    }

    #[test]
    fn empty_query_shows_everything_in_given_order() {
        let l = launcher();
        assert_eq!(l.filtered, vec![0, 1, 2, 3]);
    }

    #[test]
    fn query_filters_and_ranks() {
        let mut l = launcher();
        l.set_query("fi", &spec(), 0.0);
        let names: Vec<_> = l.filtered.iter().map(|&i| l.apps[i].name.as_str()).collect();
        assert!(names.contains(&"Firefox"));
        assert!(names.contains(&"Files"));
        assert!(!names.contains(&"Dolphin"));
        // Both start with "fi": prefix matches, tie broken by name.
        assert_eq!(names[0], "Files");
    }

    #[test]
    fn query_drives_board_population() {
        let mut l = launcher();
        l.retarget(&spec(), 0.0);
        let mut t = 0.0;
        while l.board.tick(t) {
            t += 16.0;
        }
        assert_eq!(l.board.len(), 4);
        // "dolph" is a clean single-app query: it's a subsequence of
        // "org.kde.dolphin" (id) and "Dolphin" (name), and of nothing else's
        // id or name here. ("kon" would also do — but it turns out to be a
        // subsequence of "org.kde.dolphin" too: k(de)…o(lphin)…n, which is
        // correct fuzzy behavior, just not the single-survivor case this test
        // wants.)
        l.set_query("dolph", &spec(), t);
        // Leavers are still fading this instant…
        assert!(l.board.len() > 1);
        while l.board.tick(t) {
            t += 16.0;
        }
        // …and gone once settled.
        assert_eq!(l.board.len(), 1);
        assert_eq!(l.board.poses().next().unwrap().0, "org.kde.dolphin");
    }

    #[test]
    fn launch_spec_is_argv_not_a_string() {
        let l = launcher();
        assert_eq!(
            l.launch_spec(0),
            Some(LaunchSpec { argv: vec!["firefox".into()], terminal: false })
        );
        assert_eq!(l.launch_spec(99), None);
    }

    #[test]
    fn tap_rule_kills_the_accidental_close() {
        let slop = 12.0;
        // Clean tap on a tile: launch.
        assert_eq!(tap_outcome(3.0, slop, Some(5)), TapOutcome::Launch(5));
        // Clean tap on background: close.
        assert_eq!(tap_outcome(3.0, slop, None), TapOutcome::Close);
        // Finger slid — the plasmoid's old bug closed here. We do nothing.
        assert_eq!(tap_outcome(30.0, slop, None), TapOutcome::Nothing);
        assert_eq!(tap_outcome(30.0, slop, Some(5)), TapOutcome::Nothing);
    }
}
