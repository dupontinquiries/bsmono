// gridpick — a small headless picker: a search box filters a list of
// (name, tag) items down to a grid, and a cursor moves through *that*
// filtered grid, wrapping at the edges. One touch tap names a cell by
// index; one controller nudge moves the cursor by a row/column; one
// keystroke types into the search box. All three land in the same place.
//
// Adapted from games/clawbound's `lobby.rs` (`LobbyInput::Point/Move/Search`
// over a `grid()` the renderer and the hit-test both read) — that screen
// picks among the same thirteen ancestors this one does, so the shape
// carried over directly. This version is generic over any `(name, tag)`
// list, which is what lets the same tested code back both the ancestor
// select screen (name = ancestor, tag = role) and the shop's item grid
// (name = item, tag = its category), rather than writing the filter-and-
// cursor logic twice and letting the two drift.

/// One pass over `items`: which indices survive the search, in original
/// order. Case-insensitive substring match against either field, so typing
/// "guard" finds every Guardian even if no ancestor is *named* that.
pub fn filter<T>(items: &[T], search: &str, text: impl for<'a> Fn(&'a T) -> (&'a str, &'a str)) -> Vec<usize> {
    let want = search.trim().to_lowercase();
    (0..items.len())
        .filter(|&i| {
            if want.is_empty() {
                return true;
            }
            let (name, tag) = text(&items[i]);
            name.to_lowercase().contains(&want) || tag.to_lowercase().contains(&want)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct GridPicker {
    pub search: String,
    pub cursor: usize,
    pub cols: usize,
}

impl GridPicker {
    pub fn new(cols: usize) -> GridPicker {
        GridPicker { search: String::new(), cursor: 0, cols: cols.max(1) }
    }

    /// Filtered indices for the current search, against `items`.
    pub fn shown<T>(&self, items: &[T], text: impl for<'a> Fn(&'a T) -> (&'a str, &'a str)) -> Vec<usize> {
        filter(items, &self.search, text)
    }

    /// The index into `items` the cursor currently sits on, if anything matches.
    pub fn hovered<T>(&self, items: &[T], text: impl for<'a> Fn(&'a T) -> (&'a str, &'a str)) -> Option<usize> {
        let shown = self.shown(items, text);
        shown.get(self.cursor.min(shown.len().saturating_sub(1))).copied().filter(|_| !shown.is_empty())
    }

    /// Move the cursor by a grid step (`dx` columns, `dy` rows), wrapping in
    /// both directions so a d-pad or arrow key never runs into a wall.
    /// `len` is the number of cells currently shown (post-filter).
    pub fn move_cursor(&mut self, dx: i32, dy: i32, len: usize) {
        if len == 0 {
            return;
        }
        let cols = self.cols as i32;
        let c = self.cursor as i32 + dx + dy * cols;
        self.cursor = c.rem_euclid(len as i32) as usize;
    }

    /// A tap named this cell directly (already an index into the *shown* list).
    pub fn point(&mut self, cell: usize, len: usize) {
        if cell < len {
            self.cursor = cell;
        }
    }

    /// The search box changed. Resets the cursor — a shorter list under the
    /// old cursor position would otherwise silently jump to a different item.
    pub fn set_search(&mut self, s: String) {
        self.search = s;
        self.cursor = 0;
    }
}

impl Default for GridPicker {
    fn default() -> Self {
        GridPicker::new(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEMS: &[(&str, &str)] = &[("Goldenmane", "Warrior"), ("Duskstripe", "Assassin"), ("Snowhush", "Guardian"), ("Riptail", "Guardian")];
    fn text<'a>(i: &'a (&str, &str)) -> (&'a str, &'a str) {
        (i.0, i.1)
    }

    #[test]
    fn search_matches_name_or_tag_case_insensitively() {
        let mut p = GridPicker::new(3);
        assert_eq!(p.shown(ITEMS, text), vec![0, 1, 2, 3]);
        p.set_search("guard".into());
        assert_eq!(p.shown(ITEMS, text), vec![2, 3]);
        p.set_search("DUSK".into());
        assert_eq!(p.shown(ITEMS, text), vec![1]);
        p.set_search("nobody".into());
        assert!(p.shown(ITEMS, text).is_empty());
    }

    #[test]
    fn cursor_wraps_both_ways_and_search_resets_it() {
        let mut p = GridPicker::new(2);
        p.cursor = 3; // bottom-right of a 2x2-ish shown set
        p.move_cursor(1, 0, 4);
        assert_eq!(p.cursor, 0, "off the right edge wraps to the start");
        p.move_cursor(-1, 0, 4);
        assert_eq!(p.cursor, 3, "off the left edge wraps to the end");
        p.move_cursor(0, 1, 4);
        assert_eq!(p.cursor, 1, "down a row wraps too");
        p.cursor = 3;
        p.set_search("guard".into());
        assert_eq!(p.cursor, 0, "a narrower list must not leave the cursor past the end");
    }

    #[test]
    fn a_tap_names_a_cell_in_the_shown_list_directly() {
        let mut p = GridPicker::new(3);
        p.set_search("guard".into());
        p.point(1, p.shown(ITEMS, text).len());
        assert_eq!(p.hovered(ITEMS, text), Some(3), "cell 1 of the filtered pair is Riptail");
        p.point(9, 2); // out of range: ignored, cursor unchanged
        assert_eq!(p.cursor, 1);
    }

    #[test]
    fn hovered_is_none_on_an_empty_filter_not_a_panic() {
        let mut p = GridPicker::new(3);
        p.set_search("nobody".into());
        assert_eq!(p.hovered(ITEMS, text), None);
    }
}
