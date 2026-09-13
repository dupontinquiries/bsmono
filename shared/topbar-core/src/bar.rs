//! bar — the top bar's own state: a collapsed strip vs. an expanded
//! launcher+taskbar sheet, and the unified [`hatch_core::anim::Board`]
//! driving both rows' motion. Pure: no egui, no wayland-client. A shell
//! ticks this every frame and paints `Bar::poses()`.

use hatch_core::anim::{Board, BoardConfig, TilePose};
use hatch_core::grid::GridSpec;

use crate::tile::TileItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarMode {
    /// The slim strip: status only, no tiles rendered.
    Collapsed,
    /// The pulled-down sheet: launcher + taskbar tiles visible.
    Expanded,
}

pub struct Bar {
    mode: BarMode,
    board: Board,
    items: Vec<TileItem>,
}

impl Bar {
    pub fn new(config: BoardConfig) -> Bar {
        Bar { mode: BarMode::Collapsed, board: Board::new(config), items: Vec::new() }
    }

    pub fn mode(&self) -> BarMode {
        self.mode
    }

    /// Collapse or expand. Deliberately doesn't touch `items`/`board` —
    /// collapsing keeps the tile set and its live poses intact so
    /// re-expanding doesn't replay every tile's enter animation.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.mode = if expanded { BarMode::Expanded } else { BarMode::Collapsed };
    }

    /// Feed the current unified tile set (see [`crate::tile::unify`]).
    /// Cheap to call every frame regardless of whether anything changed —
    /// `Board::retarget` only actually animates on a genuine key-order
    /// change, per its own doc.
    pub fn set_items(&mut self, items: Vec<TileItem>, spec: &GridSpec, now_ms: f64) {
        let keys: Vec<&str> = items.iter().map(|t| t.key.as_str()).collect();
        self.board.retarget(&keys, spec, now_ms);
        self.items = items;
    }

    /// Advance the board to `now_ms`. Returns true while anything is still
    /// moving — the shell's cue to keep requesting repaints.
    pub fn tick(&mut self, now_ms: f64) -> bool {
        self.board.tick(now_ms)
    }

    /// Live poses paired with the tile they belong to, in paint order.
    pub fn poses(&self) -> impl Iterator<Item = (&TileItem, TilePose)> {
        self.board.poses().filter_map(move |(key, pose)| self.items.iter().find(|t| t.key == key).map(|t| (t, pose)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hatch_core::anim::BoardConfig;
    use crate::tile::TileSource;

    fn spec() -> GridSpec {
        GridSpec::fit(800.0, 96.0, 64.0, 0.0)
    }

    fn item(key: &str) -> TileItem {
        TileItem {
            key: key.into(),
            label: key.into(),
            icon: String::new(),
            running: true,
            source: TileSource::Pinned,
            category: crate::category::TileCategory::Launcher,
        }
    }

    #[test]
    fn starts_collapsed_with_no_tiles() {
        let bar = Bar::new(BoardConfig::default());
        assert_eq!(bar.mode(), BarMode::Collapsed);
        assert_eq!(bar.poses().count(), 0);
    }

    #[test]
    fn expanding_does_not_reset_already_settled_tiles() {
        let mut zero = BoardConfig::default();
        zero.enter_ms = 0.0;
        zero.stagger_ms = 0.0;
        let mut bar = Bar::new(zero);
        bar.set_items(vec![item("a"), item("b")], &spec(), 0.0);
        bar.tick(0.0);
        assert!(bar.poses().all(|(_, p)| p.alpha >= 1.0), "zero-duration settles immediately");

        bar.set_expanded(true);
        assert_eq!(bar.poses().count(), 2, "the same tiles are still there after expanding");
    }

    #[test]
    fn poses_pair_up_with_the_right_tile() {
        let mut bar = Bar::new(BoardConfig::default());
        bar.set_items(vec![item("a"), item("b")], &spec(), 0.0);
        let keys: Vec<&str> = bar.poses().map(|(t, _)| t.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b"]);
    }
}
