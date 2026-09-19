// hold — what a long press on the Stack page lands on.
//
// The press itself is the kit's: `uikit_egui::touch::TouchSource` feeding
// touchkit's recogniser, which reports `LongPress(pos)` for a contact held still
// (and nothing at all while a widget owns the contact, so a press on a button
// stays a button press). This module is the other half: which thing was under
// that position. Rects are recorded while the page draws and read on the next
// frame's press, which is the only order immediate mode allows.

use egui::{Pos2, Rect};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Long-press a source line: breakpoint there.
    SourceLine { file: PathBuf, line: u32 },
    /// Long-press a local: copy `name = value`.
    Local { name: String, value: String },
    /// Long-press the frame title: copy `file:line`.
    Location(String),
}

#[derive(Default)]
pub struct Targets(Vec<(Rect, Target)>);

impl Targets {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn add(&mut self, rect: Rect, target: Target) {
        self.0.push((rect, target));
    }

    /// Last recorded wins, as with paint order: a later widget is on top.
    pub fn hit(&self, pos: Pos2) -> Option<&Target> {
        self.0.iter().rev().find(|(r, _)| r.contains(pos)).map(|(_, t)| t)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Source line under `y`, given the laid-out rows of a snippet. A source line
/// that wrapped occupies several rows; `ends_with_newline` marks the last row of
/// each, which is how rows map back to lines. `first_line` is the number of the
/// first line in the snippet.
pub fn line_at(rows: &[(Rect, bool)], first_line: u32, pos: Pos2) -> Option<u32> {
    let mut line = first_line;
    for (rect, ends_with_newline) in rows {
        if rect.y_range().contains(pos.y) && pos.x >= rect.left() {
            return Some(line);
        }
        if *ends_with_newline {
            line += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(y: f32, nl: bool) -> (Rect, bool) {
        (Rect::from_min_size(egui::pos2(10.0, y), egui::vec2(300.0, 14.0)), nl)
    }

    #[test]
    fn a_wrapped_source_line_is_one_line_however_many_rows_it_takes() {
        // line 7 (1 row), line 8 wrapped over 3 rows, line 9 (1 row)
        let rows = [row(0.0, true), row(14.0, false), row(28.0, false), row(42.0, true), row(56.0, true)];
        let at = |y| line_at(&rows, 7, egui::pos2(50.0, y));
        assert_eq!(at(5.0), Some(7));
        assert_eq!([at(15.0), at(30.0), at(50.0)], [Some(8), Some(8), Some(8)]);
        assert_eq!(at(60.0), Some(9));
        assert_eq!(at(100.0), None, "below the snippet");
        assert_eq!(line_at(&rows, 7, egui::pos2(2.0, 5.0)), None, "left of the text");
    }

    #[test]
    fn the_topmost_target_wins_and_a_miss_is_none() {
        let mut t = Targets::default();
        let big = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        t.add(big, Target::Location("main.rs:1".into()));
        t.add(big.shrink(30.0), Target::Local { name: "n".into(), value: "1".into() });
        assert!(matches!(t.hit(egui::pos2(50.0, 50.0)), Some(Target::Local { .. })));
        assert!(matches!(t.hit(egui::pos2(5.0, 5.0)), Some(Target::Location(_))));
        assert_eq!(t.hit(egui::pos2(500.0, 5.0)), None);
    }
}
