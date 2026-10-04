//! uikit-term — a shared egui terminal widget.
//!
//! A portable screen-grid model ([`Grid`]) + a VT/ANSI [`Parser`] (printable text, C0 controls,
//! CSI cursor/erase/SGR) — both pure and unit-tested, no egui or PTY — plus a themed run-length
//! cell renderer ([`render_grid`]) and a small [`App`] holder. Colours resolve through the
//! uikit-egui [`PaletteColors`](uikit_egui::PaletteColors) and the display-accessibility
//! [`filter_color`](uikit_egui::filter_color), and pinch-zoom rides the shared touch layer — so an
//! embedding app gets a terminal that already matches the fleet's theme, a11y, and gesture feel.
//!
//! Lifted out of `voiceterm-egui` (which now re-exports this) so logscope, gitmind, or any egui app
//! can embed a shell/terminal in a few lines. The caller wires a PTY to [`Terminal::feed_bytes`]
//! and calls [`App::render`] each frame; [`Terminal::stage`] drops a command on the input line
//! (the cross-app action-protocol seam) without running it.

pub mod keys;
pub mod blocks;
pub mod snapshot;

// ── colours + style ──────────────────────────────────────────────────────────

/// A terminal colour: the theme default, a palette index, or a direct colour.
///
/// `Ansi` is the FULL 256-entry palette, not just 0–15. Widening its meaning
/// rather than adding a second indexed variant keeps `SGR 31` and
/// `SGR 38;5;1` landing on the same value — two representations of one index
/// is the kind of ambiguity that makes a later comparison quietly wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermColor {
    Default,
    /// xterm palette index: 0–7 normal, 8–15 bright, 16–231 the 6x6x6 cube,
    /// 232–255 the greyscale ramp.
    Ansi(u8),
    /// A direct 24-bit colour (`SGR 38;2;r;g;b`).
    Rgb(u8, u8, u8),
}

/// A cell's visual attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub inverse: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style { fg: TermColor::Default, bg: TermColor::Default, bold: false, inverse: false }
    }
}

/// The filler in the second column of a double-width glyph.
///
/// The cell must be OCCUPIED — otherwise the next `put` lands on top of the
/// glyph's right half — but it holds no character of its own. It used to be a
/// space, which made `row_text` read back one more column than the screen
/// shows: `世界` came out as `"世 界"`, so search, copy and every consumer of
/// the text saw a word that is not on screen.
///
/// A sentinel rather than a flag on [`Cell`]: every reader already handles
/// `ch`, so one skip in the three places that assemble text is the whole
/// change, and `Cell` stays `Copy` and two fields wide.
pub const WIDE_TAIL: char = '\0';

/// The primary screen while the alternate one is in front of it.
#[derive(Debug, Clone)]
struct AltSave {
    cells: Vec<Cell>,
    /// The parked primary's soft-wrap flags, so it can be REFLOWED on a resize
    /// while a full-screen program has the alternate screen.
    wrapped: Vec<bool>,
    cur_row: usize,
    cur_col: usize,
    pen: Style,
    scroll_top: usize,
    scroll_bot: usize,
}

/// One screen cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', style: Style::default() }
    }
}

// ── the screen grid ──────────────────────────────────────────────────────────

/// A fixed-size character grid with a cursor and current pen style. Mutated by the parser.
#[derive(Debug, Clone)]

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    pub cur_row: usize,
    pub cur_col: usize,
    pub pen: Style,
    /// Lines that scrolled off the top (most-recent last), capped.
    pub scrollback: Vec<Vec<Cell>>,
    scrollback_cap: usize,
    /// Per screen row: `true` when the row ended because text reached the right
    /// margin and CONTINUED on the next row (a soft wrap), `false` for a real
    /// line end. The one bit reflow needs and nothing recorded: without it a
    /// wrapped line and two short lines are indistinguishable, so a resize can
    /// only truncate -- and growing the pane back never restored the text.
    /// Moves with its row through every scroll; cleared when a row's end is
    /// erased (how a shell redraws a line).
    wrapped: Vec<bool>,
    /// The same flag for each `scrollback` row, kept parallel by `sb_push`.
    scrollback_wrapped: Vec<bool>,
    /// DEC private modes the KEY ENCODER needs.
    ///
    /// These live on the grid because the program running in the terminal sets
    /// them (`CSI ? 1 h`, `CSI ? 2004 h`) and the encoder reads them: the same
    /// arrow key is two different byte sequences depending on what the
    /// foreground process asked for. Before this, the parser dropped the `?`
    /// as an "intermediate byte" and dispatched `CSI ? 1 h` as a plain
    /// `CSI 1 h`, so DECCKM never arrived — and `keys::encode_mode`'s
    /// application-cursor branch, ported specifically to fix arrow navigation
    /// inside fzf and vim, could never be reached.
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// The scrolling region (DECSTBM), 0-based and INCLUSIVE: the band of rows
    /// a line feed scrolls, leaving everything outside it fixed. Defaults to
    /// the whole screen, which is what every existing caller assumed.
    ///
    /// Without this, a program that reserves a status line — less, vim, htop,
    /// anything full-screen — had its region ignored, so a scroll meant for
    /// part of the screen moved all of it and the status line marched off the
    /// top.
    pub scroll_top: usize,
    pub scroll_bot: usize,
    /// The cursor and pen parked by DECSC (`ESC 7`) or SCOSC (`CSI s`), for
    /// the matching restore. Prompt-drawing code parks the cursor, writes
    /// somewhere else and expects to come back; with this dropped, it came
    /// back to wherever it happened to be.
    saved_cursor: Option<(usize, usize, Style)>,
    /// The PRIMARY screen, parked while the alternate one is showing.
    ///
    /// `Some` IS the "alternate screen is active" flag — one field rather than
    /// a bool beside a buffer, so the two cannot disagree about which screen
    /// is on. Full-screen programs (vim, less, htop) switch to the alternate
    /// screen, draw, and switch back expecting to find the shell exactly as
    /// they left it; without this they drew over the shell's output and it was
    /// gone for good.
    alt_saved: Option<AltSave>,
    /// Scrollback view: how many lines ABOVE the live bottom the viewport is showing.
    /// 0 = live. While scrolled back, new output does not move the view (the offset is bumped
    /// as lines enter scrollback — the standard terminal pin), and the cursor is hidden.
    pub view_offset: usize,
}

impl Grid {
    /// The modes the key encoder needs, in the shape it wants them.
    ///
    /// One accessor so a caller cannot read one flag and forget the other —
    /// which is how a terminal ends up with correct arrows and broken paste.
    pub fn key_mode(&self) -> crate::keys::TermMode {
        crate::keys::TermMode {
            app_cursor: self.app_cursor,
            bracketed_paste: self.bracketed_paste,
        }
    }

    pub fn new(cols: usize, rows: usize) -> Grid {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Grid {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            cur_row: 0,
            cur_col: 0,
            pen: Style::default(),
            scrollback: Vec::new(),
            scrollback_cap: 1000,
            wrapped: vec![false; rows],
            scrollback_wrapped: Vec::new(),
            app_cursor: false,
            bracketed_paste: false,
            scroll_top: 0,
            scroll_bot: rows - 1,
            saved_cursor: None,
            alt_saved: None,
            view_offset: 0,
        }
    }

    fn idx(&self, r: usize, c: usize) -> usize {
        r * self.cols + c
    }

    /// Buffer length, so a test can catch a screen restored at the wrong size
    /// (which otherwise shows up as an unrelated panic several operations
    /// later).
    #[doc(hidden)]
    pub fn cells_len_for_test(&self) -> usize {
        self.cells.len()
    }

    pub fn cell(&self, r: usize, c: usize) -> Cell {
        if r < self.rows && c < self.cols {
            self.cells[self.idx(r, c)]
        } else {
            Cell::default()
        }
    }

    /// The visible text of a row, trailing blanks trimmed.
    pub fn row_text(&self, r: usize) -> String {
        if r >= self.rows {
            return String::new();
        }
        let start = self.idx(r, 0);
        let s: String = self.cells[start..start + self.cols].iter()
            .map(|c| c.ch).filter(|c| *c != WIDE_TAIL).collect();
        s.trim_end().to_string()
    }

    /// Every line as text — scrollback (oldest first) then the visible rows. The unit of search and
    /// copy; line 0 is the oldest scrollback row.
    pub fn all_lines(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .scrollback
            .iter()
            .map(|row| row.iter().map(|c| c.ch).filter(|c| *c != WIDE_TAIL)
                .collect::<String>().trim_end().to_string())
            .collect();
        for r in 0..self.rows {
            v.push(self.row_text(r));
        }
        v
    }

    /// Columns a character occupies: 2 for the wide ranges, **0 for combining
    /// marks**, 1 otherwise.
    ///
    /// The zero case is the one that bites. A combining accent that advanced
    /// the cursor pushes the rest of the line right by one column PER ACCENT —
    /// which is how a terminal turns an accented word into a staircase. And a
    /// CJK glyph advancing by one leaves the next character painted over its
    /// right half.
    ///
    /// Ported from voiceterm's `term_grid`, which had this table and no caller.
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

    /// Write a printable char at the cursor and advance (wrapping + scrolling as needed).
    pub fn put(&mut self, ch: char) {
        let w = Self::char_width(ch);

        // A combining mark belongs to the character BEFORE it and must not
        // occupy a cell of its own. Dropping it would lose the accent; taking a
        // cell would shift the line. Neither is right, so it is skipped here —
        // the cell keeps its base character, which is what a renderer without
        // grapheme clustering can actually draw.
        if w == 0 { return; }

        // A wide glyph needs two columns, so it must not START in the last one:
        // half of it would land on the next row. Wrapping first keeps it whole.
        if self.cur_col + w > self.cols {
            // A SOFT wrap: this row continues on the next. Recorded BEFORE the
            // line feed, so if it scrolls the flag travels with the row.
            self.wrapped[self.cur_row] = true;
            self.cur_col = 0;
            self.line_feed();
        }
        let i = self.idx(self.cur_row, self.cur_col);
        self.cells[i] = Cell { ch, style: self.pen };
        if w == 2 {
            // The second column is a PHANTOM: it holds no character of its own
            // but must be occupied, or the next put lands on top of the wide
            // glyph's right half. A space is the honest filler — row_text then
            // reads back the same visual width the screen shows.
            let j = self.idx(self.cur_row, self.cur_col + 1);
            self.cells[j] = Cell { ch: WIDE_TAIL, style: self.pen };
        }
        self.cur_col += w;
    }

    /// Move down one line, scrolling if at the bottom.
    pub fn line_feed(&mut self) {
        // At the region's BOTTOM, scroll the region. Not `rows - 1`: with a
        // region set, the last row of the screen is outside it and must not
        // move, and a cursor already below the region just walks down.
        if self.cur_row == self.scroll_bot {
            self.scroll_up();
        } else if self.cur_row + 1 < self.rows {
            self.cur_row += 1;
        }
    }

    /// Reverse index (`ESC M`): up one, scrolling the region DOWN at its top.
    /// What a program uses to draw upward, and what `less` uses to scroll back.
    pub fn reverse_index(&mut self) {
        if self.cur_row == self.scroll_top {
            self.scroll_region_down(1);
        } else {
            self.cur_row = self.cur_row.saturating_sub(1);
        }
    }

    /// Is the alternate screen showing?
    pub fn on_alt_screen(&self) -> bool {
        self.alt_saved.is_some()
    }

    /// Switch to the alternate screen (`CSI ?1049h` / `?1047h` / `?47h`).
    ///
    /// Idempotent: a second enter while already on the alternate screen must
    /// NOT re-save, or the primary buffer is overwritten with the alternate
    /// one and the shell's output is lost — which is the exact failure this
    /// exists to prevent, arriving through the mechanism meant to prevent it.
    /// Programs do send a redundant enter (a nested pager under an editor).
    ///
    /// `save_cursor` decides whether the cursor rides along: `?1049` saves it,
    /// the older `?47`/`?1047` do not.
    pub fn enter_alt_screen(&mut self, save_cursor: bool) {
        if self.alt_saved.is_some() {
            return;
        }
        self.alt_saved = Some(AltSave {
            cells: self.cells.clone(),
            wrapped: self.wrapped.clone(),
            cur_row: self.cur_row,
            cur_col: self.cur_col,
            pen: self.pen,
            scroll_top: self.scroll_top,
            scroll_bot: self.scroll_bot,
        });
        self.scroll_top = 0;
        self.scroll_bot = self.rows - 1;
        if !save_cursor {
            // `?47h` / `?1047h` swap the buffer and DO NOTHING ELSE: no cursor
            // save, no clear, no home. Checked against tmux, where the
            // character after `?47l` lands at the column the alternate screen
            // reached rather than the one the primary left. Clearing and
            // homing here felt obviously right and was obviously wrong — the
            // difference between these and `?1049` is precisely that `?1049`
            // was added to do the extra work.
            return;
        }
        self.save_cursor();
        // The alternate screen starts blank. Not `clear()`, which would also
        // home the cursor — the save above has to happen first, and doing the
        // two steps by hand makes that ordering visible.
        for c in &mut self.cells {
            *c = Cell::default();
        }
        self.wrapped.iter_mut().for_each(|w| *w = false);
        self.cur_row = 0;
        self.cur_col = 0;
    }

    /// Switch back, putting the primary screen and its cursor back exactly.
    /// A leave with no alternate screen active is a no-op rather than a clear:
    /// a stray `?1049l` from a program that crashed mid-switch must not eat
    /// the shell's scrollback position.
    pub fn leave_alt_screen(&mut self, restore_cursor: bool) {
        let Some(saved) = self.alt_saved.take() else { return };
        self.cells = saved.cells;
        self.wrapped = saved.wrapped;
        self.scroll_top = saved.scroll_top;
        self.scroll_bot = saved.scroll_bot;
        if restore_cursor {
            self.restore_cursor();
        }
        // Otherwise the cursor is LEFT WHERE THE ALTERNATE SCREEN PUT IT.
        //
        // That looks like an omission and is not: `?47`/`?1047` restore the
        // buffer and say nothing about the cursor, which is exactly why
        // `?1049` was added. Checked against tmux — after `?47l` the next
        // character lands at the column the alternate screen had reached, not
        // at the one the primary left. Putting `saved.cur_*` back here would
        // be tidier and wrong, and the fixture catches it.
        let _ = (saved.cur_row, saved.cur_col, saved.pen);
    }

