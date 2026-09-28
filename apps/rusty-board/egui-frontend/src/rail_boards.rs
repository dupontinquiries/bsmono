//! rail_boards — which boards sit on the layout rail.
//!
//! By default every board the keyboard has (the code-built ones, the layout
//! files, and the user's own) is on the rail, and a vertical swipe pages
//! through them. The companion app lets a person trim that to the few they
//! use; the choice is `rail.boards` in `settings.conf`, the kept names
//! joined by `|`. Empty or absent means "all" — so a board added later
//! shows up without anyone having to tick it.

use std::collections::BTreeSet;

pub const KEY: &str = "rail.boards";

/// Where a rail entry comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// An index into `Board`'s layouts (code-built boards and user layouts).
    Board(usize),
    /// An index into `RealBoard::files`.
    File(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What `rail.boards` stores. NOT the name: a code-built board and a
    /// layout file can share one ("Stenotype" is both), and a trim by name
    /// would take both off at once. Files are `file:<name>`.
    pub id: String,
    /// What a person reads.
    pub name: String,
    pub source: Source,
}

/// Every board that could be on the rail, in rail order.
pub fn all(board: &crate::Board, real: &crate::real_layout::RealBoard) -> Vec<Entry> {
    let mut v: Vec<Entry> = board
        .layout_names()
        .iter()
        .enumerate()
        .map(|(i, n)| Entry { id: n.to_string(), name: n.to_string(), source: Source::Board(i) })
        .collect();
    for fi in real.tab_files() {
        let name = real.files[fi].display_name.clone();
        v.push(Entry { id: format!("file:{name}"), name, source: Source::File(fi) });
    }
    v
}

/// The kept names, or `None` for "all".
pub fn parse(value: &str) -> Option<BTreeSet<String>> {
    let set: BTreeSet<String> = value.split('|').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    (!set.is_empty()).then_some(set)
}

pub fn render(kept: &BTreeSet<String>) -> String {
    kept.iter().cloned().collect::<Vec<_>>().join("|")
}

/// The rail as shown: `all` trimmed to the kept names. A setting that
/// keeps nothing that exists (every kept board was deleted) shows all,
/// rather than an empty rail with no way to switch.
pub fn shown(all: Vec<Entry>, value: &str) -> Vec<Entry> {
    let Some(kept) = parse(value) else { return all };
    let trimmed: Vec<Entry> = all.iter().filter(|e| kept.contains(&e.id)).cloned().collect();
    if trimmed.is_empty() { all } else { trimmed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(n: &str, i: usize) -> Entry {
        Entry { id: n.into(), name: n.into(), source: Source::Board(i) }
    }

    #[test]
    fn empty_means_all_and_a_trim_keeps_order() {
        let all = vec![e("qwerty", 0), e("symbols", 1), e("steno", 2)];
        assert_eq!(shown(all.clone(), ""), all);
        let s = shown(all.clone(), "steno|qwerty");
        assert_eq!(s.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["qwerty", "steno"], "rail order, not setting order");
    }

    #[test]
    fn a_setting_that_names_nothing_that_exists_does_not_empty_the_rail() {
        let all = vec![e("qwerty", 0)];
        assert_eq!(shown(all.clone(), "deleted-board"), all);
    }

    #[test]
    fn round_trip() {
        let k: BTreeSet<String> = ["a", "b c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse(&render(&k)), Some(k));
        assert_eq!(parse(" | "), None);
    }
}
