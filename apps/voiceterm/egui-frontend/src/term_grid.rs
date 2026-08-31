//! term_grid — the terminal screen, from `pty_term.rs`.
//!
//! [`crate::pty_input`] already turns keys into PTY bytes; this is the other
//! half — what the bytes coming back paint onto. Together they are a working
//! terminal.
//!
//! ## Wide characters need a phantom cell, not a wider cell
//!
//! A CJK glyph or an emoji occupies two columns. The original stores it in the
//! left cell and marks the right one **phantom**: it holds a space, is skipped
//! at render, and the wide glyph paints over both. Its comment gives the
//! reason — *"this is how we preserve column-precise cursor positioning even
//! when the terminal contains mixed-width content."*
//!
//! A grid that stored one wide char in one cell would drift: every wide glyph
//! would put the cursor one column left of where the shell believes it is, and
//! line editing would corrupt visibly.
//!
//! ## Dirty tracking is conservative on purpose
//!
//! *"Over-marking only costs a redundant re-render, under-marking would paint
//! stale content."* The asymmetry decides the design: `print` marks one row
//! precisely because it is the hot path, and every bulk or uncertain mutation
//! marks **all** rows.
//!
//! That is the right way round. A terminal that occasionally redraws too much
//! is a terminal; one that occasionally redraws too little shows you output
//! that is not there.

/// One screen cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    /// 0–255 palette index; 256+ encodes 24-bit RGB.
    pub fg: u16,
    pub bg: u16,
    /// bit 0 bold, 1 italic, 2 underline, 3 reverse, 4 strike, 5 dim,
    /// 6 blink, 7 phantom.
    pub attr: u8,
}

pub const ATTR_BOLD: u8 = 1 << 0;
pub const ATTR_ITALIC: u8 = 1 << 1;
pub const ATTR_UNDERLINE: u8 = 1 << 2;
pub const ATTR_REVERSE: u8 = 1 << 3;
pub const ATTR_STRIKE: u8 = 1 << 4;
pub const ATTR_DIM: u8 = 1 << 5;
pub const ATTR_BLINK: u8 = 1 << 6;
/// The right half of a wide glyph. Holds a space and is skipped at render.
pub const ATTR_PHANTOM: u8 = 1 << 7;

pub const DEFAULT_FG: u16 = 7;
pub const DEFAULT_BG: u16 = 0;

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: DEFAULT_FG, bg: DEFAULT_BG, attr: 0 }
    }
}

impl Cell {
    pub fn is_phantom(&self) -> bool { self.attr & ATTR_PHANTOM != 0 }
}

/// Columns a character occupies.
///
/// Two for the wide ranges, **zero for combining marks** — a combining accent
/// that advanced the cursor would push the rest of the line right by one column
/// per accent, which is how a terminal turns an accented word into a staircase.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if matches!(u, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F) {
        return 0;
    }
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF |
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF |
        0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F |
        0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 |
        0x1F300..=0x1F64F | 0x1F900..=0x1F9FF);
    if wide { 2 } else { 1 }
}

/// The screen.
pub struct TermGrid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Vec<Cell>>,
    cursor_x: usize,
    cursor_y: usize,
    dirty: bool,
    dirty_rows: Option<(usize, usize)>,
    pen: Cell,
}

impl TermGrid {
    pub fn new(cols: usize, rows: usize) -> TermGrid {
        // A zero-sized grid is reachable during layout before a size is known,
        // and every index below assumes at least one cell.
        let cols = cols.max(1);
        let rows = rows.max(1);
        TermGrid {
            cols, rows,
            cells: vec![vec![Cell::default(); cols]; rows],
            cursor_x: 0, cursor_y: 0,
            dirty: true, dirty_rows: Some((0, rows - 1)),
            pen: Cell::default(),
        }
    }

    pub fn cursor(&self) -> (usize, usize) { (self.cursor_x, self.cursor_y) }
    pub fn cell(&self, x: usize, y: usize) -> Option<&Cell> { self.cells.get(y)?.get(x) }
    pub fn is_dirty(&self) -> bool { self.dirty }
    pub fn dirty_rows(&self) -> Option<(usize, usize)> { self.dirty_rows }