    /// DECSC / SCOSC — park the cursor AND the pen.
    ///
    /// The pen matters and is easy to leave out: a prompt that saves, switches
    /// to a colour to draw something elsewhere, then restores, expects its
    /// original colour back. Saving only the position gives the cursor back
    /// and leaves the rest of the line painted in whatever the excursion used.
    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some((self.cur_row, self.cur_col, self.pen));
    }

    /// DECRC / SCORC. A restore with nothing saved homes the cursor, which is
    /// what a VT does — not a no-op, which would leave it wherever a
    /// half-understood sequence had wandered.
    pub fn restore_cursor(&mut self) {
        match self.saved_cursor {
            Some((r, c, pen)) => {
                self.cur_row = r.min(self.rows - 1);
                self.cur_col = c.min(self.cols - 1);
                self.pen = pen;
            }
            None => {
                self.cur_row = 0;
                self.cur_col = 0;
            }
        }
    }

    /// DECALN (`ESC # 8`) — fill the screen with `E` and home the cursor. A
    /// diagnostic pattern, rare in the wild, and here because the parser has to
    /// consume the `#` either way: half-understanding it printed the `8`.
    pub fn screen_alignment_test(&mut self) {
        for c in &mut self.cells {
            *c = Cell { ch: 'E', style: Style::default() };
        }
        self.cur_row = 0;
        self.cur_col = 0;
    }

    /// Set the scrolling region (DECSTBM), 1-based inclusive as the protocol
    /// sends it. A region that would be empty or inverted is REFUSED — the
    /// whole screen is restored instead, which is what `CSI r` with no
    /// parameters means and what a program sends to undo one.
    pub fn set_scroll_region(&mut self, top: u16, bot: u16) {
        let t = (top.max(1) as usize - 1).min(self.rows - 1);
        let b = if bot == 0 { self.rows - 1 } else { (bot as usize - 1).min(self.rows - 1) };
        if t >= b {
            self.scroll_top = 0;
            self.scroll_bot = self.rows - 1;
        } else {
            self.scroll_top = t;
            self.scroll_bot = b;
        }
        // DECSTBM homes the cursor. Programs rely on it: the usual sequence is
        // "set the region, then draw from the top of it" with no explicit move.
        self.cur_row = 0;
        self.cur_col = 0;
    }

    /// Move the rows of `[top, bot]` up by `n`, blanking the `n` freed at the
    /// bottom. The primitive under a line feed at the region's foot, `SU`, and
    /// `DL` (which passes the cursor's row as `top`).
    fn shift_up(&mut self, top: usize, bot: usize, n: usize) {
        if top > bot || n == 0 { return; }
        let n = n.min(bot - top + 1);
        for r in top..=bot - n {
            let (src, dst) = (self.idx(r + n, 0), self.idx(r, 0));
            self.cells.copy_within(src..src + self.cols, dst);
            self.wrapped[r] = self.wrapped[r + n];
        }
        for r in bot + 1 - n..=bot {
            self.wrapped[r] = false;
            let base = self.idx(r, 0);
            for c in 0..self.cols {
                self.cells[base + c] = Cell::default();
            }
        }
    }

    /// The mirror of [`Grid::shift_up`], for `RI`, `SD` and `IL`.
    fn shift_down(&mut self, top: usize, bot: usize, n: usize) {
        if top > bot || n == 0 { return; }
        let n = n.min(bot - top + 1);
        for r in (top + n..=bot).rev() {
            let (src, dst) = (self.idx(r - n, 0), self.idx(r, 0));
            self.cells.copy_within(src..src + self.cols, dst);
            self.wrapped[r] = self.wrapped[r - n];
        }
        for r in top..top + n {
            self.wrapped[r] = false;
            let base = self.idx(r, 0);
            for c in 0..self.cols {
                self.cells[base + c] = Cell::default();
            }
        }
    }

    /// Scroll the region down `n` (content moves toward the bottom).
    pub fn scroll_region_down(&mut self, n: usize) {
        let (t, b) = (self.scroll_top, self.scroll_bot);
        self.shift_down(t, b, n);
    }

    /// Insert `n` blank lines AT THE CURSOR, pushing the rest of the region
    /// down and off its bottom. A no-op outside the region, which is what
    /// keeps a stray IL from a program that has moved above its own region
    /// from shifting the status line it was protecting.
    pub fn insert_lines(&mut self, n: usize) {
        if self.cur_row < self.scroll_top || self.cur_row > self.scroll_bot { return; }
        let (r, b) = (self.cur_row, self.scroll_bot);
        self.shift_down(r, b, n.max(1));
    }

    /// Delete `n` lines at the cursor, pulling the rest of the region up.
    pub fn delete_lines(&mut self, n: usize) {
        if self.cur_row < self.scroll_top || self.cur_row > self.scroll_bot { return; }
        let (r, b) = (self.cur_row, self.scroll_bot);
        self.shift_up(r, b, n.max(1));
    }

    pub fn carriage_return(&mut self) {
        self.cur_col = 0;
    }

    pub fn backspace(&mut self) {
        self.cur_col = self.cur_col.saturating_sub(1);
    }

    pub fn tab(&mut self) {
        let next = ((self.cur_col / 8) + 1) * 8;
        self.cur_col = next.min(self.cols - 1);
    }

    /// Scroll the SCROLLING REGION up one line.
    ///
    /// The row leaving the top enters scrollback only when the region is the
    /// whole screen. A program that reserved a status line is redrawing a
    /// window over its own content, not producing history; putting those rows
    /// in the scrollback would fill it with half-frames of vim and make the
    /// buffer useless for the thing it exists for.
    ///
    /// (xterm's own rule is narrower — it saves whenever the region's top is
    /// row 0 — which would also capture a bottom-anchored region. Requiring
    /// the FULL screen is the conservative reading: it never captures a frame
    /// that was not history, at the cost of missing scrollback for a program
    /// that reserves only a footer. Worth revisiting with a real `less`
    /// session in front of you.)
    pub fn scroll_up(&mut self) {
        // The alternate screen is a scratch surface, not history. A pager that
        // scrolls through a long file would otherwise push every frame of its
        // own rendering into the buffer and bury the shell session underneath
        // it — and the buffer exists to hold the shell session.
        let full = self.scroll_top == 0 && self.scroll_bot == self.rows - 1
            && self.alt_saved.is_none();
        if full {
            let top: Vec<Cell> = self.cells[0..self.cols].to_vec();
            let w = self.wrapped[0];
            self.sb_push(top, w);
            // pin the view while scrolled back: the bottom moved one line away, so keep pointing at
            // the same content (clamped — eviction above can shrink what's reachable)
            if self.view_offset > 0 {
                self.view_offset = (self.view_offset + 1).min(self.scrollback.len());
            }
        }
        let (t, b) = (self.scroll_top, self.scroll_bot);
        self.shift_up(t, b, 1);
    }

    /// Page the viewport up into scrollback by `n` lines (clamped to what exists).
    pub fn page_up(&mut self, n: usize) {
        self.view_offset = (self.view_offset + n).min(self.scrollback.len());
    }

    /// Page the viewport back down toward live by `n` lines.
    pub fn page_down(&mut self, n: usize) {
        self.view_offset = self.view_offset.saturating_sub(n);
    }

    /// Jump back to the live view.
    pub fn scroll_to_bottom(&mut self) {
        self.view_offset = 0;
    }

    pub fn is_scrolled_back(&self) -> bool {
        self.view_offset > 0
    }

    /// Text of a GLOBAL line: index over scrollback (oldest first) then the live rows — the
    /// same indexing [`Grid::all_lines`] and search matches use.
    pub fn line_text_global(&self, line: usize) -> String {
        let sb = self.scrollback.len();
        if line < sb {
            self.scrollback[line].iter().map(|c| c.ch).filter(|c| *c != WIDE_TAIL)
                .collect::<String>().trim_end().to_string()
        } else {
            self.row_text((line - sb).min(self.rows.saturating_sub(1)))
        }
    }

    /// The view offset that brings GLOBAL `line` into the viewport, about a third of the way
    /// down (0 when the line is a live row — the live view already shows it).
    pub fn offset_to_show_line(&self, line: usize) -> usize {
        let sb = self.scrollback.len();
        if line >= sb {
            return 0;
        }
        ((sb - line) + self.rows / 3).min(sb)
    }

    /// Which viewport row (if any) currently shows GLOBAL `line`, honoring the view offset.
    pub fn viewport_row_of_line(&self, line: usize) -> Option<usize> {
        let virt_top = self.scrollback.len() - self.view_offset;
        line.checked_sub(virt_top).filter(|r| *r < self.rows)
    }

    /// The GLOBAL line a viewport row currently shows (inverse of
    /// [`Grid::viewport_row_of_line`]).
    pub fn global_line_of_viewport_row(&self, r: usize) -> usize {
        self.scrollback.len() - self.view_offset + r
    }

    /// The cell at viewport position (r, c) honoring [`Grid::view_offset`]: a window over
    /// scrollback stitched onto the live rows. Scrollback rows captured before a resize may be
    /// narrower/wider than `cols`; out-of-range columns read as blank (default cell).
    pub fn display_cell(&self, r: usize, c: usize) -> Cell {
        if self.view_offset == 0 {
            return self.cell(r, c);
        }
        let sb = self.scrollback.len();
        let virt = sb - self.view_offset + r;
        if virt < sb {
            self.scrollback[virt].get(c).copied().unwrap_or_default()
        } else {
            self.cell(virt - sb, c)
        }
    }

    /// Clear the visible cells and home the cursor (scrollback is preserved).
    pub fn clear(&mut self) {
        for c in &mut self.cells {
            *c = Cell::default();
        }
        self.wrapped.iter_mut().for_each(|w| *w = false);
        self.cur_row = 0;
        self.cur_col = 0;
    }

    /// Absolute cursor move, 0-based, clamped to the grid.
    pub fn move_to(&mut self, r: usize, c: usize) {
        self.cur_row = r.min(self.rows - 1);
        self.cur_col = c.min(self.cols - 1);
    }

    /// Erase in line: 0 = cursor→end, 1 = start→cursor, 2 = whole line.
    pub fn erase_in_line(&mut self, mode: u16) {
        let (from, to) = match mode {
            1 => (0, self.cur_col + 1),
            2 => (0, self.cols),
            _ => (self.cur_col, self.cols),
        };
        let base = self.idx(self.cur_row, 0);
        for c in from..to.min(self.cols) {
            self.cells[base + c] = Cell { ch: ' ', style: self.pen };
        }
        // The row's END was erased (modes 0 and 2): it no longer runs on into
        // the next row. Mode 1 erases the start and leaves the end alone.
        if mode != 1 {
            self.wrapped[self.cur_row] = false;
        }
    }

    /// Erase `n` characters from the cursor (ECH, `CSI n X`): blank them in
    /// place, shifting nothing and moving the cursor nowhere.
    ///
    /// Distinct from [`Grid::erase_in_line`], which always runs to the end of
    /// the row. A line editor uses ECH to clear exactly the width of the text
    /// it is about to overwrite, and treating it as "erase to end" throws away
    /// the tail the editor was relying on keeping.
    pub fn erase_chars(&mut self, n: usize) {
        let base = self.idx(self.cur_row, 0);
        let start = self.cur_col.min(self.cols);
        let end = (start + n.max(1)).min(self.cols);
        for c in start..end {
            self.cells[base + c] = Cell { ch: ' ', style: self.pen };
        }
    }

    /// Insert `n` blanks at the cursor (ICH, `CSI n @`), pushing the rest of
    /// the row right. Characters pushed past the last column are lost — the
    /// row does not wrap into the next one, which is what makes this cheap
    /// enough for an editor to use on every keystroke.
    pub fn insert_chars(&mut self, n: usize) {
        let base = self.idx(self.cur_row, 0);
        let start = self.cur_col.min(self.cols);
        if start >= self.cols {
            return;
        }
        let n = n.max(1).min(self.cols - start);
        for c in (start + n..self.cols).rev() {
            self.cells[base + c] = self.cells[base + c - n];
        }
        for c in start..(start + n).min(self.cols) {
            self.cells[base + c] = Cell { ch: ' ', style: self.pen };
        }
    }

    /// Delete `n` characters at the cursor (DCH, `CSI n P`), pulling the rest
    /// of the row left and blanking the `n` columns freed at the end.
    pub fn delete_chars(&mut self, n: usize) {
        let base = self.idx(self.cur_row, 0);
        let start = self.cur_col.min(self.cols);
        if start >= self.cols {
            return;
        }
        let n = n.max(1).min(self.cols - start);
        for c in start..self.cols - n {
            self.cells[base + c] = self.cells[base + c + n];
        }
        for c in self.cols - n..self.cols {
            self.cells[base + c] = Cell { ch: ' ', style: self.pen };
        }
    }

    /// Erase in display: 0 = cursor→end, 1 = start→cursor, 2 = whole screen.
    pub fn erase_in_display(&mut self, mode: u16) {
        let (cr, rows) = (self.cur_row, self.rows);
        match mode {
            2 => self.wrapped.iter_mut().for_each(|w| *w = false),
            1 => self.wrapped[..cr].iter_mut().for_each(|w| *w = false),
            _ => self.wrapped[cr..rows].iter_mut().for_each(|w| *w = false),
        }
        match mode {
            2 => {
                for c in &mut self.cells {
                    *c = Cell { ch: ' ', style: self.pen };
                }
            }
            1 => {
                let end = self.idx(self.cur_row, self.cur_col);
                let n = self.cells.len();
                let last = end.min(n.saturating_sub(1));
                for c in &mut self.cells[..=last] {
                    *c = Cell { ch: ' ', style: self.pen };
                }
            }
            _ => {
                let start = self.idx(self.cur_row, self.cur_col);
                for c in &mut self.cells[start..] {
                    *c = Cell { ch: ' ', style: self.pen };
                }
            }
        }
    }

    /// Push a row into scrollback with its wrap flag, evicting the oldest past
    /// the cap. The one place scrollback grows, so the flags stay parallel.
    fn sb_push(&mut self, row: Vec<Cell>, wrapped: bool) {
        self.scrollback.push(row);
        self.scrollback_wrapped.push(wrapped);
        while self.scrollback.len() > self.scrollback_cap {
            self.scrollback.remove(0);
            if !self.scrollback_wrapped.is_empty() {
                self.scrollback_wrapped.remove(0);
            }
        }
    }

    /// Resize the grid, REFLOWING the primary screen and its scrollback.
    ///
    /// Soft-wrapped lines (see `wrapped`) are joined and rewrapped at the new
    /// width: narrowing wraps them further, and widening UNWRAPS them again.
    /// This replaces a resize that could only truncate -- growing a pane back
    /// after shrinking it never restored the text -- and a workaround that
    /// pushed every truncated row onto the END of scrollback while its narrow
    /// copy stayed on screen, leaving out-of-order duplicates in the history.
    ///
    /// The primary screen is reflowed wherever it is: live, or PARKED while a
    /// full-screen program has the alternate screen (so the shell is intact
    /// when it exits). The alternate screen itself is only resized -- its
    /// program redraws it on SIGWINCH, as every terminal assumes.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let (old_cols, old_rows) = (self.cols, self.rows);
        // Scrollback flags are kept parallel; a mismatch (which nothing should
        // produce) is repaired rather than trusted, treating extras as hard ends.
        self.scrollback_wrapped.resize(self.scrollback.len(), false);
        let sb = std::mem::take(&mut self.scrollback);
        let sbw = std::mem::take(&mut self.scrollback_wrapped);

        if let Some(saved) = self.alt_saved.take() {
            let r = reflow_primary(sb, sbw, &saved.cells, &saved.wrapped, old_cols,
                                   (saved.cur_row, saved.cur_col), cols, rows, self.scrollback_cap);
            self.scrollback = r.scrollback;
            self.scrollback_wrapped = r.scrollback_wrapped;
            self.alt_saved = Some(AltSave {
                cells: r.cells,
                wrapped: r.wrapped,
                cur_row: r.cursor.0,
                cur_col: r.cursor.1.min(cols - 1),
                pen: saved.pen,
                scroll_top: 0,
                scroll_bot: rows - 1,
            });
            // The alternate screen: resized, not reflowed.
            let mut next = vec![Cell::default(); cols * rows];
            for r in 0..rows.min(old_rows) {
                for c in 0..cols.min(old_cols) {
                    next[r * cols + c] = self.cells[r * old_cols + c];
                }
            }
            self.cells = next;
            self.wrapped = vec![false; rows];
            self.cur_row = self.cur_row.min(rows - 1);
            self.cur_col = self.cur_col.min(cols - 1);
        } else {
            let r = reflow_primary(sb, sbw, &self.cells, &self.wrapped, old_cols,
                                   (self.cur_row, self.cur_col), cols, rows, self.scrollback_cap);
            self.scrollback = r.scrollback;
            self.scrollback_wrapped = r.scrollback_wrapped;
            self.cells = r.cells;
            self.wrapped = r.wrapped;
            self.cur_row = r.cursor.0;
            // `cols` itself is the pending-wrap position `put` already uses
            // after writing the last column; clamping to `cols - 1` would make
            // the next character overwrite the last one.
            self.cur_col = r.cursor.1.min(cols);
        }
        self.cols = cols;
        self.rows = rows;
        // A region that outlived the rows it named would make `line_feed`
        // compare the cursor against a row that no longer exists, and the
        // screen would stop scrolling entirely. A resize is also when a shell
        // re-sends DECSTBM, so resetting to the full screen is both safe and
        // what the next escape will correct.
        self.scroll_top = 0;
        self.scroll_bot = rows - 1;
        // A DECSC position from the old geometry must not restore off-screen.
        if let Some((r, c, pen)) = self.saved_cursor {
            self.saved_cursor = Some((r.min(rows - 1), c.min(cols - 1), pen));
        }
        // The content under a scrolled-back view has moved; show the live end.
        self.view_offset = 0;
    }
}

/// A cell that is empty in every visible way: trimmed from line ends.
fn is_blank_cell(c: &Cell) -> bool {
    c.ch == ' ' && c.style == Style::default()
}

/// The result of reflowing a primary screen with its scrollback.
struct Reflowed {
    scrollback: Vec<Vec<Cell>>,
    scrollback_wrapped: Vec<bool>,
    cells: Vec<Cell>,
    wrapped: Vec<bool>,
    cursor: (usize, usize),
}

/// Rewrap `src` rows (oldest first, each with its soft-wrap flag) to `cols`.
/// `cursor` is a row index into `src` and a column. Returns the new rows and
/// where the cursor landed -- its column may be `cols`, the pending-wrap spot.
///
/// Logical lines are rows joined through their wrap flags. Each is trimmed of
/// trailing blank cells (never past the cursor, which may sit in them) and
/// split at `cols`, never splitting a wide character: one that would straddle
/// the margin moves whole to the next row. An empty line is one empty row.
fn reflow_rows(src: Vec<(Vec<Cell>, bool)>, cursor: (usize, usize), cols: usize)
    -> (Vec<(Vec<Cell>, bool)>, (usize, usize))
{
    let cols = cols.max(1);
    let mut out: Vec<(Vec<Cell>, bool)> = Vec::new();
    let mut cur = (0usize, 0usize);
    let mut line: Vec<Cell> = Vec::new();
    let mut cursor_off: Option<usize> = None;
    let n = src.len();
    // A wide character that did not fit at a row's end was pushed to the next
    // row, leaving ONE blank pad cell behind. Joined back verbatim, that pad
    // became a stray space on every widen ("ab 中 文cd"). Recognised as: a
    // wrapped row ending in one blank cell, followed by a row that STARTS with
    // a wide character. Trade-off: a genuine space in exactly that spot would
    // also go; a true marker (as Alacritty keeps) would need every renderer
    // to learn a new cell kind.
    let starts_wide = |r: &Vec<Cell>| r.len() >= 2 && r[1].ch == WIDE_TAIL;
    let pads: Vec<bool> = (0..n).map(|i| {
        src[i].1 && i + 1 < n && starts_wide(&src[i + 1].0)
            && src[i].0.last().is_some_and(is_blank_cell)
    }).collect();
    for (i, (mut row, wrapped)) in src.into_iter().enumerate() {
        if pads[i] {
            row.pop();
        }
        if i == cursor.0 {
            cursor_off = Some(line.len() + cursor.1.min(row.len()));
        }
        line.extend(row);
        if wrapped && i + 1 < n {
            continue;
        }
        let keep = cursor_off.unwrap_or(0);
        while line.len() > keep && line.last().is_some_and(is_blank_cell) {
            line.pop();
        }
        let mut row: Vec<Cell> = Vec::with_capacity(cols);
        let mut k = 0;
        while k < line.len() {
            let wide = k + 1 < line.len() && line[k + 1].ch == WIDE_TAIL && cols >= 2;
            let w = if wide { 2 } else { 1 };
            if row.len() + w > cols && !row.is_empty() {
                row.resize(cols, Cell::default());
                out.push((std::mem::replace(&mut row, Vec::with_capacity(cols)), true));
            }
            if cursor_off == Some(k) {
                cur = (out.len(), row.len());
            }
            row.push(line[k]);
            if wide {
                if cursor_off == Some(k + 1) {
                    cur = (out.len(), row.len());
                }
                row.push(line[k + 1]);
            }
            // A wide head with no room for its tail (cols < 2) drops the tail.
            k += if line.get(k + 1).is_some_and(|c| c.ch == WIDE_TAIL) && !wide { 2 } else { w };
        }
        if cursor_off == Some(line.len()) {
            cur = (out.len(), row.len());
        }
        row.resize(cols, Cell::default());
        out.push((row, false));
        line.clear();
        cursor_off = None;
    }
    (out, cur)
}

/// Reflow a primary screen (`cells`, `old_cols` wide) together with its
/// scrollback into a `cols` x `rows` screen, the surplus going to scrollback.
///
/// Blank rows BELOW the cursor are free space, not lines: they are dropped
/// before reflowing and the screen is padded again after, so a fresh
/// terminal's few lines stay at the top instead of sinking to the bottom.
/// The screen then shows the last `rows` rows, cursor always visible.
fn reflow_primary(sb: Vec<Vec<Cell>>, sbw: Vec<bool>, cells: &[Cell], wrapped: &[bool], old_cols: usize,
                  cursor: (usize, usize), cols: usize, rows: usize, cap: usize) -> Reflowed {
    let old_rows = if old_cols == 0 { 0 } else { cells.len() / old_cols };
    let sb_len = sb.len();
    let mut src: Vec<(Vec<Cell>, bool)> = sb.into_iter().zip(sbw).collect();
    let mut screen: Vec<(Vec<Cell>, bool)> = (0..old_rows)
        .map(|r| (cells[r * old_cols..(r + 1) * old_cols].to_vec(), wrapped.get(r).copied().unwrap_or(false)))
        .collect();
    let cur_row = cursor.0.min(old_rows.saturating_sub(1));
    while screen.len() > cur_row + 1
        && screen.last().is_some_and(|(r, w)| !w && r.iter().all(is_blank_cell))
    {
        screen.pop();
    }
    src.extend(screen);
    let (all, cur) = reflow_rows(src, (sb_len + cur_row, cursor.1), cols);
    let total = all.len();
    let mut start = total.saturating_sub(rows);
    if cur.0 < start {
        start = cur.0;
    }
    let mut out = Reflowed {
        scrollback: Vec::new(),
        scrollback_wrapped: Vec::new(),
        cells: Vec::with_capacity(cols * rows),
        wrapped: Vec::with_capacity(rows),
        cursor: (cur.0 - start, cur.1),
    };
    for (i, (row, w)) in all.into_iter().enumerate() {
        if i < start {
            out.scrollback.push(row);
            out.scrollback_wrapped.push(w);
        } else if i < start + rows {
            out.cells.extend(row);
            out.wrapped.push(w);
        }
    }
    let excess = out.scrollback.len().saturating_sub(cap);
    out.scrollback.drain(..excess);
    out.scrollback_wrapped.drain(..excess);
    out.cells.resize(cols * rows, Cell::default());
    out.wrapped.resize(rows, false);
    out
}

// ── the VT/ANSI parser ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Csi,
    /// Inside `ESC ]` (Operating System Command) — consuming and discarding
    /// bytes until the terminator. Sequences in this family (window/tab
    /// title changes, and the "semantic prompt" markers modern shells and
    /// frameworks emit — OSC 133 for prompt/command boundaries, OSC 633 for
    /// VS Code's shell integration, OSC 7 for the working-directory URL,
    /// OSC 2/0 for window titles) carry no information this grid renders,
    /// but before this state existed they were actively harmful: with only
    /// `Esc`/`Csi` recognised, `ESC ]` fell through `Esc`'s catch-all back to
    /// `Ground` and every byte of the OSC payload — semicolons, digits,
    /// key=value pairs — got typed onto the screen as if it were normal
    /// output. A bare interactive shell that sets a window title (or any
    /// shell with prompt-integration hooks) reliably produced exactly this:
    /// stray characters like a lone `=` appearing right after the prompt,
    /// with no correlation to anything the test actually ran.
    /// After `ESC` and an INTERMEDIATE byte (`(`, `)`, `*`, `+`, `#`, `%`) —
    /// one more character completes the sequence and is consumed.
    ///
    /// Without this state, `ESC ( B` (designate ASCII into G0, which ncurses
    /// emits on almost every screen setup) fell through `Esc`'s catch-all back
    /// to `Ground` and the `B` was PRINTED. A stray letter on screen from a
    /// sequence that carries no visible information at all — the worst trade
    /// available, since not implementing it would have been harmless.
    EscIntermediate(char),
    Osc,
    /// Inside `ESC ]`, having just seen an `ESC` — one more char decides
    /// whether this is the String Terminator (`ESC \`, ending the OSC) or
    /// just a stray ESC inside the payload (rare, but BEL is by far the
    /// more common terminator in practice; treated as "back into the OSC
    /// payload" rather than as the start of a new escape, since a shell
    /// that meant to start a fresh escape wouldn't do it mid-OSC).
    OscEsc,
}

