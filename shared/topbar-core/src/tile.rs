//! tile — the bar's "unified tile" idea, carried over from hatch: a pinned
//! launcher icon and a running window's taskbar entry are the same *kind*
//! of thing on this bar (an id, a label, an icon, a running/not state), fed
//! into the one [`hatch_core::anim::Board`] rather than two separate grids
//! with two separate animation systems.

use hatch_core::AppEntry;
use systemcompkit::ToplevelInfo;

use crate::category::TileCategory;

#[derive(Debug, Clone, PartialEq)]
pub struct TileItem {
    /// The key `Board::retarget` tracks identity by. Stable across ticks
    /// for the same app/window so restack (not enter+exit) is what plays
    /// when only order changes.
    pub key: String,
    pub label: String,
    pub icon: String,
    pub running: bool,
    pub source: TileSource,
    /// What kind of tile this is, for `Placement` to decide whether a
    /// given form factor shows it at all — see `category`'s own doc.
    pub category: TileCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileSource {
    /// A favorited launcher entry. Keyed by the desktop-file id
    /// (`AppEntry::id`), which is also what `ToplevelInfo::app_id` should
    /// match if the compositor reports it consistently with the .desktop
    /// file — the thing `unify` checks to fold a running pinned app into
    /// one tile instead of two.
    Pinned,
    /// A window with no pinned entry behind it. Keyed by
    /// `format!("toplevel:{id}")` — the compositor's `u64` toplevel id has
    /// no relation to a desktop-file id, so it gets its own namespace
    /// rather than risking a collision.
    Running(u64),
}

/// Merge favorites with running windows into one ordered list: pinned apps
/// first, in the launcher's own order, then any running window whose
/// `app_id` doesn't match a pinned entry. A pinned app that's currently
/// running gets exactly one tile (`running: true`), not a launcher tile
/// plus a separate taskbar tile for the same app.
///
/// `pinned` should already be filtered to favorites by the caller (e.g.
/// `launcher.results().filter(|e| launcher.is_favorite(&e.id))`) — this
/// function doesn't know about ranking or favorites, only merging.
pub fn unify<'a>(pinned: impl Iterator<Item = &'a AppEntry>, running: &[ToplevelInfo]) -> Vec<TileItem> {
    let mut items = Vec::new();
    let mut matched_app_ids: Vec<&str> = Vec::new();

    for entry in pinned {
        let is_running = running.iter().any(|t| t.app_id == entry.id);
        if is_running {
            matched_app_ids.push(entry.id.as_str());
        }
        items.push(TileItem {
            key: entry.id.clone(),
            label: entry.name.clone(),
            icon: entry.icon.clone(),
            running: is_running,
            source: TileSource::Pinned,
            category: TileCategory::Launcher,
        });
    }

    for t in running {
        if matched_app_ids.contains(&t.app_id.as_str()) {
            continue; // already represented by its pinned tile above
        }
        if t.title.is_empty() && t.app_id.is_empty() {
            // Genuinely nothing to paint yet — see ToplevelInfo::title's own
            // doc on why this is never defaulted to a placeholder string.
            continue;
        }
        items.push(TileItem {
            key: format!("toplevel:{}", t.id),
            label: if t.title.is_empty() { t.app_id.clone() } else { t.title.clone() },
            icon: t.app_id.clone(),
            running: true,
            source: TileSource::Running(t.id),
            category: TileCategory::Taskbar,
        });
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use systemcompkit::ToplevelState;

    fn entry(id: &str) -> AppEntry {
        AppEntry { id: id.into(), name: id.into(), icon: format!("{id}-icon"), ..Default::default() }
    }

    fn toplevel(id: u64, app_id: &str, title: &str) -> ToplevelInfo {
        ToplevelInfo { id, title: title.into(), app_id: app_id.into(), state: ToplevelState::default() }
    }

    #[test]
    fn a_pinned_app_thats_running_is_one_tile_not_two() {
        let pinned = vec![entry("firefox")];
        let running = vec![toplevel(1, "firefox", "Mozilla Firefox")];
        let items = unify(pinned.iter(), &running);
        assert_eq!(items.len(), 1, "no duplicate tile for the same app");
        assert!(items[0].running);
        assert_eq!(items[0].source, TileSource::Pinned);
    }

    #[test]
    fn a_running_app_with_no_pinned_entry_gets_its_own_tile() {
        let pinned = vec![entry("firefox")];
        let running = vec![toplevel(1, "firefox", "Firefox"), toplevel(2, "org.kde.okular", "some.pdf")];
        let items = unify(pinned.iter(), &running);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].key, "toplevel:2");
        assert_eq!(items[1].label, "some.pdf");
    }

    #[test]
    fn a_pinned_app_thats_not_running_still_shows_unrun() {
        let pinned = vec![entry("files")];
        let items = unify(pinned.iter(), &[]);
        assert_eq!(items.len(), 1);
        assert!(!items[0].running);
    }

    #[test]
    fn a_toplevel_with_no_title_or_app_id_yet_is_skipped() {
        let running = vec![toplevel(9, "", "")];
        let items = unify(std::iter::empty(), &running);
        assert!(items.is_empty(), "nothing presentable yet — see the New/Title/AppId event ordering");
    }
}