    /// Clear the dirty state after a render.
    pub fn take_dirty(&mut self) -> Option<(usize, usize)> {
        self.dirty = false;
        self.dirty_rows.take()
    }

    fn mark_row(&mut self, row: usize) {
        self.dirty = true;
        self.dirty_rows = Some(match self.dirty_rows {
            None => (row, row),
            Some((a, b)) => (a.min(row), b.max(row)),
        });
    }

    /// Mark everything. Used by any bulk or uncertain mutation — over-marking
    /// costs a redundant redraw, under-marking paints stale content.
    fn mark_all(&mut self) {
        self.dirty = true;
        self.dirty_rows = Some((0, self.rows - 1));
    }

    /// Write one character at the cursor.
    pub fn print(&mut self, c: char) {
        let w = char_width(c);
        if w == 0 {
            // A combining mark attaches to the character already written; it
            // must not advance the cursor or occupy a cell of its own.
            return;
        }
        if self.cursor_x + w > self.cols {
            self.newline();
        }
        let (x, y) = (self.cursor_x, self.cursor_y);
        self.cells[y][x] = Cell { ch: c, ..self.pen };
        if w == 2 && x + 1 < self.cols {
            // The phantom holds a space so a naive text extraction reads a
            // sensible line, and carries the flag so render skips it.
            self.cells[y][x + 1] = Cell { ch: ' ', attr: self.pen.attr | ATTR_PHANTOM, ..self.pen };
        }
        self.cursor_x += w;
        self.mark_row(y);
    }

    pub fn newline(&mut self) {
        self.cursor_x = 0;
        if self.cursor_y + 1 >= self.rows { self.scroll_up(); }
        else { self.cursor_y += 1; }
    }

    /// Scroll one line. Marks every row, because every row moved.
    pub fn scroll_up(&mut self) {
        self.cells.remove(0);
        self.cells.push(vec![Cell::default(); self.cols]);
        self.mark_all();
    }

    pub fn carriage_return(&mut self) { self.cursor_x = 0; }

    /// Move the cursor, clamped to the screen.
    ///
    /// Clamped rather than trusted: an escape sequence can ask for row 999 on a
    /// 24-row screen, and a grid that indexed it would panic on somebody else's
    /// output.
    pub fn move_to(&mut self, x: usize, y: usize) {
        self.cursor_x = x.min(self.cols - 1);
        self.cursor_y = y.min(self.rows - 1);
    }

    /// Erase the whole screen and home the cursor.
    pub fn clear(&mut self) {
        for row in self.cells.iter_mut() {
            for c in row.iter_mut() { *c = Cell::default(); }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.mark_all();
    }

    /// Erase from the cursor to the end of the line.
    pub fn erase_to_eol(&mut self) {
        let (x, y) = (self.cursor_x, self.cursor_y);
        for c in self.cells[y][x..].iter_mut() { *c = Cell::default(); }
        self.mark_row(y);
    }

    /// Resize, preserving as much content as fits.
    ///
    /// Content is kept top-aligned rather than reflowed: reflowing is what a
    /// real terminal does and is a much larger job, and dropping the content
    /// entirely would clear the screen every time a phone rotates.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows { return; }
        let mut next = vec![vec![Cell::default(); cols]; rows];
        for (y, row) in self.cells.iter().take(rows).enumerate() {
            for (x, c) in row.iter().take(cols).enumerate() { next[y][x] = *c; }
        }
        self.cells = next;
        self.cols = cols;
        self.rows = rows;
        self.cursor_x = self.cursor_x.min(cols - 1);
        self.cursor_y = self.cursor_y.min(rows - 1);
        self.mark_all();
    }