/// A byte/char-stream VT parser that drives a [`Grid`]. Feed it decoded text (UTF-8); it
/// applies printable characters, C0 controls, and CSI sequences.
pub struct Parser {
    state: State,
    params: Vec<u16>,
    /// True when the current CSI began `CSI ?` — a DEC PRIVATE sequence.
    private: bool,
    /// The `>` / `=` markers of the current CSI. Tracked because DA1 (`CSI c`),
    /// DA2 (`CSI > c`) and DA3 (`CSI = c`) share a final byte and differ ONLY
    /// here -- dropping them as generic intermediates answered DA2 with DA1.
    gt: bool,
    eq: bool,
    cur: Option<u16>,
    /// Bytes the TERMINAL must send back to the program: answers to queries.
    ///
    /// ## Why this exists: the helix start-up delay
    ///
    /// Reported as "the hx command has a delay in the egui version, fixed in
    /// the Qt predecessor". It was fixed there in `pty_term.rs` v0.1.784, and
    /// that comment is the diagnosis: crossterm's
    /// `supports_keyboard_enhancement()`, called at start-up by helix, gitui
    /// and other ratatui/crossterm TUIs, sends `CSI ? u` then `CSI c` and BLOCKS
    /// waiting for the Device Attributes reply as a sync marker. With no reply
    /// it stalls ~2-3 s before giving up.
    ///
    /// This parser had no outbound channel at all, so it could not have
    /// answered even if it had recognised the query. The host drains this with
    /// [`Parser::take_replies`] and writes it to the PTY master.
    replies: Vec<u8>,
}

impl Default for Parser {
    fn default() -> Self {
        Parser {
            state: State::Ground, params: Vec::new(), cur: None, private: false,
            gt: false, eq: false, replies: Vec::new(),
        }
    }
}

impl Parser {
    /// Bytes to write back to the PTY: answers to terminal queries, in order.
    ///
    /// Must be drained and written after every feed. A reply that is queued
    /// and never sent is exactly as slow as no reply -- the program is still
    /// waiting on the other end of the PTY.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }
}

impl Parser {
    pub fn new() -> Parser {
        Parser::default()
    }

    /// Feed decoded text; mutates `grid`.
    pub fn feed(&mut self, text: &str, grid: &mut Grid) {
        for ch in text.chars() {
            self.step(ch, grid);
        }
    }

    fn take_params(&mut self) -> Vec<u16> {
        if let Some(v) = self.cur.take() {
            self.params.push(v);
        }
        std::mem::take(&mut self.params)
    }

    fn param_or(params: &[u16], i: usize, default: u16) -> u16 {
        params.get(i).copied().filter(|&v| v != 0).unwrap_or(default)
    }

    fn step(&mut self, ch: char, grid: &mut Grid) {
        match self.state {
            State::Ground => match ch {
                '\x1b' => self.state = State::Esc,
                '\n' => grid.line_feed(),
                '\r' => grid.carriage_return(),
                '\t' => grid.tab(),
                '\x08' => grid.backspace(),
                c if (c as u32) >= 0x20 => grid.put(c),
                _ => {} // ignore other C0 controls for now
            },
            State::Esc => match ch {
                '[' => {
                    self.params.clear();
                    self.private = false;
                    self.gt = false;
                    self.eq = false;
                    self.cur = None;
                    self.state = State::Csi;
                }
                ']' => self.state = State::Osc,
                // RI — reverse index. Not a CSI: a bare `ESC M`.
                'M' => { grid.reverse_index(); self.state = State::Ground; }
                // DECSC / DECRC.
                '7' => { grid.save_cursor(); self.state = State::Ground; }
                '8' => { grid.restore_cursor(); self.state = State::Ground; }
                // Charset designators and the DEC private `#` family: the
                // NEXT byte belongs to the sequence and must not be printed.
                '(' | ')' | '*' | '+' | '#' | '%' | ' ' => {
                    self.state = State::EscIntermediate(ch);
                }
                _ => self.state = State::Ground, // ignore non-CSI escapes for v1
            },
            State::EscIntermediate(kind) => {
                // Only `ESC # 8` does anything this grid can show; every other
                // designator is consumed and discarded, which is the whole
                // point of the state.
                if kind == '#' && ch == '8' {
                    grid.screen_alignment_test();
                }
                self.state = State::Ground;
            }
            State::Osc => match ch {
                '\x07' => self.state = State::Ground, // BEL terminator
                '\x1b' => self.state = State::OscEsc,
                _ => {} // discard the OSC payload
            },
            State::OscEsc => match ch {
                '\\' => self.state = State::Ground, // ST (ESC \) terminator
                // Not actually a terminator — back into the payload. The ESC
                // itself is discarded same as any other OSC byte; if it was
                // meant to start something new, that's indistinguishable
                // from OSC-internal noise without a lookahead this parser
                // doesn't do, and BEL-terminated OSC (the common case) never
                // reaches this arm at all.
                _ => self.state = State::Osc,
            },
            State::Csi => {
                match ch {
                    '0'..='9' => {
                        let d = ch as u16 - '0' as u16;
                        self.cur = Some(self.cur.unwrap_or(0) * 10 + d);
                    }
                    ';' => {
                        self.params.push(self.cur.take().unwrap_or(0));
                    }
                    '?' => { self.private = true; }
                    '>' => { self.gt = true; }
                    '=' => { self.eq = true; }
                    '@'..='~' => {
                        let params = self.take_params();
                        if self.private {
                            self.dispatch_private(ch, &params, grid);
                        } else {
                            self.dispatch_csi(ch, &params, grid);
                        }
                        self.state = State::Ground;
                    }
                    _ => { /* other intermediate bytes ignored */ }
                }
            }
        }
    }

    /// `CSI ? Pm h` / `CSI ? Pm l` — DEC private mode set/reset.
    ///
    /// Only the modes this renderer can honour are acted on. An unrecognised
    /// one is IGNORED rather than falling through to `dispatch_csi`, which is
    /// what used to happen: `CSI ? 25 l` (hide cursor) was arriving as
    /// `CSI 25 l` and being interpreted as an ANSI mode reset. Silently doing
    /// the wrong thing is worse than doing nothing, because nothing is
    /// debuggable.
    fn dispatch_private(&mut self, final_byte: char, params: &[u16], grid: &mut Grid) {
        let set = match final_byte {
            'h' => true,
            'l' => false,
            // Other private finals (DECRQM and friends) are not answered.
            _ => return,
        };
        for p in params {
            match p {
                // DECCKM — application cursor keys. fzf, vim and less set it,
                // and the key encoder sends SS3 rather than CSI while it holds.
                1 => grid.app_cursor = set,
                // DEC 2004 — bracketed paste.
                2004 => grid.bracketed_paste = set,
                // The alternate screen. Three spellings with slightly
                // different cursor behaviour, all handled here rather than in
                // three places: `?1049` saves and restores the cursor around
                // the switch, `?47` and `?1047` switch the buffer only.
                1049 => if set { grid.enter_alt_screen(true) } else { grid.leave_alt_screen(true) },
                47 | 1047 => if set { grid.enter_alt_screen(false) } else { grid.leave_alt_screen(false) },
                // `?1048` is the cursor half on its own — some programs pair it
                // with `?1047` to get `?1049`'s behaviour in two escapes.
                1048 => if set { grid.save_cursor() } else { grid.restore_cursor() },
                _ => {}
            }
        }
    }

    fn dispatch_csi(&mut self, final_byte: char, params: &[u16], grid: &mut Grid) {
        match final_byte {
            'A' => {
                let n = Self::param_or(params, 0, 1) as usize;
                grid.move_to(grid.cur_row.saturating_sub(n), grid.cur_col);
            }
            'B' => {
                let n = Self::param_or(params, 0, 1) as usize;
                grid.move_to(grid.cur_row + n, grid.cur_col);
            }
            'C' => {
                let n = Self::param_or(params, 0, 1) as usize;
                grid.move_to(grid.cur_row, grid.cur_col + n);
            }
            'D' => {
                let n = Self::param_or(params, 0, 1) as usize;
                grid.move_to(grid.cur_row, grid.cur_col.saturating_sub(n));
            }
            'H' | 'f' => {
                let r = Self::param_or(params, 0, 1).saturating_sub(1) as usize;
                let c = Self::param_or(params, 1, 1).saturating_sub(1) as usize;
                grid.move_to(r, c);
            }
            // CNL / CPL — down/up n lines AND to column 0. Not the same as
            // 'B'/'A', which preserve the column.
            'E' => {
                let n = Self::param_or(params, 0, 1) as usize;
                grid.move_to(grid.cur_row + n, 0);
            }
            'F' => {
                let n = Self::param_or(params, 0, 1) as usize;
                grid.move_to(grid.cur_row.saturating_sub(n), 0);
            }
            // CHA / HPA — absolute column, 1-based. This is how readline
            // redraws an edited line: it jumps to a column rather than
            // counting backspaces. Dropping it was why a themed prompt's
            // redraw interleaved with itself (C13_TODO §19).
            'G' | '`' => {
                let c = Self::param_or(params, 0, 1).saturating_sub(1) as usize;
                grid.move_to(grid.cur_row, c);
            }
            // VPA — absolute row, 1-based, column preserved.
            'd' => {
                let r = Self::param_or(params, 0, 1).saturating_sub(1) as usize;
                grid.move_to(r, grid.cur_col);
            }
            '@' => grid.insert_chars(Self::param_or(params, 0, 1) as usize),
            'P' => grid.delete_chars(Self::param_or(params, 0, 1) as usize),
            'X' => grid.erase_chars(Self::param_or(params, 0, 1) as usize),
            'L' => grid.insert_lines(Self::param_or(params, 0, 1) as usize),
            'M' => grid.delete_lines(Self::param_or(params, 0, 1) as usize),
            // SU / SD scroll the region itself, cursor unmoved — distinct from
            // IL/DL, which anchor at the cursor.
            'S' => {
                let n = Self::param_or(params, 0, 1) as usize;
                for _ in 0..n.max(1) { grid.scroll_up(); }
            }
            'T' => grid.scroll_region_down(Self::param_or(params, 0, 1) as usize),
            // SCOSC / SCORC — the ANSI.SYS spelling of save/restore. Same
            // meaning as ESC 7 / ESC 8 and a different byte, so a program that
            // uses one and not the other still works.
            's' => grid.save_cursor(),
            'u' => grid.restore_cursor(),
            'r' => grid.set_scroll_region(
                Self::param_or(params, 0, 1), params.get(1).copied().unwrap_or(0)),
            'J' => grid.erase_in_display(params.first().copied().unwrap_or(0)),
            'K' => grid.erase_in_line(params.first().copied().unwrap_or(0)),
            'm' => self.apply_sgr(params, grid),
            // DEVICE ATTRIBUTES. Answered exactly as Qt's `pty_term.rs` does:
            //   DA1 `CSI c` / `CSI 0 c` -> `ESC [ ? 1 ; 2 c` (VT100 + advanced
            //       video). The content is irrelevant to crossterm, which only
            //       needs SOME reply as a sync marker; kept conservative so
            //       programs do not assume features this terminal lacks.
            //   DA2 `CSI > c`           -> `ESC [ > 0 ; 276 ; 0 c`
            //   DA3 `CSI = c`           -> left unanswered, as in Qt (rare, no
            //       known blocker).
            // NOT answering `CSI ? u` is deliberate and correct: silence there
            // means "no kitty keyboard protocol", which is true.
            'c' => {
                if self.gt {
                    self.replies.extend_from_slice(b"\x1b[>0;276;0c");
                } else if !self.eq && params.first().copied().unwrap_or(0) == 0 {
                    self.replies.extend_from_slice(b"\x1b[?1;2c");
                }
            }
            // DEVICE STATUS REPORT. Tools that ask where the cursor is block
            // for the answer too (Qt's v0.1.418 names ollama's spinner).
            //   DSR 5 -> `ESC [ 0 n` (status OK)
            //   DSR 6 -> `ESC [ row ; col R`, 1-based, as the query expects.
            'n' => match params.first().copied().unwrap_or(0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let r = format!("\x1b[{};{}R", grid.cur_row + 1, grid.cur_col + 1);
                    self.replies.extend_from_slice(r.as_bytes());
                }
                _ => {}
            },
            _ => {} // unsupported CSI: ignore
        }
    }

    /// SGR, including the extended `38`/`48` forms.
    ///
    /// The extended forms CONSUME their sub-parameters, and that is the whole
    /// point. Before this they did not: `38;5;33` fell through to the plain
    /// table, where `38` matched nothing, `5` matched nothing and `33` landed
    /// in the `30..=37` arm as foreground 3 — a wrong colour, silently. Worse,
    /// `38;2;255;128;0` ended on a `0`, which is SGR RESET, so a truecolour
    /// escape cleared every attribute the pen was carrying. Modern prompts
    /// emit these constantly, so "themed prompt loses all its styling" was one
    /// missing loop.
    fn apply_sgr(&self, params: &[u16], grid: &mut Grid) {
        let ps = if params.is_empty() { &[0u16][..] } else { params };
        let mut i = 0usize;
        while i < ps.len() {
            let p = ps[i];
            // `38`/`48` take a form selector and then its arguments. An
            // incomplete run at the end of the sequence is DROPPED rather than
            // partially applied: a truncated escape is not evidence about what
            // colour was wanted, and guessing is how the reset bug behaved.
            if matches!(p, 38 | 48) {
                let taken = match ps.get(i + 1) {
                    Some(5) => match ps.get(i + 2) {
                        Some(&n) => {
                            let c = TermColor::Ansi(n.min(255) as u8);
                            if p == 38 { grid.pen.fg = c } else { grid.pen.bg = c }
                            3
                        }
                        None => ps.len() - i,
                    },
                    Some(2) => match (ps.get(i + 2), ps.get(i + 3), ps.get(i + 4)) {
                        (Some(&r), Some(&g), Some(&b)) => {
                            let c = TermColor::Rgb(
                                r.min(255) as u8, g.min(255) as u8, b.min(255) as u8);
                            if p == 38 { grid.pen.fg = c } else { grid.pen.bg = c }
                            5
                        }
                        _ => ps.len() - i,
                    },
                    // An unknown selector (0/1/3/4 exist in the ISO form and
                    // nothing here can honour them). Skip the selector too, so
                    // its arguments are not read as colour codes.
                    Some(_) => 2,
                    None => 1,
                };
                i += taken;
                continue;
            }
            i += 1;
            match p {
                0 => grid.pen = Style::default(),
                1 => grid.pen.bold = true,
                22 => grid.pen.bold = false,
                7 => grid.pen.inverse = true,
                27 => grid.pen.inverse = false,
                30..=37 => grid.pen.fg = TermColor::Ansi((p - 30) as u8),
                39 => grid.pen.fg = TermColor::Default,
                40..=47 => grid.pen.bg = TermColor::Ansi((p - 40) as u8),
                49 => grid.pen.bg = TermColor::Default,
                90..=97 => grid.pen.fg = TermColor::Ansi((p - 90 + 8) as u8),
                100..=107 => grid.pen.bg = TermColor::Ansi((p - 100 + 8) as u8),
                _ => {}
            }
        }
    }
}

// ── terminal (grid + parser + input line) ────────────────────────────────────

/// A terminal: a grid, a parser, and the input line (where staged commands land).
pub struct Terminal {
    pub grid: Grid,
    parser: Parser,
    /// The current input line the user is editing (prefilled by [`stage`]).
    pub input: String,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Terminal {
        Terminal { grid: Grid::new(cols, rows), parser: Parser::new(), input: String::new() }
    }

    /// Feed output bytes from the PTY (decoded lossily to UTF-8).
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        self.parser.feed(&text, &mut self.grid);
    }

    /// Feed already-decoded text.
    pub fn feed(&mut self, text: &str) {
        self.parser.feed(text, &mut self.grid);
    }

    /// Answers to terminal queries that must be written back to the PTY.
    ///
    /// CALL AFTER EVERY FEED AND WRITE THE RESULT TO THE PTY MASTER. See
    /// `Parser::replies` for why: helix and other crossterm programs block on
    /// start-up waiting for these, and a reply that is queued but never sent
    /// is exactly as slow as none.
    pub fn take_replies(&mut self) -> Vec<u8> {
        self.parser.take_replies()
    }

    /// Place a command on the input line **without running it** — the cross-app action
    /// `stage_command` (e.g. gitmind saying "you need to run this"). The user reviews it and
    /// presses enter.
    pub fn stage(&mut self, command: &str) {
        self.input = command.to_string();
    }

    /// Clear the visible screen (cells + cursor home); scrollback is kept.
    pub fn clear(&mut self) {
        self.grid.clear();
    }

    /// The visible screen as text — one line per row, trailing blank lines trimmed. Used to put the
    /// screen on the clipboard (the compass "copy" action).
    pub fn screen_text(&self) -> String {
        let mut lines: Vec<String> = (0..self.grid.rows).map(|r| self.grid.row_text(r)).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Clear the input line — the ^C / interrupt gesture.
    pub fn interrupt(&mut self) {
        self.input.clear();
    }

    /// Append pasted text to the input line (Ctrl+V / paste). Control characters are dropped so a
    /// multi-line paste doesn't smuggle newlines onto a single input line.
    pub fn paste(&mut self, text: &str) {
        for ch in text.chars() {
            if !ch.is_control() {
                self.input.push(ch);
            }
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }
}

// ── egui cell renderer + colour resolution ───────────────────────────────────

/// The standard 16-colour ANSI palette (0–7 normal, 8–15 bright), as egui colours.
pub fn ansi_color(idx: u8) -> egui::Color32 {
    use egui::Color32 as C;
    match idx {
        0 => C::from_rgb(0x1c, 0x1c, 0x1c),
        1 => C::from_rgb(0xcc, 0x33, 0x33),
        2 => C::from_rgb(0x33, 0xaa, 0x33),
        3 => C::from_rgb(0xcc, 0xaa, 0x33),
        4 => C::from_rgb(0x33, 0x66, 0xcc),
        5 => C::from_rgb(0xaa, 0x33, 0xaa),
        6 => C::from_rgb(0x33, 0xaa, 0xaa),
        7 => C::from_rgb(0xcc, 0xcc, 0xcc),
        8 => C::from_rgb(0x55, 0x55, 0x55),
        9 => C::from_rgb(0xff, 0x55, 0x55),
        10 => C::from_rgb(0x55, 0xff, 0x55),
        11 => C::from_rgb(0xff, 0xff, 0x55),
        12 => C::from_rgb(0x55, 0x99, 0xff),
        13 => C::from_rgb(0xff, 0x55, 0xff),
        14 => C::from_rgb(0x55, 0xff, 0xff),
        15 => C::from_rgb(0xff, 0xff, 0xff),
        // The 6x6x6 cube. The level table is not linear — xterm's steps are
        // 0, 95, then 40 apart — and interpolating instead would make every
        // 256-colour prompt subtly wrong in a way nobody can point at.
        16..=231 => {
            const L: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let i = (idx - 16) as usize;
            C::from_rgb(L[i / 36], L[(i / 6) % 6], L[i % 6])
        }
        // The 24-step greyscale ramp, 8..238 by 10.
        _ => {
            let v = 8u16 + 10 * (idx as u16 - 232);
            let v = v.min(255) as u8;
            C::from_rgb(v, v, v)
        }
    }
}

/// Resolve a cell's `(foreground, background)` egui colours, honouring inverse video and the
/// theme default (default fg = the palette text colour, default bg = the base background).
pub fn resolve_colors(style: Style, colors: &uikit_egui::PaletteColors) -> (egui::Color32, egui::Color32) {
    let fg = match style.fg {
        TermColor::Default => colors.text,
        TermColor::Ansi(i) => ansi_color(i),
        TermColor::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
    };
    let bg = match style.bg {
        TermColor::Default => colors.bg_base,
        TermColor::Ansi(i) => ansi_color(i),
        TermColor::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
    };
    if style.inverse {
        (bg, fg)
    } else {
        (fg, bg)
    }
}

/// Render the terminal grid into `ui` as a monospace cell grid. Efficient: per row, cells are
/// grouped into runs of identical style and drawn as one text galley (not one per cell).
/// Colours route through `filter_color` so the display-accessibility settings apply live.
/// The terminal cursor's interaction mode — ported 1:1 from voiceterm's QML `termCursor`
/// (main.qml): mode 0 hides the cursor entirely (an inner TUI like vim draws its own, so ours
/// on top would double-image); mode 1 holds a flat 0.7 opacity; mode 2 pulses; mode 3 sits at a
/// static 0.85 opacity but slides smoothly between cells. The QML defaults to 3 (slide) as the
/// primary experience — 1/2 exist for accessibility/reduced-motion preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorMode {
    Off,
    Static,
    Pulse,
    #[default]
    Slide,
}

