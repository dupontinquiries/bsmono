//! buffer — a `ropey::Rope` with the conversions an editor actually needs.
//!
//! Everything is addressed in **chars** (ropey's native unit), with (line, col)
//! only at the edges: the shell renders in lines, the model edits in chars.
//! All conversions clamp instead of panicking — a stale position from an
//! animation frame after an edit must degrade to "nearest valid", never abort.

use ropey::Rope;
use std::borrow::Cow;
use std::io;
use std::path::Path;

/// A (line, col) position in **char** units, 0-based. `col` may exceed the
/// line's length transiently (sticky columns); conversions clamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Pos { line, col }
    }
}

/// The text. Cheap to clone (rope), O(log n) to edit.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    rope: Rope,
}

impl Buffer {
    pub fn from_str(s: &str) -> Self {
        Buffer { rope: Rope::from_str(s) }
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Ok(Buffer::from_str(&s))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        std::fs::write(path, self.rope.to_string())
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Number of lines. ropey counts the empty line after a trailing `\n`,
    /// which is exactly editor semantics (the caret can sit there).
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// One line's content **without** its trailing newline. Borrowed when the
    /// rope chunk is contiguous (the common case for normal files).
    pub fn line(&self, idx: usize) -> Cow<'_, str> {
        if idx >= self.len_lines() {
            return Cow::Borrowed("");
        }
        let slice = self.rope.line(idx);
        let cow: Cow<str> = slice.into();
        match cow {
            Cow::Borrowed(s) => Cow::Borrowed(s.trim_end_matches(['\n', '\r'])),
            Cow::Owned(mut s) => {
                while s.ends_with('\n') || s.ends_with('\r') {
                    s.pop();
                }
                Cow::Owned(s)
            }
        }
    }

    /// Line length in chars, newline excluded.
    pub fn line_len(&self, idx: usize) -> usize {
        self.line(idx).chars().count()
    }

    /// Clamp a char index into the valid range.
    pub fn clamp(&self, ci: usize) -> usize {
        ci.min(self.len_chars())
    }

    pub fn char_to_pos(&self, ci: usize) -> Pos {
        let ci = self.clamp(ci);
        let line = self.rope.char_to_line(ci);
        let col = ci - self.rope.line_to_char(line);
        Pos { line, col }
    }

    /// Clamps both axes: line into the buffer, col into the line.
    pub fn pos_to_char(&self, p: Pos) -> usize {
        let line = p.line.min(self.len_lines().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        start + p.col.min(self.line_len(line))
    }

    pub fn char_at(&self, ci: usize) -> Option<char> {
        if ci < self.len_chars() {
            Some(self.rope.char(ci))
        } else {
            None
        }
    }

    /// Extract `range` (chars) as a String — used by ops to capture inverse text.
    pub fn slice_to_string(&self, start: usize, end: usize) -> String {
        let (start, end) = (self.clamp(start), self.clamp(end));
        self.rope.slice(start..end.max(start)).to_string()
    }

    pub fn insert(&mut self, at: usize, text: &str) {
        let at = self.clamp(at);
        self.rope.insert(at, text);
    }

    pub fn remove(&mut self, start: usize, end: usize) {
        let (start, end) = (self.clamp(start), self.clamp(end));
        if start < end {
            self.rope.remove(start..end);
        }
    }

    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_extraction_strips_newline_and_counts_trailing_empty_line() {
        let b = Buffer::from_str("alpha\nbeta\n");
        assert_eq!(b.len_lines(), 3); // "alpha", "beta", "" — caret can sit on the last
        assert_eq!(b.line(0), "alpha");
        assert_eq!(b.line(1), "beta");
        assert_eq!(b.line(2), "");
        assert_eq!(b.line(99), ""); // out of range degrades, never panics
    }

    #[test]
    fn pos_char_roundtrip_and_clamping() {
        let b = Buffer::from_str("ab\ncdé\n"); // é: one char, two bytes — chars, not bytes
        assert_eq!(b.char_to_pos(4), Pos::new(1, 1));
        assert_eq!(b.pos_to_char(Pos::new(1, 1)), 4);
        assert_eq!(b.pos_to_char(Pos::new(1, 99)), 6); // col clamps to line end ("cdé" = 3 chars)
        assert_eq!(b.pos_to_char(Pos::new(99, 0)), b.len_chars()); // line clamps to last (empty) line
    }

    #[test]
    fn edits_land_where_addressed() {
        let mut b = Buffer::from_str("hello world");
        b.insert(5, ",");
        assert_eq!(b.to_string(), "hello, world");
        b.remove(5, 6);
        assert_eq!(b.to_string(), "hello world");
        assert_eq!(b.slice_to_string(6, 11), "world");
    }
}