    /// Set the pen from an SGR parameter.
    pub fn sgr(&mut self, param: u16) {
        match param {
            0 => self.pen = Cell::default(),
            1 => self.pen.attr |= ATTR_BOLD,
            2 => self.pen.attr |= ATTR_DIM,
            3 => self.pen.attr |= ATTR_ITALIC,
            4 => self.pen.attr |= ATTR_UNDERLINE,
            5 => self.pen.attr |= ATTR_BLINK,
            7 => self.pen.attr |= ATTR_REVERSE,
            9 => self.pen.attr |= ATTR_STRIKE,
            22 => self.pen.attr &= !(ATTR_BOLD | ATTR_DIM),
            23 => self.pen.attr &= !ATTR_ITALIC,
            24 => self.pen.attr &= !ATTR_UNDERLINE,
            27 => self.pen.attr &= !ATTR_REVERSE,
            30..=37 => self.pen.fg = param - 30,
            39 => self.pen.fg = DEFAULT_FG,
            40..=47 => self.pen.bg = param - 40,
            49 => self.pen.bg = DEFAULT_BG,
            90..=97 => self.pen.fg = param - 90 + 8,
            100..=107 => self.pen.bg = param - 100 + 8,
            _ => {}
        }
    }

    /// A row as text, skipping phantoms so a wide glyph appears once.
    pub fn row_text(&self, y: usize) -> String {
        match self.cells.get(y) {
            None => String::new(),
            Some(row) => row.iter().filter(|c| !c.is_phantom()).map(|c| c.ch).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printing_advances_and_marks_only_its_own_row() {
        // print is the hot path and marks precisely; everything else is
        // conservative.
        let mut g = TermGrid::new(10, 4);
        g.take_dirty();
        g.move_to(0, 2);
        g.print('a');
        assert_eq!(g.cell(0, 2).unwrap().ch, 'a');
        assert_eq!(g.cursor(), (1, 2));
        assert_eq!(g.dirty_rows(), Some((2, 2)));
    }

    #[test]
    fn a_wide_glyph_takes_two_columns_with_a_phantom_on_the_right() {
        // "This is how we preserve column-precise cursor positioning even when
        // the terminal contains mixed-width content." A grid storing one wide
        // char in one cell drifts by a column per glyph, and line editing
        // corrupts visibly.
        let mut g = TermGrid::new(10, 2);
        g.print('中');
        assert_eq!(g.cell(0, 0).unwrap().ch, '中');
        assert!(g.cell(1, 0).unwrap().is_phantom());
        assert_eq!(g.cursor(), (2, 0), "cursor did not clear the wide glyph");
    }

    #[test]
    fn row_text_reads_a_wide_glyph_once() {
        // The phantom holds a space so naive extraction is sensible, and is
        // skipped so it does not double.
        let mut g = TermGrid::new(10, 2);
        g.print('中');
        g.print('x');
        assert!(g.row_text(0).starts_with("中x"), "{:?}", g.row_text(0));
    }

    #[test]
    fn a_combining_mark_does_not_advance_the_cursor() {
        // An accent that advanced would push the rest of the line right by one
        // column per accent — how a terminal turns an accented word into a
        // staircase.
        let mut g = TermGrid::new(10, 2);
        g.print('e');
        let before = g.cursor();
        g.print('\u{0301}');
        assert_eq!(g.cursor(), before);
        assert_eq!(char_width('\u{0301}'), 0);
    }

    #[test]
    fn a_wide_glyph_at_the_last_column_wraps_rather_than_splitting() {
        // Half a glyph on each line is unreadable and leaves the cursor in a
        // column the shell does not believe in.
        let mut g = TermGrid::new(3, 3);
        g.print('a');
        g.print('b');
        g.print('中');
        assert_eq!(g.cursor(), (2, 1), "did not wrap: {:?}", g.cursor());
        assert_eq!(g.cell(0, 1).unwrap().ch, '中');
    }

    #[test]
    fn scrolling_marks_every_row_because_every_row_moved() {
        // "Over-marking only costs a redundant re-render, under-marking would
        // paint stale content."
        let mut g = TermGrid::new(4, 3);
        g.take_dirty();
        g.scroll_up();
        assert_eq!(g.dirty_rows(), Some((0, 2)));
    }

    #[test]
    fn output_past_the_last_row_scrolls_rather_than_growing() {
        let mut g = TermGrid::new(4, 2);
        g.print('a'); g.newline();
        g.print('b'); g.newline();
        g.print('c');
        assert_eq!(g.rows, 2);
        assert!(g.row_text(0).starts_with('b'), "top line did not scroll off");
        assert!(g.row_text(1).starts_with('c'));
    }

    #[test]
    fn an_escape_asking_for_a_row_off_screen_is_clamped_not_indexed() {
        // Somebody else's output should not panic the terminal.
        let mut g = TermGrid::new(10, 5);
        g.move_to(999, 999);
        assert_eq!(g.cursor(), (9, 4));
        g.print('x');
        assert_eq!(g.cell(9, 4).unwrap().ch, 'x');
    }

    #[test]
    fn erase_to_end_of_line_leaves_what_is_before_the_cursor() {
        let mut g = TermGrid::new(6, 2);
        for c in "abcdef".chars() { g.print(c); }
        g.move_to(3, 0);
        g.erase_to_eol();
        assert_eq!(g.row_text(0), "abc   ");
    }

    #[test]
    fn resizing_keeps_the_content_that_fits() {
        // Dropping it entirely would clear the screen every time a phone
        // rotates, which is the case this port exists for.
        let mut g = TermGrid::new(10, 4);
        for c in "hello".chars() { g.print(c); }
        g.resize(6, 2);
        assert_eq!(g.cols, 6);
        assert!(g.row_text(0).starts_with("hello"));
    }

    #[test]
    fn a_shrink_pulls_the_cursor_back_inside_the_screen() {
        // A cursor outside the grid is a panic on the next print.
        let mut g = TermGrid::new(20, 10);
        g.move_to(19, 9);
        g.resize(5, 3);
        assert_eq!(g.cursor(), (4, 2));
        g.print('x');
    }

    #[test]
    fn a_zero_sized_grid_is_clamped_rather_than_empty() {
        // Reachable during layout before a size is known, and every index
        // assumes at least one cell.
        let mut g = TermGrid::new(0, 0);
        assert!(g.cols >= 1 && g.rows >= 1);
        g.print('a');
        assert_eq!(g.cell(0, 0).unwrap().ch, 'a');
    }

    #[test]
    fn sgr_zero_resets_every_attribute_and_colour() {
        // A reset that left a colour behind tints everything after it.
        let mut g = TermGrid::new(8, 2);
        g.sgr(1); g.sgr(4); g.sgr(31); g.sgr(44);
        g.sgr(0);
        g.print('x');
        let c = g.cell(0, 0).unwrap();
        assert_eq!(c.attr, 0);
        assert_eq!((c.fg, c.bg), (DEFAULT_FG, DEFAULT_BG));
    }

    #[test]
    fn bold_off_does_not_clear_the_other_attributes() {
        // SGR 22 turns off bold and dim only; clearing everything is the bug
        // that makes underlines vanish mid-line.
        let mut g = TermGrid::new(8, 2);
        g.sgr(1); g.sgr(4);
        g.sgr(22);
        g.print('x');
        let c = g.cell(0, 0).unwrap();
        assert_eq!(c.attr & ATTR_BOLD, 0);
        assert_ne!(c.attr & ATTR_UNDERLINE, 0, "underline was cleared by bold-off");
    }

    #[test]
    fn bright_colours_land_in_the_upper_palette_half() {
        let mut g = TermGrid::new(4, 2);
        g.sgr(91);
        g.print('x');
        assert_eq!(g.cell(0, 0).unwrap().fg, 9, "bright red is palette 9");
    }

    #[test]
    fn taking_the_dirty_state_clears_it() {
        // A flag that never clears makes every frame a full redraw, which is
        // exactly the cost the dirty tracking exists to avoid.
        let mut g = TermGrid::new(4, 2);
        assert!(g.is_dirty(), "a new grid needs its first paint");
        g.take_dirty();
        assert!(!g.is_dirty());
        assert_eq!(g.dirty_rows(), None);
    }
}

// ── the VT parser ───────────────────────────────────────────────────────────

/// Feed terminal output into a grid.
///
/// This lived inside the binary as `feed_pty`, which made the crate
/// header's claim — "a portable terminal core, a screen-grid model plus a
/// VT/ANSI parser, pure and unit-tested, with no egui or PTY in it" —
/// true of the grid and false of the parser. It was reachable only from a
/// method on the app, so nothing could test it and nothing else could use
/// it. Moving it here costs nothing and makes the sentence true.
///
/// Stateless between calls in one respect that matters and is wrong: an
/// escape sequence split across two reads is lost, because parsing starts
/// fresh each time. A pty hands over whatever arrived, and a 4KB boundary
/// falls in the middle of a colour code eventually. [`Vt`] below carries
/// the partial sequence across calls, which is why it is a struct rather
/// than a function.
#[derive(Debug, Clone, Default)]
pub struct Vt {
    /// An escape sequence that has begun and not finished.
    pending: String,
}

impl Vt {
    pub fn new() -> Vt { Vt::default() }

    /// Feed bytes. Text is printed, control codes and CSI sequences acted on.
    pub fn feed(&mut self, grid: &mut TermGrid, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        // Anything left mid-sequence from last time goes in front.
        let text = if self.pending.is_empty() {
            text
        } else {
            let mut t = std::mem::take(&mut self.pending);
            t.push_str(&text);
            t
        };
        let mut it = text.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '\x1b' => {
                    // Collect the whole sequence first, so an unterminated
                    // one can be held rather than half-applied.
                    let mut seq = String::from('\x1b');
                    if it.peek() == Some(&'[') {
                        seq.push(it.next().unwrap());
                        let mut done = false;
                        for c2 in it.by_ref() {
                            seq.push(c2);
                            if c2.is_ascii_alphabetic() { done = true; break; }
                        }
                        if !done {
                            // Split across reads. Held for the next call —
                            // dropping it turns a colour code into visible
                            // garbage on screen, which is what a 4KB
                            // boundary through an SGR sequence produces.
                            self.pending = seq;
                            return;
                        }
                        let body = &seq[2..seq.len() - 1];
                        let final_byte = seq.chars().last().unwrap();
                        csi(grid, body, final_byte);
                    } else if it.peek().is_none() {
                        // A bare ESC at the very end: also possibly a split.
                        self.pending = seq;
                        return;
                    }
                    // Any other introducer is dropped, as before.
                }
                '\r' => grid.carriage_return(),
                '\n' => grid.newline(),
                '\t' => {
                    // Next 8-column stop. Printing spaces rather than
                    // moving, so erase_to_eol still clears what a tab
                    // covered.
                    let (x, _) = grid.cursor();
                    for _ in 0..(8 - (x % 8)) { grid.print(' '); }
                }
                '\x08' => {
                    let (x, y) = grid.cursor();
                    grid.move_to(x.saturating_sub(1), y);
                }
                c if (c as u32) < 0x20 => {}   // other C0: ignored, not drawn
                c => grid.print(c),
            }
        }
    }
}