/// Cursor animation state: the QML's `Behavior on x/y { NumberAnimation { duration: 80;
/// easing.type: Easing.OutCubic } }` (mode 3, slide) plus the mode-2 pulse (`SequentialAnimation`,
/// 0.3↔0.7 opacity, 600ms each leg, `Easing.InOutQuad`). Kept as a small tick-driven struct
/// (matching the `PlaqueScroll` shape used for logscope's smooth scroll) rather than reaching for
/// egui's built-in `animate_value` so the exact QML durations/curves carry over precisely.
#[derive(Clone, Copy, Debug)]
pub struct CursorAnim {
    x: f32, y: f32,           // current rendered position (cells)
    from_x: f32, from_y: f32, // where this glide started
    target_x: f32, target_y: f32,
    elapsed: f32,             // seconds into the current slide
    pulse_t: f32,             // seconds into the current pulse leg
    pulse_rising: bool,
}

impl Default for CursorAnim {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, from_x: 0.0, from_y: 0.0, target_x: 0.0, target_y: 0.0, elapsed: 0.0, pulse_t: 0.0, pulse_rising: false }
    }
}

impl CursorAnim {
    const SLIDE_DURATION: f32 = 0.08; // 80ms, matching the QML exactly
    const PULSE_LEG: f32 = 0.6; // 600ms per leg, matching the QML exactly

    /// Set a new target cell (call every frame with the grid's current cursor position).
    pub fn set_target(&mut self, col: usize, row: usize) {
        let (tx, ty) = (col as f32, row as f32);
        if (tx - self.target_x).abs() < 1e-4 && (ty - self.target_y).abs() < 1e-4 {
            return; // already headed there (or already there) — don't restart the glide
        }
        self.from_x = self.x;
        self.from_y = self.y;
        self.target_x = tx;
        self.target_y = ty;
        self.elapsed = 0.0;
    }

    /// Advance both the slide and the pulse by `dt` seconds. Returns (x, y, opacity) in cell
    /// units / [0,1] for the caller to convert to pixels and paint.
    pub fn tick(&mut self, dt: f32, mode: CursorMode) -> (f32, f32, f32) {
        // slide (mode 3): eased position; other modes snap instantly (matches the QML's
        // `Behavior { enabled: cursorMode === 3 }` — the Behavior simply doesn't apply otherwise).
        if mode == CursorMode::Slide {
            if (self.target_x - self.x).abs() > 1e-4 || (self.target_y - self.y).abs() > 1e-4 {
                self.elapsed = (self.elapsed + dt.max(0.0)).min(Self::SLIDE_DURATION);
                let t = uikit_egui::motion::ease::out_cubic(self.elapsed / Self::SLIDE_DURATION);
                self.x = self.from_x + (self.target_x - self.from_x) * t;
                self.y = self.from_y + (self.target_y - self.from_y) * t;
                if self.elapsed >= Self::SLIDE_DURATION { self.x = self.target_x; self.y = self.target_y; }
            }
        } else {
            self.x = self.target_x;
            self.y = self.target_y;
        }
        // pulse (mode 2): a continuous 0.3↔0.7 oscillation, InOutQuad each leg.
        let opacity = match mode {
            CursorMode::Off => 0.0,
            CursorMode::Static => 0.7,
            CursorMode::Slide => 0.85,
            CursorMode::Pulse => {
                self.pulse_t += dt.max(0.0);
                if self.pulse_t >= Self::PULSE_LEG {
                    self.pulse_t -= Self::PULSE_LEG;
                    self.pulse_rising = !self.pulse_rising;
                }
                let t = uikit_egui::motion::ease::in_out_quad(self.pulse_t / Self::PULSE_LEG);
                if self.pulse_rising { 0.3 + (0.7 - 0.3) * t } else { 0.7 + (0.3 - 0.7) * t }
            }
        };
        (self.x, self.y, opacity)
    }

    pub fn is_animating(&self, mode: CursorMode) -> bool {
        mode == CursorMode::Pulse
            || (mode == CursorMode::Slide && ((self.target_x - self.x).abs() > 1e-4 || (self.target_y - self.y).abs() > 1e-4))
    }
}

/// Lighten a colour toward white by `factor` (0 = unchanged, 1 = white) — matches Qt's
/// `Qt.lighter(colour, 1.2)` used for the QML cursor's fill (a lightened terminal-fg, not the
/// theme accent — the cursor should read as "brighter text", not as a UI-chrome highlight).
fn lighten(c: egui::Color32, factor: f32) -> egui::Color32 {
    let f = factor.clamp(0.0, 1.0);
    let lerp = |a: u8| (a as f32 + (255.0 - a as f32) * f).round() as u8;
    egui::Color32::from_rgb(lerp(c.r()), lerp(c.g()), lerp(c.b()))
}

/// A span to emphasize in the viewport this frame (search hits, the selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Highlight {
    /// Viewport row.
    pub row: usize,
    /// Start column (character cells) and span length; a whole-row wash when `len == 0`.
    pub col: usize,
    pub len: usize,
    /// The active search hit draws stronger than the others.
    pub active: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn render_grid(
    ui: &egui::Ui,
    grid: &Grid,
    colors: &uikit_egui::PaletteColors,
    display: &settings_kit::DisplayPrefs,
    origin: egui::Pos2,
    cell_w: f32,
    cell_h: f32,
    font_size: f32,
    cursor_mode: CursorMode,
    cursor_anim: &mut CursorAnim,
    dt: f32,
    highlights: &[Highlight],
) {
    let painter = ui.painter();
    // The terminal viewport's own base is a hardcoded near-black in the QML (Canvas's default
    // cell colour, `#0d0d0d` — a sentinel literal, not theme-derived), distinct from the lighter
    // chrome bg_base around it. Paint it explicitly rather than relying on whatever panel fill
    // happens to be behind this region.
    const TERM_BASE: egui::Color32 = egui::Color32::from_rgb(0x0d, 0x0d, 0x0d);
    painter.rect_filled(
        egui::Rect::from_min_size(origin, egui::vec2(grid.cols as f32 * cell_w, grid.rows as f32 * cell_h)),
        0.0,
        TERM_BASE,
    );
    let filt = |c: egui::Color32| uikit_egui::filter_color(c, display);
    let font = egui::FontId::monospace(font_size);

    for r in 0..grid.rows {
        let y = origin.y + r as f32 * cell_h;
        let mut c = 0;
        while c < grid.cols {
            let start = grid.display_cell(r, c);
            // extend a run of identical style
            let mut end = c + 1;
            while end < grid.cols && grid.display_cell(r, end).style == start.style {
                end += 1;
            }
            // The wide-glyph tail is skipped here too: painting a NUL would
            // put a tofu box in the right half of every CJK character, and
            // painting a space would shift the run's remaining text left.
            let text: String = (c..end).map(|cc| grid.display_cell(r, cc).ch)
                .filter(|c| *c != WIDE_TAIL).collect();
            let (fg, bg) = resolve_colors(start.style, colors);
            let x0 = origin.x + c as f32 * cell_w;
            let rect = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2((end - c) as f32 * cell_w, cell_h));
            // paint background only when it differs from the base
            if start.style.bg != TermColor::Default || start.style.inverse {
                painter.rect_filled(rect, 0.0, filt(bg));
            }
            if !text.trim().is_empty() {
                // LAYOUT IS THE COST, not the paint. `painter.text` shapes the
                // string every call — measuring glyphs, resolving fonts,
                // wrapping — and in immediate mode that happens per run per
                // row per frame. Painting a pre-shaped galley skips the
                // shaping and keeps the draw.
                //
                // egui memoises galleys by (text, font, colour) inside
                // `layout_job`, so going through `ui.fonts` reuses the shaped
                // result for an unchanged row rather than rebuilding it. This
                // is the lever voiceterm's row_render describes: "an unchanged
                // row reuses its galley, a dirty row re-shapes" — and skipping
                // the PAINT instead "would save nothing and break the display".
                let galley = painter.layout_no_wrap(text, font.clone(), filt(fg));
                painter.galley(egui::pos2(x0, y), galley, filt(fg));
            }
            c = end;
        }
    }
    // search-hit / selection washes: translucent accent over the span (whole row when len==0);
    // the ACTIVE search hit gets a stroked box so ▲▼ visibly walks the matches.
    for h in highlights {
        if h.row >= grid.rows {
            continue;
        }
        let (x0, w) = if h.len == 0 {
            (origin.x, grid.cols as f32 * cell_w)
        } else {
            (origin.x + h.col as f32 * cell_w, h.len as f32 * cell_w)
        };
        let rect = egui::Rect::from_min_size(
            egui::pos2(x0, origin.y + h.row as f32 * cell_h),
            egui::vec2(w, cell_h),
        );
        let accent = filt(colors.accent);
        painter.rect_filled(rect, 2.0, accent.gamma_multiply(if h.active { 0.35 } else { 0.18 }));
        if h.active {
            painter.rect_stroke(rect, egui::CornerRadius::same(2), egui::Stroke::new(1.0_f32, accent), egui::StrokeKind::Outside);
        }
    }

    // scrolled-back badge: a small top-right pill showing how far above live the view is, so
    // "why is nothing updating" is never a mystery (the cursor is hidden in this state).
    if grid.is_scrolled_back() {
        let label = format!("\u{2191} {} ", grid.view_offset);
        let right = origin.x + grid.cols as f32 * cell_w - 4.0;
        let pos = egui::pos2(right, origin.y + 4.0);
        let galley_rect = painter.text(
            pos,
            egui::Align2::RIGHT_TOP,
            &label,
            egui::FontId::monospace(font_size * 0.9),
            filt(colors.accent),
        );
        painter.rect_stroke(
            galley_rect.expand(2.0),
            egui::CornerRadius::same(4),
            egui::Stroke::new(1.0_f32, filt(colors.accent)),
            egui::StrokeKind::Outside,
        );
    }

    // cursor — ported from voiceterm's QML `termCursor` (see CursorAnim's doc comment for the
    // exact mode semantics/timings this matches).
    if cursor_mode != CursorMode::Off && !grid.is_scrolled_back() {
        cursor_anim.set_target(grid.cur_col, grid.cur_row);
        let (cx_cells, cy_cells, opacity) = cursor_anim.tick(dt, cursor_mode);
        let cx = origin.x + cx_cells * cell_w;
        let cy = origin.y + cy_cells * cell_h;
        let cursor_color = lighten(filt(colors.text), 0.2); // Qt.lighter(th.tfg, 1.2)
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cell_w, cell_h)),
            1.0, // QML: radius: 1
            cursor_color.gamma_multiply(opacity),
        );
        if cursor_anim.is_animating(cursor_mode) {
            ui.ctx().request_repaint(); // keep the pulse/slide going
        }
    }
}

/// Cell/font-size bounds for pinch-to-zoom — clamped so the terminal never becomes illegibly
/// tiny or so huge it can't fit any real content. The cell dimensions scale proportionally with
/// font_size, keeping the same aspect ratio as the default (8.5/13.0 wide, 17.0/13.0 tall).
/// Cell/font-size bounds and ratio — named constants for what was three duplicated magic-number
/// sets (`apply_settings`, `apply_pinch`, and this module) before this pass. `set_font_size` is now
/// the single place that computes cell-from-font; `apply_settings`/`apply_pinch` both delegate to it.
pub const MIN_FONT_SIZE: f32 = 7.0;
pub const MAX_FONT_SIZE: f32 = 48.0;
const DEFAULT_FONT_SIZE: f32 = 13.0;
const CELL_W_RATIO: f32 = 0.62;
const CELL_H_RATIO: f32 = 1.3;

/// A tap or long-press's resolved terminal position + (for long-press) what it selected.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// The VIEWPORT row this selection is on, if any.
    pub row: Option<usize>,
    /// The column that was hit, for a tap. `None` for a long-press,
    /// which selects the whole line and has no single column.
    ///
    /// Carried because a host may hit-test with THIS widget (it owns the
    /// cell metrics) while the text lives in a different grid — see
    /// `cell_at`. Without the column such a host can only recover the
    /// line, so a tap-to-select-WORD silently becomes select-line.
    pub col: Option<usize>,
    /// The selected text (a word for a tap on it, a whole line for a long-press).
    pub text: String,
}

/// A minimal voiceterm view: the terminal, current prefs, and settings adoption. The egui
/// `main` connects a PTY to `terminal.feed_bytes` and calls `render` each frame; the voice and
/// mail panes attach here later.
pub struct App {
    pub terminal: Terminal,
    /// Raw bytes from the last input pass that go straight to the PTY —
    /// control codes, arrows, tab. Distinct from the submitted line, which is
    /// user text the host may echo or log.
    pending_bytes: Vec<u8>,
    /// Who encodes keyboard input — see `set_input_owner`. Default
    /// `Widget`, which is the safe side: a host that forgets to declare
    /// itself gets a working line editor rather than a dead terminal.
    input_owner: keys::InputOwner,
    pub prefs: settings_kit::Prefs,
    pub cell: (f32, f32),
    pub font_size: f32,
    /// Multitouch input (pinch-to-zoom the terminal, tap-to-select-word, long-press-to-select-line)
    /// via the shared kit bridge.
    pub touch: uikit_egui::touch::TouchSource,
    pub recognizer: uikit_egui::touch::GestureRecognizer,
    /// In-screen find state; `search_open` toggles the search bar.
    pub search: SearchState,
    pub search_open: bool,
    /// Set when the bar is opened, cleared once the query field has
    /// taken focus. See `open_search`.
    search_wants_focus: bool,
    /// Cursor interaction mode + its glide/pulse state — see `CursorMode`/`CursorAnim`.
    pub cursor_mode: CursorMode,
    cursor_anim: CursorAnim,
    /// The most recent tap/long-press selection (word or line) — cleared by the caller once read.
    pub selection: Option<Selection>,
    /// The grid size `fit_to` last resized to, so it's a no-op when the available area hasn't
    /// actually changed (resizing the grid drops any columns/rows that no longer fit).
    fitted_size: Option<(usize, usize)>,
    /// Screen position of the most recent long-press, latched for the caller (the pane action
    /// menu opens there) — take with [`App::take_long_press`]. The line selection still happens;
    /// the two compose (the menu's "copy" copies the long-pressed line).
    long_press: Option<egui::Pos2>,
    /// A tap that landed on a URL: (position, the trimmed URL) — the QML blue-link tap. Take with
    /// [`App::take_link_tap`]; the link action menu opens there.
    link_tap: Option<(egui::Pos2, String)>,
    /// Fractional-line accumulator for touch scrollback (pan/fling deltas arrive in px; whole
    /// lines are consumed as they accrue, remainder carried).
    scroll_accum: f32,
}

impl App {
    pub fn new(cols: usize, rows: usize) -> App {
        App {
            pending_bytes: Vec::new(),
            input_owner: keys::InputOwner::default(),
            terminal: Terminal::new(cols, rows),
            prefs: settings_kit::Prefs::default(),
            cell: (DEFAULT_FONT_SIZE * CELL_W_RATIO, DEFAULT_FONT_SIZE * CELL_H_RATIO),
            font_size: DEFAULT_FONT_SIZE,
            touch: uikit_egui::touch::TouchSource::new(),
            recognizer: uikit_egui::touch::GestureRecognizer::new(uikit_egui::touch::Config::default()),
            search: SearchState::default(),
            search_open: false,
            search_wants_focus: false,
            cursor_mode: CursorMode::default(), // Slide, matching the QML's default
            cursor_anim: CursorAnim::default(),
            selection: None,
            fitted_size: None,
            long_press: None,
            link_tap: None,
            scroll_accum: 0.0,
        }
    }

    /// Resize the terminal grid to fill `avail` at the current cell size — the fix for the
    /// terminal not tracking its actual pane/window size (it used to stay at whatever cols/rows
    /// `new()` was given forever, so resizing the window either left dead space or clipped the
    /// grid). Call once per frame with `ui.available_size()` before `render`; a no-op if the
    /// computed cols/rows haven't changed since the last call.
    pub fn fit_to(&mut self, avail: egui::Vec2) {
        let cols = ((avail.x / self.cell.0).floor() as usize).max(1);
        let rows = ((avail.y / self.cell.1).floor() as usize).max(1);
        if self.fitted_size != Some((cols, rows)) {
            self.terminal.grid.resize(cols, rows);
            self.fitted_size = Some((cols, rows));
        }
    }

