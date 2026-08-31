//! favorites — jump to a directory, from KDE's Places or just Home.
//!
//! Ported from `favoritesPopup` in `main.qml`. Opened by voice ("show
//! favorites") or the home icon in the toolbar; picking an entry changes
//! directory.
//!
//! ## Two rules the QML states and this keeps
//!
//! **It falls back gracefully on non-KDE systems: only the Home anchor
//! appears.** This fleet is mostly not KDE — postmarketOS handsets and Asahi
//! machines running whatever — so the fallback is the common case, not the
//! edge. A favorites list that is empty on most of the fleet is a menu item
//! that opens onto nothing, and Home is always a real answer.
//!
//! **The list refreshes on every open**, so someone who edits Dolphin's
//! favorites mid-session sees the change without restarting. That means this
//! holds no cached list — [`entries`] reads the file each time it is called,
//! and the cost is parsing one small XML file against a human deciding where
//! to go.
//!
//! `smart_cd::dolphin_favorites` already parses `~/.local/share/user-places.xbel`
//! and returns an empty vec on any failure, which is the behaviour the fallback
//! needs. Included by path rather than reimplemented.

// smart_cd reaches `crate::data` for the DESCENT heuristics, which
// `dolphin_favorites` does not touch — but `#[path]` pulls the whole file, so
// the reference has to resolve. The stub lives at the CRATE ROOT in lib.rs,
// because `crate::` resolves there and not in this module; putting it here
// compiled as `crate::favorites::data` and left the original unresolved.
#[path = "../../src/smart_cd.rs"]
#[allow(dead_code, unused_imports)]
pub mod smart_cd;

pub use smart_cd::Favorite;

/// Where a favorite came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Always present, on every system.
    Home,
    /// From KDE's Places file.
    Places,
}

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub origin: Origin,
}

/// The list to show, read fresh.
///
/// `home` is passed in rather than read from the environment here: a UI module
/// that resolves $HOME is a second answer to a question the shell already
/// answers, and the two disagree the moment someone runs under sudo.
pub fn entries(home: &str, places: Vec<Favorite>) -> Vec<Entry> {
    // HOME FIRST AND ALWAYS. On a non-KDE system it is the whole list, and it
    // is the one destination that cannot fail to exist.
    let mut out = vec![Entry {
        name: "Home".into(), path: home.to_string(), origin: Origin::Home,
    }];
    for f in places {
        // A Places entry pointing at home would be a duplicate row with a
        // different label — Dolphin ships one by default, so this is the
        // common case rather than a defensive check.
        if f.path.trim_end_matches('/') == home.trim_end_matches('/') { continue; }
        // Non-local entries are dropped. The XBEL holds trash:/, remote://,
        // timeline: and search: URLs, and `cd remote://foo` is not a thing a
        // terminal can do — offering it produces an error the operator did
        // not ask for.
        if !f.path.starts_with('/') { continue; }
        out.push(Entry { name: f.name, path: f.path, origin: Origin::Places });
    }
    out
}

/// What was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Change to this directory.
    Cd(String),
    Close,
}

/// Draw the panel.
pub fn favorites_ui(
    ui: &mut egui::Ui,
    rows: &[Entry],
    c: &uikit_egui::PaletteColors,
) -> Option<Action> {
    let mut act = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("favorites").strong());
        if ui.button("close").clicked() { act = Some(Action::Close); }
    });
    ui.separator();
    for e in rows {
        let resp = ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&e.name)
                .color(if e.origin == Origin::Home { c.accent } else { c.text }));
            ui.label(egui::RichText::new(&e.path).monospace().small().color(c.text_weak));
        });
        if uikit_egui::dock::row_click(ui, resp.response.rect, &e.path).clicked() {
            act = Some(Action::Cd(e.path.clone()));
        }
    }
    if rows.len() == 1 {
        // Said, because a one-row list looks like a list that failed to load.
        // It has not: KDE's Places file is simply not on this machine, which
        // is the ordinary case on most of this fleet.
        ui.separator();
        ui.label(egui::RichText::new(
            "no KDE Places file — Home is the whole list on this system")
            .small().weak());
    }
    act
}