/// Apply one CSI sequence. `body` is what sits between `[` and the final byte.
fn csi(grid: &mut TermGrid, body: &str, final_byte: char) {
    // A private-mode sequence — `[?25l`, `[?1049h` — is addressed to the
    // terminal, not the screen. Ignored rather than parsed as numbers,
    // because `?25` parses as nothing and would land as a default.
    if body.starts_with('?') {
        return;
    }
    let n = |d: usize| body.split(';').next()
        .and_then(|x| x.parse::<usize>().ok()).unwrap_or(d);
    match final_byte {
        'H' | 'f' => {
            // 1-based on the wire, 0-based in the grid.
            let mut p = body.split(';');
            let row = p.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(1);
            let col = p.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(1);
            grid.move_to(col.saturating_sub(1), row.saturating_sub(1));
        }
        'J' => grid.clear(),
        'K' => grid.erase_to_eol(),
        'm' => {
            let mut any = false;
            for part in body.split(';') {
                if let Ok(v) = part.parse::<u16>() { grid.sgr(v); any = true; }
            }
            // `[m` and `[0m` both mean reset. An empty body parsed as
            // nothing and left the attributes on, so `ls --color` painted
            // the rest of the screen in the last file's colour.
            if !any { grid.sgr(0); }
        }
        'A' | 'B' | 'C' | 'D' => {
            let (x, y) = grid.cursor();
            let d = n(1).max(1);
            let (nx, ny) = match final_byte {
                'A' => (x, y.saturating_sub(d)),
                'B' => (x, y + d),
                'C' => (x + d, y),
                _ => (x.saturating_sub(d), y),
            };
            grid.move_to(nx, ny);
        }
        _ => {}
    }
}