    /// Set the font size directly (clamped to [`MIN_FONT_SIZE`], [`MAX_FONT_SIZE`]), recomputing
    /// the cell size to match — the two must move together or text stops fitting its own cells.
    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.cell = (self.font_size * CELL_W_RATIO, self.font_size * CELL_H_RATIO);
    }

    /// Convert a screen position to a (col, row) in the grid, given the terminal's on-screen
    /// origin. `None` if the position falls outside the grid.
    /// Which (column, viewport row) a screen position falls on.
    ///
    /// Public because hit-testing needs this widget's cell metrics
    /// (font size, pinch zoom) while the TEXT may live in a different
    /// grid — voiceterm renders its own and feeds this one nothing, so
    /// `handle_gestures` selecting from `self.terminal.grid` there
    /// returns empty strings. A host in that position uses this to find
    /// the cell and reads the text from its own grid.
    pub fn cell_at(&self, origin: egui::Pos2, pos: egui::Pos2) -> Option<(usize, usize)> {
        let rel = pos - origin;
        if rel.x < 0.0 || rel.y < 0.0 { return None; }
        let (col, row) = ((rel.x / self.cell.0) as usize, (rel.y / self.cell.1) as usize);
        if col < self.terminal.grid.cols && row < self.terminal.grid.rows { Some((col, row)) } else { None }
    }

    /// Consume this frame's keyboard/paste events into the input line. Returns `Some(command)` on Enter
    /// (the submitted line, already taken out of the input); typing appends, Backspace deletes, and a
    /// paste inserts its text (control chars stripped). The host runs the returned command on its PTY.
    /// Consume keyboard input for the terminal.
    ///
    /// **Returns immediately when another widget wants the keyboard.** This
    /// reads `ui.input()`, which is the GLOBAL event stream — every keystroke
    /// in the frame, whether or not a text field has focus. Without this guard
    /// the terminal steals characters from any `TextEdit` on screen, and the
    /// symptom is the one that gets reported: a URL bar that accepts one
    /// character and then stops, because the terminal is eating the rest.
    ///
    /// `egui_wants_keyboard_input()` is true exactly when a widget has focus and
    /// takes text, which is the condition under which the terminal must not
    /// look at the event stream at all.
    pub fn handle_input(&mut self, ui: &egui::Ui) -> Option<String> {
        self.pending_bytes.clear();
        if ui.ctx().egui_wants_keyboard_input() {
            return None;
        }
        // Who encodes keys. `InputOwner::Host` means the host already
        // read these events and produced bytes itself, so accumulating a
        // line and encoding control keys here would send everything
        // twice — see keys::InputOwner.
        let owner = self.input_owner;
        let mine = |c: keys::EventClass| keys::widget_handles(owner, c);
        let mut pending: Vec<u8> = Vec::new();
        let mut submitted = None;
        ui.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Text(t) => {
                        // Scroll-to-bottom happens in BOTH modes: typing
                        // returning you to the live tail is the widget's
                        // job either way, and the host's raw path has no
                        // access to the scrollback.
                        self.terminal.grid.scroll_to_bottom();
                        if mine(keys::EventClass::Text) {
                            self.terminal.input.push_str(t);
                            // LOCAL ECHO. Nothing else draws `input` -- the
                            // renderer paints the grid and the pending line is
                            // not in it -- so without this you type and the
                            // screen does not change until Enter. That is what
                            // `terminal-standalone` looked like: a live prompt
                            // that ignored the keyboard.
                            //
                            // Only under InputOwner::Widget, which is the whole
                            // point of the distinction: its own doc says "the
                            // line buffer is the only thing making typing
                            // visible". Under Host the PTY echoes, and doing it
                            // here too would double every character.
                            self.terminal.feed(t);
                        }
                    }
                    egui::Event::Key { key: egui::Key::PageUp, pressed: true, modifiers, .. }
                        if modifiers.shift =>
                    {
                        let page = self.terminal.grid.rows.saturating_sub(1).max(1);
                        self.terminal.grid.page_up(page);
                    }
                    egui::Event::Key { key: egui::Key::PageDown, pressed: true, modifiers, .. }
                        if modifiers.shift =>
                    {
                        let page = self.terminal.grid.rows.saturating_sub(1).max(1);
                        self.terminal.grid.page_down(page);
                    }
                    egui::Event::Paste(t) => {
                        // Two destinations on purpose, because there are two
                        // hosts: the demo consumes `terminal.input` as a
                        // command line, while a host with a real PTY drains
                        // `pending_bytes`. A host wires ONE of them.
                        //
                        // The PTY copy is bracketed when the program asked for
                        // it (DEC 2004). Without that, pasting a block of text
                        // into vim runs every line through its keymap — the
                        // classic wall of autoindent.
                        self.terminal.paste(t);
                        pending.extend_from_slice(
                            &keys::paste(t, self.terminal.grid.key_mode()));
                    }
                    egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. }
                        if mine(keys::EventClass::LineEdit) =>
                    {
                        // Echo the rub-out too, and ONLY if something was
                        // actually removed -- backspacing at an empty prompt
                        // must not eat the `$ ` in front of it.
                        if self.terminal.input.pop().is_some() {
                            self.terminal.feed("\u{8} \u{8}");
                        }
                    }
                    egui::Event::Key { key: egui::Key::Enter, pressed: true, .. }
                        if mine(keys::EventClass::LineEdit) =>
                    {
                        // The newline is echoed here rather than left to the
                        // host: the host is being handed a COMMAND and should
                        // not have to know whether the line it came from was
                        // already terminated on screen.
                        self.terminal.feed("\r\n");
                        submitted = Some(std::mem::take(&mut self.terminal.input));
                    }
                    // Ctrl/Cmd+F: open the find bar. Must come BEFORE the catch-all key arm
                    // below — that arm has no guard and matches every key press, so with this
                    // one after it, Ctrl+F would already be consumed as a control-code chord
                    // (queuing a raw 0x06 byte to the shell) before match ever reached here.
                    // Confirmed unreachable by the compiler's own warning before this reorder;
                    // see `ctrl_f_opens_search_instead_of_leaking_a_control_byte_to_the_shell`.
                    egui::Event::Key { key: egui::Key::F, pressed: true, modifiers, .. }
                        if modifiers.command || modifiers.ctrl =>
                    {
                        self.open_search();
                    }
                    // Ctrl/Cmd+P: reserved for the host app (voiceterm-egui's command palette).
                    // This crate has no notion of "palette" — a no-op arm here just keeps the
                    // catch-all below from sending its control byte (0x10, DLE) to the shell.
                    // Bash/readline binds Ctrl+P to "previous history entry", the same action
                    // ArrowUp already sends (`keys::Key::Up`, mapped below), so nothing unique
                    // is lost — this is deduplicating a redundant path, not removing a feature.
                    // See `ctrl_p_is_reserved_and_does_not_leak_a_control_byte_to_the_shell`.
                    egui::Event::Key { key: egui::Key::P, pressed: true, modifiers, .. }
                        if modifiers.command || modifiers.ctrl =>
                    {
                    }
                    // Everything else a SHELL needs, encoded as the bytes it
                    // parses. Without these the widget is a text field that
                    // submits lines: no Ctrl-C to stop a runaway command, no
                    // up-arrow so every command is retyped, no tab completion.
                    egui::Event::Key { key: k, pressed: true, modifiers, .. }
                        if mine(keys::EventClass::Encoded) =>
                    {
                        let m = keys::Mods {
                            ctrl: modifiers.ctrl || modifiers.command,
                            alt: modifiers.alt,
                            shift: modifiers.shift,
                        };
                        let mapped = match k {
                            egui::Key::ArrowUp => Some(keys::Key::Up),
                            egui::Key::ArrowDown => Some(keys::Key::Down),
                            egui::Key::ArrowLeft => Some(keys::Key::Left),
                            egui::Key::ArrowRight => Some(keys::Key::Right),
                            egui::Key::Home => Some(keys::Key::Home),
                            egui::Key::End => Some(keys::Key::End),
                            egui::Key::Insert => Some(keys::Key::Insert),
                            egui::Key::Delete => Some(keys::Key::Delete),
                            egui::Key::Tab => Some(keys::Key::Tab),
                            egui::Key::Escape => Some(keys::Key::Escape),
                            // PageUp/PageDown WITHOUT shift go to the shell;
                            // with shift they scroll the scrollback, which the
                            // arms above already claimed.
                            egui::Key::PageUp if !m.shift => Some(keys::Key::PageUp),
                            egui::Key::PageDown if !m.shift => Some(keys::Key::PageDown),
                            other => {
                                // Ctrl-<letter>: the control codes. egui gives
                                // no Text event for these, so without this arm
                                // Ctrl-C simply does nothing.
                                if m.ctrl {
                                    other.name().chars().next()
                                        .filter(|c| c.is_ascii_alphabetic())
                                        .map(keys::Key::Char)
                                } else { None }
                            }
                        };
                        if let Some(mk) = mapped {
                            // MODE-AWARE. This used to call `keys::encode`,
                            // which always emits CSI for the arrows — so the
                            // application-cursor branch added to keys.rs, and
                            // the DECCKM tracking added to the parser, were
                            // both real and both unreachable from here. Three
                            // pieces of one fix, each landed separately, and
                            // the chain was only complete at the third.
                            let mode = self.terminal.grid.key_mode();
                            if let Some(bytes) = keys::encode_mode(mk, m, mode) {
                                self.terminal.grid.scroll_to_bottom();
                                pending.extend_from_slice(&bytes);
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
        self.pending_bytes = pending;
        submitted
    }

    /// Bytes produced by the last `handle_input` that must go straight to the
    /// PTY rather than into the line buffer — control codes, arrows, tab.
    ///
    /// Separate from the submitted line because they are a different thing: a
    /// line is user text the host may echo or log, these are raw terminal
    /// input the shell interprets immediately.
    /// Declare who encodes keyboard input.
    ///
    /// A host that runs its own key encoder over the same egui events
    /// MUST call this with [`keys::InputOwner::Host`], or every
    /// keystroke reaches the shell twice. Defaults to `Widget`, which is
    /// right for a host with no shell at all.
    pub fn set_input_owner(&mut self, owner: keys::InputOwner) {
        self.input_owner = owner;
    }

    pub fn input_owner(&self) -> keys::InputOwner {
        self.input_owner
    }

    pub fn take_pending_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_bytes)
    }

    /// Open the find bar (the compass "search" action / Ctrl+F).
    pub fn open_search(&mut self) {
        self.search_open = true;
        // AND TAKE THE KEYBOARD. A find bar that opens without focus
        // sends your query to whatever had it before — for a terminal
        // host that is the SHELL, so pressing Ctrl+F and typing
        // "FINDME" ran `FINDME` as a command. Seen in a screenshot: the
        // field still showed its placeholder while the text appeared at
        // the prompt.
        //
        // Deferred to the next frame rather than done here because the
        // widget does not exist yet; `search_ui` consumes the flag.
        self.search_wants_focus = true;
    }

    /// Render the search bar when open: a query field, an `n/N` match counter, prev/next, and close.
    /// Re-runs the search when the query changes; Esc or ✕ closes. Returns whether it's still open.
    pub fn search_ui(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.search_open {
            return false;
        }
        ui.horizontal(|ui| {
            ui.label("🔍");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search.query).id_salt("lib-self_search_query")
                    .hint_text("find in screen…")
                    .desired_width(180.0),
            );
            if std::mem::take(&mut self.search_wants_focus) {
                resp.request_focus();
            }
            if resp.changed() {
                self.search.run(&self.terminal.grid);
            }
            let n = self.search.count();
            let cur = if n == 0 { 0 } else { self.search.active + 1 };
            ui.label(format!("{cur}/{n}"));
            if resp.changed() || resp.lost_focus() {
                self.scroll_to_search_active();
            }
            // ASCII labels, not the triangle/cross glyphs. egui's
            // default font stack does not carry them on every platform:
            // a screenshot of this bar under a plain X session showed
            // three empty boxes, and a control you cannot identify is a
            // control you will not press. Found by looking; no test can
            // see a missing glyph.
            if ui.button("prev").clicked() {
                self.search.prev();
                self.scroll_to_search_active();
            }
            if ui.button("next").clicked() {
                self.search.next();
                self.scroll_to_search_active();
            }
            if ui.button("close").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search_open = false;
                self.search.query.clear();
                self.search.matches.clear();
            }
        });
        self.search_open
    }

    /// Adopt cross-app settings (theme/text/button + gesture feel handled by the kit).
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        self.prefs = prefs.clone();
        uikit_egui::apply_prefs(ctx, prefs);
        // scale the monospace cell to the text-size preference
        self.set_font_size(DEFAULT_FONT_SIZE * prefs.touch.text_scale.max(0.5));
    }

    /// Apply a pinch scale factor to the terminal font (pure; clamped to a readable range).
    pub fn apply_pinch(&mut self, rel: f32) {
        self.set_font_size(self.font_size * rel);
    }

    /// Poll multitouch for this frame and fold it into terminal actions: a tap selects the word
    /// under it, a long-press selects the whole line, and a pinch live-scales the font. Returns
    /// whether fling momentum is still gliding, so the caller can `ctx.request_repaint()`.
    /// Graceful — no touch input means no gestures and a no-op. `origin` is the terminal's
    /// on-screen top-left (`ui.min_rect().min`, the same value `render` uses) — needed to resolve
    /// a tap/long-press's screen position to a grid cell.
    pub fn handle_gestures(&mut self, ctx: &egui::Context, origin: egui::Pos2) -> bool {
        let (gestures, gliding) = self.touch.poll(ctx, &mut self.recognizer);
        for g in gestures {
            match g {
                uikit_egui::touch::Gesture::Pinch { rel, .. } => self.apply_pinch(rel),
                uikit_egui::touch::Gesture::Tap(p) => {
                    if let Some((col, row)) = self.cell_at(origin, egui::pos2(p.x, p.y)) {
                        // view-aware: while scrolled back, the tapped viewport row shows a
                        // scrollback line — select THAT text, not the live row hidden under it
                        let line = self.terminal.grid.global_line_of_viewport_row(row);
                        let word = word_at(&self.terminal.grid.line_text_global(line), col);
                        // a tap on a URL is a link tap (the QML blue-link affordance); the word
                        // selection still happens, so copy-word flows keep working
                        if let Some(url) = url_in_word(&word) {
                            self.link_tap = Some((egui::pos2(p.x, p.y), url));
                        }
                        self.selection = Some(Selection { row: Some(row), col: Some(col), text: word });
                    }
                }
                uikit_egui::touch::Gesture::LongPress(p) => {
                    if let Some((_, row)) = self.cell_at(origin, egui::pos2(p.x, p.y)) {
                        let line = self.terminal.grid.global_line_of_viewport_row(row);
                        // No column: a long-press takes the whole line.
                        self.selection = Some(Selection { row: Some(row), col: None, text: self.terminal.grid.line_text_global(line) });
                    }
                    self.long_press = Some(egui::pos2(p.x, p.y));
                }
                // touch scrollback: content follows the finger — dragging DOWN reveals earlier
                // lines. Same for momentum after a fling; whole lines consumed as px accrue.
                uikit_egui::touch::Gesture::Pan { delta, .. }
                | uikit_egui::touch::Gesture::MomentumTick { delta } => {
                    self.scroll_accum += delta.y / self.cell.1.max(1.0);
                    let whole = self.scroll_accum.trunc();
                    self.scroll_accum -= whole;
                    if whole >= 1.0 {
                        self.terminal.grid.page_up(whole as usize);
                    } else if whole <= -1.0 {
                        self.terminal.grid.page_down((-whole) as usize);
                    }
                }
                _ => {}
            }
        }
        gliding
    }

    /// The latched long-press position, if one happened since the last take. One-shot: reading
    /// clears it, so the pane menu opens exactly once per gesture.
    pub fn take_long_press(&mut self) -> Option<egui::Pos2> {
        self.long_press.take()
    }

    /// The latched link tap (position + trimmed URL), one-shot like [`App::take_long_press`].
    pub fn take_link_tap(&mut self) -> Option<(egui::Pos2, String)> {
        self.link_tap.take()
    }

    /// Bring the active search match into view — pages into scrollback when the match line is
    /// up there, or back to live when it's on screen.
    pub fn scroll_to_search_active(&mut self) {
        if let Some(m) = self.search.matches.get(self.search.active) {
            self.terminal.grid.view_offset = self.terminal.grid.offset_to_show_line(m.line);
        }
    }

    /// This frame's viewport highlights: every visible search hit (active one stronger) plus a
    /// whole-row wash on the selection.
    fn viewport_highlights(&self) -> Vec<Highlight> {
        let mut out = Vec::new();
        for (i, m) in self.search.matches.iter().enumerate() {
            if let Some(row) = self.terminal.grid.viewport_row_of_line(m.line) {
                out.push(Highlight { row, col: m.col, len: m.len.max(1), active: i == self.search.active });
            }
        }
        if let Some(sel) = &self.selection {
            if let Some(row) = sel.row {
                if row < self.terminal.grid.rows {
                    out.push(Highlight { row, col: 0, len: 0, active: false });
                }
            }
        }
        out
    }

    /// Draw the terminal into the current UI region.
    pub fn render(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        let origin = ui.min_rect().min;
        self.render_at(ui, colors, origin);
    }

    /// Draw the terminal with its top-left at `origin` instead of at the UI cursor.
    ///
    /// A tiling frontend places several panes inside ONE ui region, so the origin has to be
    /// caller-supplied — `ui.min_rect().min` is the whole region for every pane. `render` is
    /// exactly this with the region's own corner, so existing callers are unchanged.
    pub fn render_at(
        &mut self,
        ui: &mut egui::Ui,
        colors: &uikit_egui::PaletteColors,
        origin: egui::Pos2,
    ) {
        let dt = ui.input(|i| i.stable_dt).min(0.1); // clamp a stalled first frame's huge dt
        let highlights = self.viewport_highlights();
        render_grid(ui, &self.terminal.grid, colors, &self.prefs.display, origin, self.cell.0, self.cell.1, self.font_size, self.cursor_mode, &mut self.cursor_anim, dt, &highlights);

        // Highlight search hits that fall in the visible region (matches in scrollback still count in
        // the n/N tally, they just aren't on screen to box). The active hit gets a solid box.
        if self.search_open && !self.search.matches.is_empty() {
            let scroll = self.terminal.grid.scrollback.len();
            for (i, m) in self.search.matches.iter().enumerate() {
                if m.line < scroll {
                    continue; // in scrollback, off screen
                }
                let row = m.line - scroll;
                if row >= self.terminal.grid.rows {
                    continue;
                }
                let x = origin.x + m.col as f32 * self.cell.0;
                let y = origin.y + row as f32 * self.cell.1;
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(m.len as f32 * self.cell.0, self.cell.1),
                );
                let active = i == self.search.active;
                let accent = egui::Color32::from_rgb(230, 180, 60);
                let col = if active { accent } else { accent.gamma_multiply(0.5) };
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(2),
                    egui::Stroke::new(if active { 2.0_f32 } else { 1.0_f32 }, col),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
}

/// One search hit: `col` and `len` are in characters within line `line` (indexing [`Grid::all_lines`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SearchMatch {
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

/// The word at character-column `col` in `line` (whitespace-delimited), for tap-to-select.
/// Character-indexed (safe for multibyte); an empty string if `col` lands on whitespace or past
/// the end of the line.
pub fn word_at(line: &str, col: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    if col >= chars.len() || chars[col].is_whitespace() {
        return String::new();
    }
    let start = chars[..col].iter().rposition(|c| c.is_whitespace()).map(|i| i + 1).unwrap_or(0);
    let end = chars[col..].iter().position(|c| c.is_whitespace()).map(|i| col + i).unwrap_or(chars.len());
    chars[start..end].iter().collect()
}

/// If `word` is (or wraps) an http/https URL, the URL with trailing punctuation trimmed —
/// `None` otherwise. This is the tap-side of the QML blue-link affordance: terminal output like
/// `See https://example.com/x.` or `(https://a.b/c),` yields the clean URL. Pure.
pub fn url_in_word(word: &str) -> Option<String> {
    let start = word.find("http://").or_else(|| word.find("https://"))?;
    let mut url = &word[start..];
    // trim the punctuation shells output wraps URLs in: closers, sentence enders, quotes
    while let Some(last) = url.chars().last() {
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '>' | '"' | '\'') {
            url = &url[..url.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    // require something after the scheme
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    if rest.is_empty() {
        return None;
    }
    Some(url.to_string())
}

/// Case-insensitive search of `lines` for `query`, returning every non-overlapping hit in reading
/// order. Character-indexed (safe for multibyte). Empty query → no matches. Pure.
pub fn search_lines(lines: &[String], query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let mut out = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let hay: Vec<char> = line.to_lowercase().chars().collect();
        if hay.len() < q.len() {
            continue;
        }
        let mut i = 0;
        while i + q.len() <= hay.len() {
            if hay[i..i + q.len()] == q[..] {
                out.push(SearchMatch { line: li, col: i, len: q.len() });
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// In-terminal search state: the query, its matches over the grid, and the active match. Drives the
/// search bar (next/prev cycle through hits).
#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub active: usize,
}

impl SearchState {
    /// Re-run the search against `grid` (call when the query changes or the screen scrolls).
    pub fn run(&mut self, grid: &Grid) {
        self.matches = search_lines(&grid.all_lines(), &self.query);
        if self.active >= self.matches.len() {
            self.active = 0;
        }
    }
    pub fn next(&mut self) {
        if !self.matches.is_empty() {
            self.active = (self.active + 1) % self.matches.len();
        }
    }
    pub fn prev(&mut self) {
        if !self.matches.is_empty() {
            self.active = (self.active + self.matches.len() - 1) % self.matches.len();
        }
    }
    pub fn current(&self) -> Option<SearchMatch> {
        self.matches.get(self.active).copied()
    }
    pub fn count(&self) -> usize {
        self.matches.len()
    }
}

#[cfg(test)]
mod tests {
    // ── reflow on resize ─────────────────────────────────────────────────────

    /// History and screen as text, blank rows dropped: what a user would read.
    fn text(t: &Terminal) -> Vec<String> {
        t.grid.all_lines().into_iter().filter(|l| !l.is_empty()).collect()
    }

    /// THE REPORTED BUG: shrink a pane so a line wraps, grow it back, and the
    /// line is whole again -- before, it stayed chopped at the narrow width.
    #[test]
    fn a_line_shrunk_then_grown_back_is_whole_again() {
        let mut t = Terminal::new(20, 5);
        t.feed("the quick brown fox\r\n$ ");
        t.resize(8, 5);
        assert_eq!(t.grid.row_text(0), "the quic");
        assert_eq!(t.grid.row_text(1), "k brown");
        assert_eq!(t.grid.row_text(2), "fox", "narrowing wraps the line, it does not cut it");
        t.resize(20, 5);
        assert_eq!(t.grid.row_text(0), "the quick brown fox", "widening unwraps it again");
        assert_eq!(t.grid.row_text(1), "$");
    }

    /// Split panes shrink and grow repeatedly: the history keeps its ORDER and
    /// gains no duplicates. The old workaround appended every truncated
    /// mid-screen row to the end of scrollback, while its narrow copy stayed.
    #[test]
    fn history_survives_repeated_resizes_in_order_without_duplicates() {
        let mut t = Terminal::new(30, 4);
        let lines: Vec<String> = (0..12).map(|i| format!("line {i:02} with some words")).collect();
        for l in &lines {
            t.feed(&format!("{l}\r\n"));
        }
        let before = text(&t);
        for &(c, r) in &[(10, 4), (7, 6), (30, 4), (13, 3), (30, 4)] {
            t.resize(c, r);
        }
        assert_eq!(text(&t), before, "same lines, same order, nothing lost or duplicated");
        assert_eq!(before, lines);
    }

    #[test]
    fn real_line_breaks_are_never_joined() {
        let mut t = Terminal::new(10, 4);
        t.feed("ab\r\ncd\r\n");
        t.resize(40, 4);
        assert_eq!((t.grid.row_text(0).as_str(), t.grid.row_text(1).as_str()), ("ab", "cd"));
    }

    /// The cursor follows its text: typing after a round trip continues the
    /// same line, not somewhere the old geometry left it.
    #[test]
    fn the_cursor_follows_its_text_through_a_reflow() {
        let mut t = Terminal::new(8, 4);
        t.feed("$ abcdefghij"); // wraps: "$ abcdef" / "ghij"
        assert_eq!((t.grid.cur_row, t.grid.cur_col), (1, 4));
        t.resize(20, 4);
        assert_eq!((t.grid.cur_row, t.grid.cur_col), (0, 12));
        t.feed("K");
        assert_eq!(t.grid.row_text(0), "$ abcdefghijK");
        t.resize(5, 4);
        t.feed("L");
        assert_eq!(text(&t).concat(), "$ abcdefghijKL", "still one line, cursor at its end");
    }

    /// A wide character is never split across the margin, and a round trip
    /// restores the text exactly.
    #[test]
    fn wide_characters_are_never_split_by_a_reflow() {
        let mut t = Terminal::new(12, 4);
        t.feed("ab\u{4e2d}\u{6587}cd");
        t.resize(3, 6);
        for r in 0..t.grid.rows {
            let last = t.grid.cell(r, 2).ch;
            let wide_head = Grid::char_width(last) == 2;
            assert!(!wide_head, "row {r} ends with half a wide character");
        }
        t.resize(12, 4);
        assert_eq!(t.grid.row_text(0), "ab\u{4e2d}\u{6587}cd");
    }

    /// The shell UNDER a full-screen program is reflowed too, and is whole
    /// when the program exits.
    #[test]
    fn the_parked_shell_is_reflowed_while_vim_has_the_screen() {
        let mut t = Terminal::new(20, 4);
        t.feed("the quick brown fox\r\n$ \x1b[?1049h");
        t.resize(8, 4);
        t.resize(20, 4);
        t.feed("\x1b[?1049l");
        assert_eq!(t.grid.row_text(0), "the quick brown fox");
    }

    /// A row whose end was erased no longer continues onto the next, so a
    /// reflow must not glue them together.
    #[test]
    fn an_erased_line_end_is_not_rejoined() {
        let mut t = Terminal::new(8, 4);
        t.feed("abcdefghij"); // soft-wrapped at 8
        t.feed("\x1b[1;5H\x1b[K"); // row 0: erase from column 5 to the end
        t.resize(20, 4);
        assert_eq!(t.grid.row_text(0), "abcd");
        assert_eq!(t.grid.row_text(1), "ij");
    }

    /// Blank rows below the cursor are space, not lines: a fresh terminal's
    /// few lines stay at the top instead of sinking to the bottom.
    #[test]
    fn a_short_screen_stays_at_the_top_after_a_resize() {
        let mut t = Terminal::new(20, 10);
        t.feed("one\r\ntwo\r\n$ ");
        t.resize(12, 8);
        assert_eq!(t.grid.row_text(0), "one");
        assert_eq!(t.grid.cur_row, 2);
        assert!(t.grid.scrollback.is_empty());
    }

    use super::*;

    #[test]
    fn url_in_word_trims_wrapping_punctuation_and_rejects_non_urls() {
        assert_eq!(url_in_word("https://example.com/x").as_deref(), Some("https://example.com/x"));
        assert_eq!(url_in_word("https://example.com/x.").as_deref(), Some("https://example.com/x"));
        assert_eq!(url_in_word("(https://a.b/c),").as_deref(), Some("https://a.b/c"));
        assert_eq!(url_in_word("\"http://a.b\"").as_deref(), Some("http://a.b"));
        assert_eq!(url_in_word("see:https://a.b/q?x=1").as_deref(), Some("https://a.b/q?x=1"));
        assert_eq!(url_in_word("hello"), None);
        assert_eq!(url_in_word("https://"), None); // scheme alone is not a link
        assert_eq!(url_in_word("ftp://a.b"), None); // only http(s), as in the QML link detector
    }

    #[test]
    fn a_tap_carries_its_column_and_a_long_press_does_not() {
        // The distinction is what lets a host that hit-tests here but
        // reads text elsewhere tell select-WORD from select-LINE. Without
        // it, such a host can only recover the line and tap-to-select-word
        // silently becomes select-line.
        let word = Selection { row: Some(2), col: Some(7), text: "cargo".into() };
        let line = Selection { row: Some(2), col: None, text: "cargo test".into() };
        assert!(word.col.is_some());
        assert!(line.col.is_none(), "a long-press has no single column");
    }

    #[test]
    fn word_at_finds_the_whitespace_delimited_word() {
        assert_eq!(word_at("hello world", 0), "hello");
        assert_eq!(word_at("hello world", 4), "hello");
        assert_eq!(word_at("hello world", 6), "world");
        assert_eq!(word_at("hello world", 10), "world");
        assert_eq!(word_at("hello world", 5), "", "landing on the space itself is empty");
        assert_eq!(word_at("hello world", 999), "", "past the end of the line is empty");
        assert_eq!(word_at("", 0), "");
    }

    #[test]
    fn word_at_handles_multibyte_characters_by_char_index_not_byte_index() {
        // "café bar" — é is multibyte; a naive byte-index would panic or mis-slice
        let line = "café bar";
        assert_eq!(word_at(line, 2), "café");
        assert_eq!(word_at(line, 5), "bar");
    }

    #[test]
    fn fit_to_resizes_the_grid_to_the_available_space() {
        let mut app = App::new(80, 24);
        app.set_font_size(10.0); // a known cell size to compute against
        let (cw, ch) = app.cell;
        app.fit_to(egui::vec2(cw * 40.0, ch * 10.0));
        assert_eq!(app.terminal.grid.cols, 40);
        assert_eq!(app.terminal.grid.rows, 10);
    }

    #[test]
    fn fit_to_is_a_no_op_when_the_computed_size_hasnt_changed() {
        let mut app = App::new(80, 24);
        app.set_font_size(10.0);
        let (cw, ch) = app.cell;
        app.fit_to(egui::vec2(cw * 40.0, ch * 10.0));
        // feed some content, then fit again at the SAME size — the grid (and its content) must
        // not have been reset by a redundant resize
        app.terminal.feed("hello");
        app.fit_to(egui::vec2(cw * 40.0, ch * 10.0));
        assert_eq!(app.terminal.grid.row_text(0), "hello", "a same-size fit_to must not clear the grid");
    }

    #[test]
    fn fit_to_never_produces_a_zero_sized_grid() {
        let mut app = App::new(80, 24);
        app.fit_to(egui::vec2(0.0, 0.0));
        assert!(app.terminal.grid.cols >= 1 && app.terminal.grid.rows >= 1, "a collapsed pane shouldn't zero out the grid");
    }

    #[test]
    fn set_font_size_clamps_to_the_readable_range_and_keeps_cell_in_ratio() {
        let mut app = App::new(80, 24);
        app.set_font_size(1.0); // absurdly small
        assert_eq!(app.font_size, MIN_FONT_SIZE);
        app.set_font_size(500.0); // absurdly large
        assert_eq!(app.font_size, MAX_FONT_SIZE);
        app.set_font_size(20.0);
        assert_eq!(app.font_size, 20.0);
        assert!((app.cell.0 - 20.0 * CELL_W_RATIO).abs() < 1e-4);
        assert!((app.cell.1 - 20.0 * CELL_H_RATIO).abs() < 1e-4);
    }

    #[test]
    fn apply_pinch_and_apply_settings_share_the_same_clamped_setter() {
        // regression guard for the three-duplicated-magic-numbers bug: both paths must land on
        // exactly the same cell size for the same font size, since they now delegate to the same
        // set_font_size rather than each recomputing cell independently.
        let mut a = App::new(80, 24);
        let mut b = App::new(80, 24);
        a.set_font_size(22.0);
        b.apply_pinch(22.0 / DEFAULT_FONT_SIZE);
        assert!((a.cell.0 - b.cell.0).abs() < 1e-3);
        assert!((a.cell.1 - b.cell.1).abs() < 1e-3);
        assert_eq!(a.font_size, b.font_size);
    }

    #[test]
    fn global_line_mapping_and_search_jump_offsets() {
        let mut g = Grid::new(4, 3);
        let mut p = Parser::new();
        for i in 0..10 {
            p.feed(&format!("l{i}\r\n"), &mut g);
        }
        let sb = g.scrollback.len();
        assert!(sb >= 7);
        // live rows map with offset 0
        assert_eq!(g.viewport_row_of_line(sb), Some(0));
        assert_eq!(g.global_line_of_viewport_row(0), sb);
        // a live line needs no offset
        assert_eq!(g.offset_to_show_line(sb + 1), 0);
        // a scrollback line: the computed offset actually makes it visible
        let target = 2usize;
        g.view_offset = g.offset_to_show_line(target);
        let row = g.viewport_row_of_line(target).expect("target line visible after jump");
        assert!(row < g.rows);
        // and the text read through the global index matches the viewport cell content
        let via_global = g.line_text_global(target);
        let via_display: String = (0..g.cols).map(|c| g.display_cell(row, c).ch).collect();
        assert_eq!(via_global, via_display.trim_end());
        // clamped for very early lines
        assert!(g.offset_to_show_line(0) <= sb);
    }

    #[test]
    fn scrolled_back_tap_selects_the_scrollback_line_not_the_live_row() {
        let mut app = App::new(8, 2);
        for i in 0..6 {
            app.terminal.feed(&format!("line{i}\r\n"));
        }
        app.terminal.grid.page_up(3);
        let shown = app.terminal.grid.global_line_of_viewport_row(0);
        let expect = app.terminal.grid.line_text_global(shown);
        // simulate what the LongPress handler now does
        let line = app.terminal.grid.global_line_of_viewport_row(0);
        app.selection = Some(Selection { row: Some(0), col: None, text: app.terminal.grid.line_text_global(line) });
        assert_eq!(app.selection.as_ref().unwrap().text, expect);
        assert_ne!(expect, app.terminal.grid.row_text(0), "scrolled view must not read the live row");
    }

    #[test]
    fn scrollback_paging_windows_and_pins_the_view() {
        let mut g = Grid::new(4, 2); // 4 cols, 2 rows
        let mut p = Parser::new();
        for line in ["aa", "bb", "cc", "dd", "ee"] {
            p.feed(&format!("{line}\r\n"), &mut g);
        }
        // 2 live rows; earlier lines went to scrollback
        let sb = g.scrollback.len();
        assert!(sb >= 3);
        assert!(!g.is_scrolled_back());
        // page up one line: the top viewport row shows the most recent scrollback line
        g.page_up(1);
        let top: String = (0..2).map(|c| g.display_cell(0, c).ch).collect();
        let expected: String = g.scrollback[sb - 1][..2].iter().map(|c| c.ch).collect();
        assert_eq!(top, expected);
        // page_up clamps to what exists
        g.page_up(999);
        assert_eq!(g.view_offset, g.scrollback.len());
        // new output pins the view: offset grows with the new scrollback line
        let before = g.view_offset;
        p.feed("ff\r\n", &mut g);
        assert_eq!(g.view_offset, (before + 1).min(g.scrollback.len()));
        // page back down to live
        g.page_down(9999);
        assert!(!g.is_scrolled_back());
        assert_eq!(g.display_cell(0, 0).ch, g.cell(0, 0).ch);
    }

    #[test]
    fn tap_selects_the_word_and_long_press_selects_the_line() {
        let mut app = App::new(80, 24);
        app.terminal.feed("hello world");
        let origin = egui::pos2(0.0, 0.0);
        let (cw, ch) = app.cell;
        // a tap in the middle of "world" (col ~8) should select "world"
        let tap_pos = egui::pos2(cw * 8.5, ch * 0.5);
        if let Some((col, row)) = app.cell_at(origin, tap_pos) {
            let word = word_at(&app.terminal.grid.row_text(row), col);
            app.selection = Some(Selection { row: Some(row), col: Some(col), text: word });
        }
        assert_eq!(app.selection.as_ref().map(|s| s.text.as_str()), Some("world"));
    }

    #[test]
    fn cell_at_returns_none_outside_the_grid() {
        let app = App::new(80, 24);
        assert!(app.cell_at(egui::pos2(0.0, 0.0), egui::pos2(-5.0, -5.0)).is_none(), "negative offset is off-grid");
        let far = egui::pos2(app.cell.0 * 1000.0, app.cell.1 * 1000.0);
        assert!(app.cell_at(egui::pos2(0.0, 0.0), far).is_none(), "far past the last column/row is off-grid");
    }

    #[test]
    fn cursor_slide_glides_to_target_and_stops() {
        let mut a = CursorAnim::default();
        a.set_target(5, 2);
        let mut last = (0.0, 0.0, 0.0);
        for _ in 0..20 { last = a.tick(1.0 / 60.0, CursorMode::Slide); }
        assert!((last.0 - 5.0).abs() < 1e-3 && (last.1 - 2.0).abs() < 1e-3, "should settle on target, got {last:?}");
        assert!((last.2 - 0.85).abs() < 1e-6, "slide mode holds a static 0.85 opacity");
        assert!(!a.is_animating(CursorMode::Slide));
    }

    #[test]
    fn cursor_static_and_off_snap_instantly_ignoring_target() {
        let mut a = CursorAnim::default();
        a.set_target(9, 4);
        let (x, y, op) = a.tick(1.0 / 60.0, CursorMode::Static);
        assert_eq!((x, y), (9.0, 4.0), "non-slide modes snap immediately, no glide");
        assert!((op - 0.7).abs() < 1e-6);
    }

    #[test]
    fn cursor_pulse_oscillates_between_0_3_and_0_7() {
        let mut a = CursorAnim::default();
        let mut seen_low = false;
        let mut seen_high = false;
        for _ in 0..200 {
            let (_, _, op) = a.tick(1.0 / 60.0, CursorMode::Pulse);
            assert!((0.3..=0.7).contains(&op), "pulse must stay within [0.3, 0.7], got {op}");
            if op < 0.35 { seen_low = true; }
            if op > 0.65 { seen_high = true; }
        }
        assert!(seen_low && seen_high, "should oscillate through both ends over ~3+ legs of 600ms at 60fps/200 frames");
    }

    #[test]
    fn cursor_off_is_zero_opacity() {
        let mut a = CursorAnim::default();
        let (_, _, op) = a.tick(1.0 / 60.0, CursorMode::Off);
        assert_eq!(op, 0.0);
    }

    #[test]
    fn lighten_moves_toward_white_and_zero_factor_is_identity() {
        let c = egui::Color32::from_rgb(0x20, 0x40, 0x60);
        assert_eq!(lighten(c, 0.0), c);
        let l = lighten(c, 0.2);
        assert!(l.r() > c.r() && l.g() > c.g() && l.b() > c.b());
    }

    #[test]
    fn search_finds_case_insensitive_char_indexed_matches() {
        let lines = vec!["Error: file not found".to_string(), "another error here".to_string(), "clean".to_string()];
        let m = search_lines(&lines, "error");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], SearchMatch { line: 0, col: 0, len: 5 }); // "Error" case-insensitive
        assert_eq!(m[1], SearchMatch { line: 1, col: 8, len: 5 });
        assert!(search_lines(&lines, "").is_empty()); // empty query
        assert!(search_lines(&lines, "zzz").is_empty());
    }

    #[test]
    fn search_state_cycles_matches() {
        let mut g = Grid::new(40, 3);
        for ch in "foo bar foo".chars() {
            g.put(ch);
        }
        let mut st = SearchState { query: "foo".to_string(), ..Default::default() };
        st.run(&g);
        assert_eq!(st.count(), 2);
        assert_eq!(st.current().unwrap().col, 0);
        st.next();
        assert_eq!(st.current().unwrap().col, 8);
        st.next(); // wraps
        assert_eq!(st.current().unwrap().col, 0);
        st.prev(); // wraps back
        assert_eq!(st.current().unwrap().col, 8);
    }

    #[test]
    fn paste_appends_and_strips_controls() {
        let mut t = Terminal::new(20, 3);
        t.stage("cd ");
        t.paste("src/main.rs
");
        assert_eq!(t.input, "cd src/main.rs"); // newline dropped, text appended
    }

    #[test]
    fn terminal_clear_copy_interrupt() {
        let mut t = Terminal::new(20, 4);
        t.feed("hello");
        t.stage("git status");
        assert_eq!(t.screen_text(), "hello");
        assert_eq!(t.input, "git status");
        t.interrupt();
        assert_eq!(t.input, ""); // interrupt clears the input line
        t.clear();
        assert_eq!(t.screen_text(), ""); // screen cleared
    }

    #[test]
    fn pinch_zooms_font_within_bounds() {
        let mut app = App::new(80, 24);
        let base = app.font_size;
        app.apply_pinch(1.5);
        assert!(app.font_size > base, "pinch out grows the font");
        assert!((app.cell.0 - app.font_size * 0.62).abs() < 1e-3, "cell tracks font");
        // clamps: repeated zoom-in saturates at 48, zoom-out at 7
        for _ in 0..20 {
            app.apply_pinch(2.0);
        }
        assert!(app.font_size <= 48.0);
        for _ in 0..20 {
            app.apply_pinch(0.1);
        }
        assert!(app.font_size >= 7.0);
    }

    fn term() -> Terminal {
        Terminal::new(20, 5)
    }

    #[test]
    fn prints_text_and_advances_cursor() {
        let mut t = term();
        t.feed("hello");
        assert_eq!(t.grid.row_text(0), "hello");
        assert_eq!((t.grid.cur_row, t.grid.cur_col), (0, 5));
    }

    #[test]
    fn newline_and_carriage_return() {
        let mut t = term();
        t.feed("a\r\nb");
        assert_eq!(t.grid.row_text(0), "a");
        assert_eq!(t.grid.row_text(1), "b");
    }

    #[test]
    fn wraps_at_column_limit() {
        let mut t = Terminal::new(4, 3);
        t.feed("abcde");
        assert_eq!(t.grid.row_text(0), "abcd");
        assert_eq!(t.grid.row_text(1), "e");
    }

    #[test]
    fn the_primary_screen_survives_a_full_screen_program() {
        let mut t = Terminal::new(20, 5);
        t.feed("shell output\r\nsecond line");
        t.feed("\x1b[?1049h");
        assert!(t.grid.on_alt_screen());
        assert_eq!(t.grid.row_text(0), "", "the alternate screen was not blank");
        t.feed("PAGER");
        assert_eq!(t.grid.row_text(0), "PAGER");
        t.feed("\x1b[?1049l");
        assert!(!t.grid.on_alt_screen());
        assert_eq!(t.grid.row_text(0), "shell output", "the shell's output was lost");
        assert_eq!(t.grid.row_text(1), "second line");
        // And the cursor came back, so the next thing the shell prints
        // continues its own line rather than starting wherever the pager left.
        t.feed("X");
        assert_eq!(t.grid.row_text(1), "second lineX");
    }

    #[test]
    fn the_alternate_screen_does_not_reach_scrollback() {
        // A pager scrolling a long file would otherwise push every frame of
        // its own rendering into the buffer and bury the shell session — and
        // the buffer exists to hold the shell session. Both directions: the
        // primary screen must still save.
        let mut t = Terminal::new(6, 3);
        t.feed("\x1b[?1049h");
        for _ in 0..10 { t.feed("x\r\n"); }
        assert!(t.grid.scrollback.is_empty(), "the pager filled the scrollback");
        t.feed("\x1b[?1049l");
        for _ in 0..10 { t.feed("y\r\n"); }
        assert!(!t.grid.scrollback.is_empty(), "the primary screen stopped saving history");
    }

    #[test]
    fn a_redundant_enter_does_not_clobber_the_parked_screen() {
        // Programs do send this — a pager launched from inside an editor. A
        // second save would overwrite the primary buffer with the alternate
        // one, which is the exact loss the alternate screen exists to prevent,
        // arriving through the mechanism meant to prevent it.
        let mut t = Terminal::new(20, 3);
        t.feed("keep me\x1b[?1049hfirst\x1b[?1049hsecond\x1b[?1049l");
        assert_eq!(t.grid.row_text(0), "keep me");
    }

    #[test]
    fn a_leave_with_no_alternate_screen_is_a_no_op() {
        // A stray `?1049l` from a program that died mid-switch must not eat
        // the screen.
        let mut t = Terminal::new(20, 3);
        t.feed("intact\x1b[?1049l");
        assert_eq!(t.grid.row_text(0), "intact");
        assert!(!t.grid.on_alt_screen());
    }

    #[test]
    fn the_older_spelling_swaps_the_buffer_and_nothing_else() {
        // `?47`/`?1047` restore the buffer and say nothing about the cursor,
        // which is exactly why `?1049` was added. Verified against tmux: the
        // character after `?47l` lands where the ALTERNATE screen reached.
        let mut t = Terminal::new(10, 3);
        t.feed("abc\x1b[2;3H\x1b[?47hXY\x1b[?47lZ");
        assert_eq!(t.grid.row_text(0), "abc", "the primary buffer was not restored");
        assert_eq!(t.grid.row_text(1), "    Z");
        assert_eq!(t.grid.cur_col, 5);
    }

    #[test]
    fn a_resize_while_parked_resizes_the_parked_screen_too() {
        // The parked buffer is sized for the old grid. Leaving it stale makes
        // `leave_alt_screen` install a Vec of the wrong length, and every
        // index after that lands on the wrong row — a corruption that surfaces
        // minutes later, when the pager exits.
        let mut t = Terminal::new(10, 6);
        t.feed("primary\x1b[?1049h");
        t.resize(6, 3);
        t.feed("\x1b[?1049l");
        assert_eq!(t.grid.cells_len_for_test(), 6 * 3, "the parked buffer was the old size");
        assert_eq!(t.grid.row_text(0), "primar", "the primary content did not survive the resize");
    }

    #[test]
    fn save_and_restore_carry_the_pen_as_well_as_the_position() {
        // The pen is the half that is easy to leave out: a prompt saves,
        // switches colour to draw elsewhere, restores, and expects its own
        // colour back. Asserting only the position would pass on that bug.
        let mut t = Terminal::new(10, 4);
        t.feed("\x1b[31;1mA\x1b7\x1b[0;32m\x1b[3;5HB\x1b8C");
        assert_eq!(t.grid.cur_row, 0);
        assert_eq!(t.grid.row_text(0), "AC", "the cursor did not come back");
        let st = t.grid.cell(0, 1).style;
        assert_eq!(st.fg, TermColor::Ansi(1), "the pen was not restored");
        assert!(st.bold, "bold was not restored");
        // The excursion still happened where it was told to.
        assert_eq!(t.grid.row_text(2), "    B");
    }

    #[test]
    fn the_ansi_sys_spelling_of_save_restore_agrees_with_the_dec_one() {
        // Two byte sequences, one meaning. A program that uses `CSI s` and not
        // `ESC 7` must behave identically, and they are easy to implement in
        // only one place.
        for (save, restore) in [("\x1b7", "\x1b8"), ("\x1b[s", "\x1b[u")] {
            let mut t = Terminal::new(10, 4);
            t.feed(&format!("A{save}\x1b[3;5HB{restore}C"));
            assert_eq!(t.grid.row_text(0), "AC", "{save}/{restore} did not round-trip");
        }
    }

    #[test]
    fn a_restore_with_nothing_saved_homes_the_cursor() {
        let mut t = Terminal::new(10, 4);
        t.feed("\x1b[3;5H\x1b8X");
        assert_eq!((t.grid.cur_row, t.grid.row_text(0)), (0, "X".to_string()));
    }

    #[test]
    fn escape_intermediates_are_consumed_not_printed() {
        // The leak, named. `ESC ( B` designates ASCII into G0 and ncurses emits
        // it on almost every screen setup; the parser dropped the `ESC (` and
        // printed the `B`. Not implementing it would have been harmless —
        // half-implementing it put a stray letter on screen.
        let mut t = Terminal::new(20, 3);
        t.feed("\x1b(B\x1b)0hello\x1b(0\x1b(Bworld");
        assert_eq!(t.grid.row_text(0), "helloworld");
        assert_eq!(t.grid.cur_col, 10, "a designator byte advanced the cursor");
    }

    #[test]
    fn decaln_fills_the_screen_and_homes() {
        // Rare in the wild; here because the `#` has to be consumed either way,
        // and consuming it while ignoring the `8` would be the same leak.
        let mut t = Terminal::new(4, 3);
        t.feed("junk\x1b#8");
        assert_eq!(t.grid.row_text(0), "EEEE");
        assert_eq!(t.grid.row_text(2), "EEEE");
        assert_eq!((t.grid.cur_row, t.grid.cur_col), (0, 0));
    }

    #[test]
    fn a_wide_glyph_reads_back_at_its_screen_width() {
        // The tail cell must be OCCUPIED (or the next put lands on the glyph's
        // right half) and must not be TEXT (or every consumer of row_text sees
        // a word that is not on screen). Both halves are asserted.
        let mut t = Terminal::new(10, 2);
        t.feed("世界x");
        assert_eq!(t.grid.row_text(0), "世界x");
        assert_eq!(t.grid.cur_col, 5, "the glyphs did not each take two columns");
        assert_eq!(t.grid.cell(0, 1).ch, WIDE_TAIL, "the tail cell was left free");
        // all_lines is scrollback-then-rows, and there is no scrollback here.
        assert_eq!(t.grid.all_lines()[0], "世界x", "all_lines disagrees with row_text");
    }

    #[test]
    fn a_line_feed_at_the_regions_foot_scrolls_only_the_region() {
        // The property, stated as what must NOT move: rows outside the band.
        // A test that only checked the band would pass on a full-screen scroll,
        // which is the bug.
        let mut t = Terminal::new(6, 5);
        t.feed("a\r\nb\r\nc\r\nd\r\ne");
        t.feed("\x1b[2;4r");                 // region = rows 1..3
        t.feed("\x1b[4;1H\n");              // cursor at the foot, then LF
        assert_eq!(t.grid.row_text(0), "a", "the row above the region moved");
        assert_eq!(t.grid.row_text(4), "e", "the row below the region moved");
        assert_eq!(t.grid.row_text(1), "c");
        assert_eq!(t.grid.row_text(2), "d");
        assert_eq!(t.grid.row_text(3), "", "the freed row was not blanked");
        assert_eq!(t.grid.cur_row, 3, "the cursor left the region's foot");
    }

    #[test]
    fn a_regional_scroll_does_not_enter_scrollback() {
        // A program redrawing a window over its own content is not producing
        // history. Both directions: a full-screen scroll still must.
        let mut t = Terminal::new(6, 4);
        t.feed("\x1b[1;3r\x1b[3;1H\n\n\n");
        assert!(t.grid.scrollback.is_empty(), "a windowed redraw polluted the scrollback");
        let mut t = Terminal::new(6, 4);
        t.feed("a\r\nb\r\nc\r\nd\n");
        assert_eq!(t.grid.scrollback.len(), 1, "a full-screen scroll must still save the row");
    }

    #[test]
    fn reverse_index_scrolls_down_only_at_the_regions_top() {
        let mut t = Terminal::new(6, 5);
        t.feed("a\r\nb\r\nc\r\nd\r\ne\x1b[2;4r");
        t.feed("\x1b[3;1H\x1bM");            // inside the region: just move up
        assert_eq!((t.grid.cur_row, t.grid.row_text(1)), (1, "b".to_string()));
        t.feed("\x1bM");                      // now AT the top: scroll down
        assert_eq!(t.grid.cur_row, 1, "RI at the top must not leave the region");
        assert_eq!(t.grid.row_text(1), "", "the top of the region was not blanked");
        assert_eq!(t.grid.row_text(2), "b");
        assert_eq!(t.grid.row_text(0), "a", "RI moved a row outside the region");
        assert_eq!(t.grid.row_text(4), "e", "RI moved a row outside the region");
    }

    #[test]
    fn insert_and_delete_line_are_bounded_by_the_region() {
        let mut t = Terminal::new(6, 5);
        t.feed("a\r\nb\r\nc\r\nd\r\ne\x1b[2;4r\x1b[2;1H\x1b[L");
        assert_eq!(t.grid.row_text(1), "");
        assert_eq!(t.grid.row_text(2), "b");
        assert_eq!(t.grid.row_text(3), "c");
        assert_eq!(t.grid.row_text(4), "e", "the row below the region was pushed");

        // A cursor OUTSIDE the region must not shift it — otherwise a stray IL
        // from a program that has moved above its own region drags the status
        // line it was protecting.
        let mut t = Terminal::new(6, 5);
        t.feed("a\r\nb\r\nc\r\nd\r\ne\x1b[2;4r\x1b[1;1H\x1b[L");
        for (r, want) in [(0, "a"), (1, "b"), (2, "c"), (3, "d"), (4, "e")] {
            assert_eq!(t.grid.row_text(r), want, "row {r} moved on an out-of-region IL");
        }
    }

    #[test]
    fn an_inverted_or_empty_region_restores_the_whole_screen() {
        // `CSI r` with no parameters is how a program undoes a region, and a
        // degenerate one must not leave the screen unable to scroll at all.
        let mut t = Terminal::new(6, 5);
        t.feed("\x1b[2;4r");
        assert_eq!((t.grid.scroll_top, t.grid.scroll_bot), (1, 3));
        t.feed("\x1b[r");
        assert_eq!((t.grid.scroll_top, t.grid.scroll_bot), (0, 4));
        t.feed("\x1b[4;2r");                  // inverted
        assert_eq!((t.grid.scroll_top, t.grid.scroll_bot), (0, 4), "an inverted region stuck");
        t.feed("\x1b[3;3r");                  // single row = empty band
        assert_eq!((t.grid.scroll_top, t.grid.scroll_bot), (0, 4));
    }

    #[test]
    fn a_resize_clears_a_region_that_would_outlive_its_rows() {
        // A scroll_bot past the new last row makes line_feed compare against a
        // row that does not exist, and the screen stops scrolling entirely.
        let mut t = Terminal::new(6, 10);
        t.feed("\x1b[3;9r");
        t.resize(6, 4);
        assert_eq!((t.grid.scroll_top, t.grid.scroll_bot), (0, 3));
        t.feed("\x1b[4;1Hx\n\ny");
        assert!(t.grid.scrollback.len() >= 1, "the screen stopped scrolling after a resize");
    }

    #[test]
    fn extended_sgr_sets_the_colour_it_names() {
        let mut t = Terminal::new(10, 2);
        t.feed("\x1b[38;5;33mA\x1b[48;5;236mB\x1b[38;2;255;128;0mC");
        assert_eq!(t.grid.cell(0, 0).style.fg, TermColor::Ansi(33));
        assert_eq!(t.grid.cell(0, 1).style.bg, TermColor::Ansi(236));
        assert_eq!(t.grid.cell(0, 2).style.fg, TermColor::Rgb(255, 128, 0));
        // The background set two escapes ago is still there: an extended form
        // must consume its own arguments and touch nothing else.
        assert_eq!(t.grid.cell(0, 2).style.bg, TermColor::Ansi(236));
    }

    #[test]
    fn an_extended_colour_does_not_reset_the_pen() {
        // The actual reported damage. `38;2;255;128;0` used to fall through to
        // the plain table and end on a `0`, which is SGR RESET — so a
        // truecolour escape cleared bold, inverse and every colour the prompt
        // had set. Asserting the new colour alone would not have caught it.
        let mut t = Terminal::new(10, 2);
        t.feed("\x1b[1;7;41m\x1b[38;2;10;20;30mX");
        let st = t.grid.cell(0, 0).style;
        assert!(st.bold, "bold was cleared by a truecolour escape");
        assert!(st.inverse, "inverse was cleared by a truecolour escape");
        assert_eq!(st.bg, TermColor::Ansi(1), "background was cleared");
        assert_eq!(st.fg, TermColor::Rgb(10, 20, 30));
    }

    #[test]
    fn attributes_after_an_extended_colour_still_apply() {
        // The complement of consuming arguments: consuming too MANY would eat
        // the parameters that follow, which is the opposite failure and just
        // as silent.
        let mut t = Terminal::new(10, 2);
        t.feed("\x1b[38;5;196;1;4mX");
        assert_eq!(t.grid.cell(0, 0).style.fg, TermColor::Ansi(196));
        assert!(t.grid.cell(0, 0).style.bold, "the `1` after the colour was swallowed");
        let mut t = Terminal::new(10, 2);
        t.feed("\x1b[38;2;1;2;3;7mX");
        assert!(t.grid.cell(0, 0).style.inverse, "the `7` after a truecolour was swallowed");
    }

    #[test]
    fn a_truncated_extended_colour_is_dropped_not_half_applied() {
        // `38;5` with no index, and `38;2;1;2` with no blue. Neither says what
        // colour was wanted, and the old code's answer was to interpret the
        // leftovers as separate SGR codes — which is how a reset appeared out
        // of a colour change.
        for seq in ["\x1b[1m\x1b[38;5mX", "\x1b[1m\x1b[38;2;1;2mX"] {
            let mut t = Terminal::new(10, 2);
            t.feed(seq);
            let st = t.grid.cell(0, 0).style;
            assert!(st.bold, "{seq:?} disturbed the pen");
            assert_eq!(st.fg, TermColor::Default, "{seq:?} invented a colour");
        }
    }

    #[test]
    fn an_unknown_extended_selector_takes_its_selector_with_it() {
        // `38` followed by a form this cannot honour (the ISO `0`-`4` variants).
        // The selector must be skipped ALONG WITH the 38, or it is read as a
        // plain SGR code — and the selectors that collide are the worst two:
        // `0` is RESET and `1` is bold. `38;0m` clearing the pen is the same
        // damage the truecolour bug did, from the other direction.
        //
        // This case was untested until breaking it changed nothing, which is
        // the only way that would have shown (CLAUDE.md §5b).
        let mut t = Terminal::new(10, 2);
        t.feed("\x1b[1;7m\x1b[38;0mX");
        let st = t.grid.cell(0, 0).style;
        assert!(st.bold && st.inverse, "an unknown selector's `0` reset the pen");

        let mut t = Terminal::new(10, 2);
        t.feed("\x1b[38;1mX");
        assert!(!t.grid.cell(0, 0).style.bold, "an unknown selector's `1` was read as bold");
    }

    #[test]
    fn the_palette_covers_all_256_entries_distinctly() {
        // `Ansi` now means 0..=255, so the resolver must answer for the cube
        // and the grey ramp too — not fall into a catch-all that paints every
        // 256-colour prompt white.
        assert_eq!(ansi_color(16), egui::Color32::from_rgb(0, 0, 0));
        assert_eq!(ansi_color(231), egui::Color32::from_rgb(255, 255, 255));
        assert_eq!(ansi_color(33), egui::Color32::from_rgb(0, 0x87, 0xff));
        assert_eq!(ansi_color(232), egui::Color32::from_rgb(8, 8, 8));
        assert_eq!(ansi_color(255), egui::Color32::from_rgb(238, 238, 238));
        // A floor on distinctness: a resolver that collapsed the cube would
        // still pass the spot checks above if they happened to be its constant.
        let n = (0u16..=255).map(|i| ansi_color(i as u8).to_array()).collect::<std::collections::BTreeSet<_>>().len();
        assert!(n > 230, "only {n} distinct colours in a 256-entry palette");
    }

    #[test]
    fn absolute_column_and_row_addressing() {
        // CHA and VPA are what readline redraws an edited line with. The two
        // spellings of CHA ('G' and the older '`') must agree, or a program
        // using the one that is missing redraws into the wrong column.
        let mut t = Terminal::new(10, 4);
        t.feed("abcdefgh\x1b[3G_");
        assert_eq!(t.grid.row_text(0), "ab_defgh");
        t.feed("\x1b[1`X");
        assert_eq!(t.grid.row_text(0), "Xb_defgh");
        t.feed("\x1b[3dQ");
        assert_eq!(t.grid.cur_row, 2, "VPA moves the row");
        assert_eq!(t.grid.row_text(2), " Q", "and preserves the column");
    }

    #[test]
    fn erase_chars_clears_exactly_n_and_shifts_nothing() {
        // The property is "n cells, in place". Asserting only that the row
        // changed would pass for erase-to-end, which is the bug ECH exists to
        // avoid — so the tail is asserted too, in both directions.
        let mut t = Terminal::new(10, 2);
        t.feed("abcdefgh\x1b[1;3H\x1b[2X");
        assert_eq!(t.grid.row_text(0), "ab  efgh");
        assert_eq!(t.grid.cur_col, 2, "ECH does not move the cursor");

        // n past the end of the row clamps rather than spilling into the next.
        let mut t = Terminal::new(4, 2);
        t.feed("abcd\x1b[2;1Hz\x1b[1;3H\x1b[99X");
        assert_eq!(t.grid.row_text(0), "ab");
        assert_eq!(t.grid.row_text(1), "z", "the next row is untouched");
    }

    #[test]
    fn insert_and_delete_chars_shift_within_the_row_only() {
        let mut t = Terminal::new(8, 2);
        t.feed("abcdef\x1b[2;1Hz\x1b[1;3H\x1b[2@");
        assert_eq!(t.grid.row_text(0), "ab  cdef");
        assert_eq!(t.grid.row_text(1), "z", "insert does not wrap into row 1");

        // What insert pushes past the last column is lost, not wrapped.
        let mut t = Terminal::new(6, 2);
        t.feed("abcdef\x1b[1;1H\x1b[2@");
        assert_eq!(t.grid.row_text(0), "  abcd");

        let mut t = Terminal::new(8, 2);
        t.feed("abcdef\x1b[1;3H\x1b[2P");
        assert_eq!(t.grid.row_text(0), "abef");
        assert_eq!(t.grid.cur_col, 2, "DCH does not move the cursor");
    }

    #[test]
    fn character_ops_at_the_last_column_are_bounded() {
        // Every one of these is an index computation on a row, and the last
        // column is where an off-by-one becomes a write into the next row.
        for seq in ["\x1b[3X", "\x1b[3@", "\x1b[3P"] {
            let mut t = Terminal::new(4, 2);
            t.feed("ab\x1b[2;1Hkeep\x1b[1;4H");
            t.feed(seq);
            assert_eq!(t.grid.row_text(1), "keep", "{seq} reached into row 1");
            assert_eq!(t.grid.cur_row, 0, "{seq} moved the cursor");
        }
    }

    #[test]
    fn cnl_and_cpl_move_to_column_zero() {
        // The whole difference between CNL/CPL and CUD/CUU is the column, so
        // that is the thing to assert.
        let mut t = Terminal::new(10, 5);
        t.feed("abcde\x1b[2EX");
        assert_eq!((t.grid.cur_row, t.grid.row_text(2)), (2, "X".to_string()));
        t.feed("\x1b[1FY");
        assert_eq!((t.grid.cur_row, t.grid.row_text(1)), (1, "Y".to_string()));
    }

    #[test]
    fn backspace_overwrites() {
        let mut t = term();
        t.feed("ab\x08c");
        assert_eq!(t.grid.row_text(0), "ac");
    }

    #[test]
    fn cursor_position_csi() {
        let mut t = term();
        t.feed("\x1b[2;3Hx"); // row 2, col 3 (1-based) → (1,2) 0-based
        assert_eq!(t.grid.cell(1, 2).ch, 'x');
    }

    #[test]
    fn sgr_sets_colour_and_resets() {
        let mut t = term();
        t.feed("\x1b[1;31mR\x1b[0mn");
        let r = t.grid.cell(0, 0);
        assert_eq!(r.style.fg, TermColor::Ansi(1));
        assert!(r.style.bold);
        let n = t.grid.cell(0, 1);
        assert_eq!(n.style.fg, TermColor::Default);
        assert!(!n.style.bold);
    }

    #[test]
    fn erase_display_clears() {
        let mut t = term();
        t.feed("abc\x1b[2J");
        assert_eq!(t.grid.row_text(0), "");
    }

    #[test]
    fn erase_line_to_end() {
        let mut t = term();
        t.feed("abcdef");
        t.feed("\x1b[3G"); // (not supported → ignored); use CUP instead
        t.feed("\r"); // cursor to col 0
        t.feed("\x1b[0K"); // erase to end of line
        assert_eq!(t.grid.row_text(0), "");
    }

    #[test]
    fn scroll_pushes_to_scrollback() {
        let mut t = Terminal::new(8, 2);
        t.feed("one\r\ntwo\r\nthree");
        // "one" scrolled off the top
        assert_eq!(t.grid.scrollback.len(), 1);
        assert_eq!(t.grid.scrollback[0].iter().map(|c| c.ch).collect::<String>().trim_end(), "one");
        assert_eq!(t.grid.row_text(0), "two");
        assert_eq!(t.grid.row_text(1), "three");
    }

    #[test]
    fn stage_sets_input_without_running() {
        let mut t = term();
        t.stage("rustup target add aarch64-unknown-linux-musl");
        assert_eq!(t.input, "rustup target add aarch64-unknown-linux-musl");
        // nothing was written to the grid
        assert_eq!(t.grid.row_text(0), "");
    }

    #[test]
    fn resize_preserves_top_left() {
        let mut t = Terminal::new(10, 3);
        t.feed("hi");
        t.resize(6, 2);
        assert_eq!(t.grid.row_text(0), "hi");
        assert_eq!((t.grid.cols, t.grid.rows), (6, 2));
    }

    #[test]
    fn ansi_colors_are_distinct_and_red_is_reddish() {
        let red = ansi_color(1);
        assert!(red.r() > red.g() && red.r() > red.b());
        assert_ne!(ansi_color(2), ansi_color(4)); // green vs blue differ
        assert_ne!(ansi_color(1), ansi_color(9)); // normal vs bright red differ
    }

    #[test]
    fn resolve_colors_handles_default_and_inverse() {
        let colors = uikit_egui::PaletteColors::from_palette(snapclone_theme_palette());
        let plain = Style::default();
        let (fg, bg) = resolve_colors(plain, &colors);
        assert_eq!(fg, colors.text);
        assert_eq!(bg, colors.bg_base);
        // inverse swaps them
        let inv = Style { inverse: true, ..Style::default() };
        let (ifg, ibg) = resolve_colors(inv, &colors);
        assert_eq!(ifg, bg);
        assert_eq!(ibg, fg);
    }

    #[test]
    fn app_applies_text_scale_to_cell() {
        let ctx = egui::Context::default();
        let mut app = App::new(20, 5);
        let base = app.font_size;
        let mut prefs = settings_kit::Prefs::default();
        prefs.touch.text_scale = 2.0;
        app.apply_settings(&ctx, &prefs);
        assert!(app.font_size > base, "text scale grows the monospace cell");
    }

    /// The bug: the catch-all key arm in `handle_input` (`egui::Event::Key { key: k, .. }`, no
    /// guard) matches every key press, including Ctrl+F, before the match ever reaches the
    /// specific `egui::Key::F ... if modifiers.command || modifiers.ctrl` arm below it — which
    /// is therefore unreachable (confirmed by the compiler's own `unreachable_patterns`
    /// warning). The doc comment on `open_search` says "Ctrl+F" is the intended shortcut; what
    /// actually happens today is the catch-all treats Ctrl+F as a control-code chord and queues
    /// a raw 0x06 (ACK) byte to send to the shell instead of opening the find bar.
    #[test]
    fn ctrl_f_opens_search_instead_of_leaking_a_control_byte_to_the_shell() {
        let ctx = egui::Context::default();
        let mut app = App::new(80, 24);
        assert!(!app.search_open, "fixture must start with search closed to be meaningful");

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let events = vec![egui::Event::Key {
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { ctrl: true, ..Default::default() },
        }];
        let _ = ctx.run(
            egui::RawInput { events, screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.handle_input(ui);
                });
            },
        );

        assert!(app.search_open, "Ctrl+F must open the find bar (see open_search's doc comment)");
        assert!(app.take_pending_bytes().is_empty(),
            "Ctrl+F must not also queue bytes to send to the shell — the catch-all key arm \
             matching first is exactly the bug this guards against");
    }

    /// The bug: `voiceterm-egui` binds Ctrl+P to open its command palette
    /// (voiceterm_egui.rs, "the eleven-tab switcher's replacement"), an app-level
    /// concern this crate has no knowledge of — but the SAME catch-all arm that
    /// swallowed Ctrl+F also swallows Ctrl+P, sending its control byte (0x10, DLE)
    /// to the shell. Bash/readline binds Ctrl+P to "previous history entry" (the
    /// same action as the already-mapped ArrowUp key, so nothing unique is lost by
    /// reserving it) — meaning every app-level Ctrl+P currently ALSO pollutes the
    /// PTY with a spurious history-navigation byte the user never asked for, on
    /// top of whatever else is going on with the shortcut on real hardware
    /// (see maintenance/C11_TODO.md's C11-6c for the on-device "dead" report this
    /// doesn't fully explain by itself, but which this collateral leak makes
    /// worse either way).
    #[test]
    fn ctrl_p_is_reserved_and_does_not_leak_a_control_byte_to_the_shell() {
        let ctx = egui::Context::default();
        let mut app = App::new(80, 24);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let events = vec![egui::Event::Key {
            key: egui::Key::P,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { ctrl: true, ..Default::default() },
        }];
        let _ = ctx.run(
            egui::RawInput { events, screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.handle_input(ui);
                });
            },
        );

        assert!(app.take_pending_bytes().is_empty(),
            "Ctrl+P is reserved for the host app's command palette and must not also \
             queue a control byte to send to the shell");
    }

    fn snapclone_theme_palette() -> &'static snapclone_theme::Palette {
        snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0])
    }
}


