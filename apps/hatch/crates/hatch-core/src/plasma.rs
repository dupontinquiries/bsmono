//! plasma — reading what the plasmoid already knows.
//!
//! The QML dash stores favorites in kicker's model, which lives in
//! `plasma-org.kde.plasma.desktop-appletsrc`; hatch-egui stores them in
//! `~/.config/hatch/favorites`. Two launchers with one name and two opinions
//! about your favorites is not a design, so this is the bridge — deliberately
//! ONE DIRECTION, Plasma → hatch, and deliberately a seed rather than a sync.
//!
//! Why not two-way: plasmashell holds that file open and rewrites it wholesale
//! on its own schedule, so an outside writer loses races it can't see. Reading
//! is safe, writing is not. Seeding on first run gets the user's curation
//! across; after that hatch owns its own file and the two can diverge, which
//! is at least honest and visible in plain text.
//!
//! Kicker favorite ids look like `applications:org.kde.dolphin.desktop`, or
//! `preferred://browser`, or a `file://` url for a document. Only the first
//! kind maps onto an app entry; the rest are dropped rather than guessed at.

use std::path::{Path, PathBuf};

/// Where Plasma keeps applet configuration.
pub fn appletsrc_path() -> PathBuf {
    config_kit::config_home().join("plasma-org.kde.plasma.desktop-appletsrc")
}

/// Turn a kicker favorite id into a hatch entry id, or `None` if it isn't an
/// application favorite.
pub fn favorite_to_entry_id(fav: &str) -> Option<String> {
    let rest = fav.strip_prefix("applications:").unwrap_or(fav);
    if rest.contains("://") || rest.starts_with('/') {
        return None; // preferred://, file://, ktp:// and friends
    }
    let stem = rest.strip_suffix(".desktop")?;
    if stem.is_empty() {
        return None;
    }
    // Kicker writes vendor-prefixed files with the same dash flattening the
    // scanner uses, so ids line up without further translation.
    Some(stem.to_string())
}

/// Parse favorites out of an appletsrc's text. Every `favorites=` key in the
/// file is taken: the group path identifies a containment and applet by
/// number, which tells us nothing durable, and any applet storing a
/// `favorites` list is a launcher of some kind whose picks are worth having.
/// Order is preserved and duplicates collapse to the first occurrence.
pub fn parse_favorites(text: &str) -> Vec<String> {
    let ini = config_kit::Ini::parse(text);
    let mut out: Vec<String> = Vec::new();
    for keys in ini.sections.values() {
        let Some(list) = keys.get("favorites") else { continue };
        for fav in list.split(',') {
            if let Some(id) = favorite_to_entry_id(fav.trim()) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    }
    out
}

/// Read and parse the user's appletsrc. `None` when there isn't one (no
/// Plasma on this machine, which is the normal case on a desktop dev box).
pub fn read_favorites(path: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(parse_favorites(&text))
}

/// Seed hatch's favorites from Plasma if — and only if — hatch has none of
/// its own yet. Returns the ids adopted. Called on startup: a first run on a
/// phone that already has a curated dash should show that curation, and a
/// second run must never resurrect a favorite the user removed in hatch.
pub fn seed_favorites_if_empty() -> Vec<String> {
    if !crate::store::load_favorites().is_empty() {
        return Vec::new();
    }
    let Some(favs) = read_favorites(&appletsrc_path()) else { return Vec::new() };
    if favs.is_empty() {
        return Vec::new();
    }
    let _ = crate::store::save_favorites(&favs);
    favs
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPLETSRC: &str = "\
[Containments][2]
plugin=org.kde.plasma.folder

[Containments][2][Applets][5]
plugin=org.kde.plasma.kickerdash

[Containments][2][Applets][5][Configuration][General]
favorites=applications:org.kde.dolphin.desktop,applications:firefox.desktop,preferred://browser
favoriteSystemActions=lock-screen,shutdown

[Containments][2][Applets][9][Configuration][General]
favorites=applications:firefox.desktop,applications:org.kde.konsole.desktop
";

    #[test]
    fn application_favorites_map_onto_entry_ids() {
        assert_eq!(
            favorite_to_entry_id("applications:org.kde.dolphin.desktop").as_deref(),
            Some("org.kde.dolphin")
        );
        assert_eq!(favorite_to_entry_id("firefox.desktop").as_deref(), Some("firefox"));
    }

    #[test]
    fn non_application_favorites_are_dropped_not_guessed() {
        assert_eq!(favorite_to_entry_id("preferred://browser"), None);
        assert_eq!(favorite_to_entry_id("file:///home/me/notes.txt"), None);
        assert_eq!(favorite_to_entry_id("applications:"), None);
        assert_eq!(favorite_to_entry_id("something-without-suffix"), None);
    }

    #[test]
    fn every_launcher_applet_contributes_in_order_without_duplicates() {
        let favs = parse_favorites(APPLETSRC);
        assert_eq!(favs, vec!["org.kde.dolphin", "firefox", "org.kde.konsole"]);
    }

    #[test]
    fn a_file_without_favorites_yields_nothing_rather_than_failing() {
        assert!(parse_favorites("[Containments][2]\nplugin=org.kde.plasma.folder\n").is_empty());
        assert_eq!(read_favorites(Path::new("/nonexistent/appletsrc")), None);
    }

    #[test]
    fn seeding_only_happens_when_hatch_has_no_favorites_of_its_own() {
        let guard = crate::testenv::scoped_config_home("seed");
        std::fs::write(guard.path.join("plasma-org.kde.plasma.desktop-appletsrc"), APPLETSRC)
            .unwrap();

        let adopted = seed_favorites_if_empty();
        assert_eq!(adopted, vec!["org.kde.dolphin", "firefox", "org.kde.konsole"]);
        assert_eq!(crate::store::load_favorites().len(), 3);

        // The user removes one in hatch; a later start must not bring it back.
        crate::store::save_favorites(&["firefox".to_string()]).unwrap();
        assert!(seed_favorites_if_empty().is_empty(), "seeding is once, not a sync");
        assert_eq!(crate::store::load_favorites(), vec!["firefox"]);
    }
}