#[cfg(test)]
mod vt_tests {
    use super::*;

    fn text_of(g: &TermGrid, y: usize) -> String {
        let mut s = String::new();
        for x in 0..g.cols { if let Some(c) = g.cell(x, y) { s.push(c.ch); } }
        s.trim_end().to_string()
    }

    #[test]
    fn a_sequence_split_between_two_reads_is_not_printed_as_text() {
        // A pty hands over whatever arrived, and a 4KB boundary falls
        // inside a colour code eventually. Parsing each read from scratch
        // dropped the opening half and printed the rest — "31mred" on the
        // screen. Held across calls now, which is why `Vt` is a struct.
        let mut g = TermGrid::new(40, 4);
        let mut vt = Vt::new();
        vt.feed(&mut g, b"a\x1b[3");
        vt.feed(&mut g, b"1mred");
        assert_eq!(text_of(&g, 0), "ared", "{:?}", text_of(&g, 0));
    }

    #[test]
    fn a_bare_reset_resets() {
        // `[m` and `[0m` both mean reset. An empty body parsed as no
        // numbers at all and left the attributes on, so the rest of the
        // screen kept the last colour — which is exactly what `ls --color`
        // ends its output with.
        let mut g = TermGrid::new(40, 4);
        let mut vt = Vt::new();
        vt.feed(&mut g, b"\x1b[1;32mgreen\x1b[mplain");
        // "green" is five cells, so "plain" starts at 5 — I miscounted
        // and landed on 'n' the first time.
        let plain = g.cell(5, 0).expect("a cell");
        assert_eq!(plain.ch, 'p', "the row is {:?}", text_of(&g, 0));
        let bold_bit = g.cell(0, 0).map(|c| c.attr).unwrap_or(0);
        assert_ne!(bold_bit, plain.attr, "the reset did not take");
    }