#[cfg(test)]
mod private_mode_tests {
    use super::*;
    use crate::keys::{encode_mode, Key, Mods};

    fn feed(seq: &str) -> Grid {
        let mut g = Grid::new(80, 24);
        let mut p = Parser::new();
        p.feed(seq, &mut g);
        g
    }

    #[test]
    fn the_question_mark_is_no_longer_dropped_as_an_intermediate_byte() {
        // It used to be: `CSI ? 1 h` dispatched as a plain `CSI 1 h`, so DECCKM
        // never arrived and the encoder's application-cursor branch — ported
        // specifically to fix arrow navigation inside fzf and vim — could never
        // be reached.
        let g = feed("\x1b[?1h");
        assert!(g.app_cursor);
    }

    #[test]
    fn decckm_reaches_the_key_encoder_which_is_the_whole_point() {
        // The fix shipped in keys.rs was wired to nothing until now. This is
        // the end-to-end path: program sets the mode, arrow changes shape.
        let plain = feed("");
        assert_eq!(encode_mode(Key::Up, Mods::none(), plain.key_mode()).unwrap(), b"\x1b[A");

        let app = feed("\x1b[?1h");
        assert_eq!(encode_mode(Key::Up, Mods::none(), app.key_mode()).unwrap(), b"\x1bOA");
    }