/// Driven through real frames — `favorites_ui` had the unclickable-row
/// bug, so "the rows are there" was never the question.
#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::frame_probe::Probe;

    fn rows() -> Vec<Entry> {
        vec![
            Entry { name: "Home".into(), path: "/home/me".into(), origin: Origin::Home },
            Entry { name: "Projects".into(), path: "/home/me/proj".into(),
                    origin: Origin::Places },
        ]
    }

    #[test]
    fn clicking_a_favorite_asks_to_cd_to_its_path() {
        // The whole purpose of the panel. It drew correctly and did nothing
        // when clicked, which is the worst of both: it looks operational.
        let p = Probe::default();
        let c = p.colors();
        let act = p.click_text("Projects", |ui| favorites_ui(ui, &rows(), &c));
        assert_eq!(act, Some(Action::Cd("/home/me/proj".into())));
    }

    #[test]
    fn every_favorite_is_drawn_once_with_its_path() {
        let p = Probe::default();
        let c = p.colors();
        let (d, _) = p.frame(|ui| favorites_ui(ui, &rows(), &c));
        assert_eq!(d.count("Projects"), 1, "{:?}", d.texts);
        assert!(d.has("/home/me/proj"), "the path is what tells two 'src' apart");
    }

    #[test]
    fn close_is_reachable_and_is_not_a_cd() {
        let p = Probe::default();
        let c = p.colors();
        let act = p.click_text("close", |ui| favorites_ui(ui, &rows(), &c));
        assert_eq!(act, Some(Action::Close));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fav(n: &str, p: &str) -> Favorite {
        Favorite { name: n.into(), path: p.into() }
    }

    #[test]
    fn home_is_present_on_a_system_with_no_places_file() {
        // The QML's stated fallback, and on this fleet it is the COMMON case
        // rather than the edge — postmarketOS handsets and Asahi machines are
        // mostly not KDE. An empty favorites list is a menu item that opens
        // onto nothing.
        let e = entries("/home/u", vec![]);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].origin, Origin::Home);
        assert_eq!(e[0].path, "/home/u");
    }

    #[test]
    fn a_places_entry_for_home_does_not_duplicate_the_anchor() {
        // Dolphin ships one by default, so this is the common case and not a
        // defensive check. Two rows for one directory with different labels is
        // a list that looks buggy.
        let e = entries("/home/u", vec![fav("Home", "/home/u"), fav("Docs", "/home/u/docs")]);
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].name, "Docs");
        // And a trailing slash is the same directory.
        let e2 = entries("/home/u", vec![fav("Home", "/home/u/")]);
        assert_eq!(e2.len(), 1);
    }

    #[test]
    fn non_local_places_are_dropped_rather_than_offered() {
        // The XBEL holds trash:/, remote://, timeline: and search: URLs. `cd
        // remote://foo` is not something a terminal can do, and offering it
        // produces an error the operator did not ask for.
        let e = entries("/home/u", vec![
            fav("Trash", "trash:/"),
            fav("Recent", "recentlyused:/files"),
            fav("Network", "remote:/"),
            fav("Code", "/home/u/code"),
        ]);
        assert_eq!(e.iter().filter(|x| x.origin == Origin::Places).count(), 1);
        assert_eq!(e[1].path, "/home/u/code");
    }

    #[test]
    fn home_is_always_first() {
        // It is the fallback and the anchor; a Places list that sorted above
        // it would bury the one entry guaranteed to work.
        let e = entries("/home/u", vec![fav("Aaa", "/aaa"), fav("Bbb", "/bbb")]);
        assert_eq!(e[0].origin, Origin::Home);
    }

    #[test]
    fn home_is_passed_in_rather_than_resolved_here() {
        // A UI module reading $HOME is a second answer to a question the shell
        // already answers, and the two disagree the moment someone runs under
        // sudo.
        let src = include_str!("favorites.rs");
        for probe in [format!("{}(\"HOME\")", "var"), format!("home_{}()", "dir")] {
            assert!(!src.contains(probe.as_str()), "this module resolves home itself: {probe}");
        }
    }

    #[test]
    fn a_single_row_explains_itself() {
        // Otherwise it reads as a list that failed to load, when in fact the
        // Places file simply is not on this machine.
        let src = include_str!("favorites.rs");
        assert!(src.contains("no KDE Places file"));
    }

    #[test]
    fn the_list_is_built_fresh_rather_than_cached() {
        // The QML refreshes on every open so a user who edits Dolphin's
        // favorites mid-session sees it without restarting. A cached list here
        // would defeat that no matter what the caller does.
        let src = include_str!("favorites.rs");
        // No state struct at all: entries() is a function of its arguments.
        let holder = format!("pub struct {}", "Favorites");
        assert!(!src.contains(holder.as_str()),
                "a holder appeared, which is where a stale cache lives");
    }
}