    #[test]
    fn a_private_mode_sequence_is_ignored_rather_than_misread() {
        // `[?25l` hides the cursor and `[?1049h` switches to the alternate
        // screen — addressed to the terminal, not the page. `?25` parses
        // as no number, so a naive reader takes the DEFAULT and moves the
        // cursor or clears the screen on a sequence that asked for
        // neither.
        let mut g = TermGrid::new(40, 4);
        let mut vt = Vt::new();
        vt.feed(&mut g, b"hello\x1b[?25l\x1b[?1049h world");
        assert_eq!(text_of(&g, 0), "hello world", "{:?}", text_of(&g, 0));
    }

    #[test]
    fn real_ls_colour_output_leaves_no_escape_text_on_screen() {
        // Shape taken from `ls --color=always` as captured on this
        // machine: the only sequences it emits are SGR colour codes and a
        // bare reset.
        let mut g = TermGrid::new(40, 4);
        let mut vt = Vt::new();
        vt.feed(&mut g, "\x1b[0m\x1b[01;34mdir\x1b[0m  \x1b[01;32mexe\x1b[0m\n".as_bytes());
        let row = text_of(&g, 0);
        assert_eq!(row, "dir  exe", "{row:?}");
        assert!(!row.contains('['), "escape text reached the screen: {row:?}");
    }

    #[test]
    fn a_cursor_move_of_zero_still_moves_one() {
        // `[0A` means one row, not none — the parameter's default applies
        // to an absent value AND to zero, per the spec. Reading it as zero
        // makes a redrawing program overwrite its own line.
        let mut g = TermGrid::new(20, 5);
        let mut vt = Vt::new();
        vt.feed(&mut g, b"\x1b[3;1Hx\x1b[0A");
        assert_eq!(g.cursor().1, 1, "cursor row {:?}", g.cursor());
    }
}