    #[test]
    fn a_program_that_exits_restores_the_normal_arrows() {
        // fzf sets DECCKM on entry and resets it on exit. A terminal that kept
        // it would leave every later arrow key sending SS3 to a shell that does
        // not expect it.
        let mut g = Grid::new(80, 24);
        let mut p = Parser::new();
        p.feed("\x1b[?1h", &mut g);
        assert!(g.app_cursor);
        p.feed("\x1b[?1l", &mut g);
        assert!(!g.app_cursor);
        assert_eq!(encode_mode(Key::Up, Mods::none(), g.key_mode()).unwrap(), b"\x1b[A");
    }

    #[test]
    fn bracketed_paste_is_tracked_too() {
        let g = feed("\x1b[?2004h");
        assert!(g.bracketed_paste);
        assert_eq!(crate::keys::paste("hi", g.key_mode()), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn several_modes_in_one_sequence_all_apply() {
        // `CSI ? 1 ; 2004 h` is legal and programs do send it.
        let g = feed("\x1b[?1;2004h");
        assert!(g.app_cursor && g.bracketed_paste);
    }

    #[test]
    fn an_unhandled_private_mode_does_nothing_rather_than_the_wrong_thing() {
        // `CSI ? 25 l` (hide cursor) used to arrive as `CSI 25 l` and be taken
        // for an ANSI mode reset. Silently doing the wrong thing is worse than
        // doing nothing, because nothing is debuggable.
        let g = feed("\x1b[?25l");
        assert!(!g.app_cursor && !g.bracketed_paste);
    }

    #[test]
    fn a_private_sequence_does_not_leak_into_the_next_one() {
        // The flag has to reset at the next CSI, or an ordinary `CSI 2 J`
        // following a private sequence would be swallowed as a private mode.
        let mut g = Grid::new(80, 24);
        let mut p = Parser::new();
        p.feed("\x1b[?1h", &mut g);
        p.feed("Hello", &mut g);
        p.feed("\x1b[2J", &mut g);      // clear, an ORDINARY csi
        assert!(g.app_cursor, "the mode survives");
        assert!(g.row_text(0).trim().is_empty(), "and the clear actually cleared");
    }

    #[test]
    fn one_accessor_hands_over_both_modes() {
        // So a caller cannot read one and forget the other — which is how a
        // terminal ends up with correct arrows and broken paste.
        let g = feed("\x1b[?1h\x1b[?2004h");
        let m = g.key_mode();
        assert!(m.app_cursor && m.bracketed_paste);
    }
}

#[cfg(test)]
mod input_wiring_tests {
    use super::*;

    /// Feed a DEC private sequence straight into an App's grid, as a program
    /// running in the terminal would.
    fn app_with(seq: &str) -> App {
        let mut a = App::new(80, 24);
        a.terminal.feed(seq);
        a
    }

    #[test]
    fn the_encoded_arrow_follows_the_mode_the_program_set() {
        // The chain this completes: keys::encode_mode gained the SS3 branch,
        // then the parser learned DECCKM, and handle_input was still calling
        // the mode-unaware encoder. Each piece was tested in isolation and the
        // path was broken end to end.
        let plain = App::new(80, 24);
        assert_eq!(keys::encode_mode(keys::Key::Up, keys::Mods::none(),
                                     plain.terminal.grid.key_mode()).unwrap(),
                   b"\x1b[A");

        let app = app_with("\x1b[?1h");
        assert_eq!(keys::encode_mode(keys::Key::Up, keys::Mods::none(),
                                     app.terminal.grid.key_mode()).unwrap(),
                   b"\x1bOA");
    }

    #[test]
    fn a_paste_is_bracketed_only_when_the_program_asked_for_it() {
        // Pasting a block into vim without brackets runs every line through
        // its keymap — the classic wall of autoindent.
        let plain = App::new(80, 24);
        assert_eq!(keys::paste("a\nb", plain.terminal.grid.key_mode()), b"a\rb");

        let app = app_with("\x1b[?2004h");
        let out = keys::paste("a\nb", app.terminal.grid.key_mode());
        assert!(out.starts_with(b"\x1b[200~") && out.ends_with(b"\x1b[201~"));
    }

    #[test]
    fn pending_bytes_is_the_pty_path_and_starts_empty() {
        // Documented here because nothing in the tree drains it yet — see
        // PORT_STATUS. The bytes are computed correctly and discarded, which is
        // why control keys appear to do nothing.
        let mut a = App::new(80, 24);
        assert!(a.take_pending_bytes().is_empty());
    }
}

#[cfg(test)]
mod wide_char_tests {
    use super::*;

    #[test]
    fn a_cjk_glyph_takes_two_columns() {
        // Advancing by one leaves the next character painted over its right
        // half, which is the classic garbled-CJK terminal.
        let mut g = Grid::new(10, 2);
        g.put('\u{4E2D}');
        assert_eq!(g.cur_col, 2);
    }

    #[test]
    fn a_combining_mark_takes_none() {
        // One column per accent turns an accented word into a staircase.
        let mut g = Grid::new(10, 2);
        g.put('e');
        let after_base = g.cur_col;
        g.put('\u{0301}');   // combining acute
        assert_eq!(g.cur_col, after_base, "the accent consumed a column");
    }

    #[test]
    fn an_ascii_char_still_takes_one() {
        // The common path must not change.
        let mut g = Grid::new(10, 2);
        g.put('a');
        assert_eq!(g.cur_col, 1);
        assert_eq!(Grid::char_width('a'), 1);
    }

    #[test]
    fn a_wide_glyph_does_not_start_in_the_last_column() {
        // Half of it would land on the next row.
        let mut g = Grid::new(4, 3);
        g.put('a'); g.put('b'); g.put('c');   // cols 0,1,2 — one left
        let row_before = g.cur_row;
        g.put('\u{4E2D}');
        assert_eq!(g.cur_row, row_before + 1, "the wide glyph was split across rows");
        assert_eq!(g.cur_col, 2);
    }

    #[test]
    fn the_phantom_column_is_occupied() {
        // Without it the next put lands on top of the wide glyph's right half.
        let mut g = Grid::new(10, 2);
        g.put('\u{4E2D}');
        g.put('x');
        let text = g.row_text(0);
        assert!(text.starts_with('\u{4E2D}'), "{text:?}");
        assert!(text.contains('x'));
    }

    #[test]
    fn emoji_are_wide() {
        assert_eq!(Grid::char_width('\u{1F600}'), 2);
        assert_eq!(Grid::char_width('\u{1F9F0}'), 2);
    }

#[cfg(test)]
mod query_reply_tests {
    use super::*;

    fn replies_for(input: &str) -> Vec<u8> {
        let mut t = Terminal::new(80, 24);
        t.feed(input);
        t.take_replies()
    }

    /// THE HELIX DELAY. crossterm sends `CSI c` at start-up and blocks until
    /// the terminal answers. The answer is Qt's, byte for byte.
    #[test]
    fn device_attributes_are_answered() {
        assert_eq!(replies_for("\x1b[c"), b"\x1b[?1;2c");
        assert_eq!(replies_for("\x1b[0c"), b"\x1b[?1;2c", "the explicit-zero form too");
    }

    /// crossterm's exact start-up pair: `CSI ? u` then `CSI c`. The keyboard
    /// query must stay UNANSWERED -- silence means "no kitty protocol", which
    /// is true -- and only the DA1 marker comes back.
    #[test]
    fn crosstermss_startup_probe_gets_exactly_the_da1_reply() {
        assert_eq!(replies_for("\x1b[?u\x1b[c"), b"\x1b[?1;2c");
    }

    /// DA2 differs from DA1 ONLY by the `>` marker. Dropping it as a generic
    /// intermediate answered DA2 with a DA1 reply.
    #[test]
    fn secondary_attributes_get_their_own_reply() {
        assert_eq!(replies_for("\x1b[>c"), b"\x1b[>0;276;0c");
        assert_eq!(replies_for("\x1b[=c"), b"", "DA3 left unanswered, as in Qt");
    }

    /// Where is the cursor? Answered 1-based, as the query expects.
    #[test]
    fn a_cursor_position_query_is_answered_one_based() {
        // Move to row 5, col 10 (1-based), then ask.
        assert_eq!(replies_for("\x1b[5;10H\x1b[6n"), b"\x1b[5;10R");
        assert_eq!(replies_for("\x1b[5n"), b"\x1b[0n", "status: OK");
    }

    /// The marker state must RESET per sequence, or a DA2 would turn the next
    /// plain DA1 into a DA2 too.
    #[test]
    fn markers_do_not_leak_into_the_next_sequence() {
        assert_eq!(replies_for("\x1b[>c\x1b[c"), b"\x1b[>0;276;0c\x1b[?1;2c");
    }

    /// Replies are drained, not accumulated -- a second take is empty.
    #[test]
    fn replies_are_taken_once() {
        let mut t = Terminal::new(80, 24);
        t.feed("\x1b[c");
        assert!(!t.take_replies().is_empty());
        assert!(t.take_replies().is_empty(), "must not be sent twice");
    }

    /// Ordinary output produces no replies at all.
    #[test]
    fn plain_output_sends_nothing_back() {
        assert!(replies_for("hello\r\n\x1b[31mred\x1b[0m\x1b[2J").is_empty());
    }
}
}
