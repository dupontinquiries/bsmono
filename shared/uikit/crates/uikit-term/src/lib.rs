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

// ── colours + style ──────────────────────────────────────────────────────────

/// A terminal colour: the theme default, or an ANSI palette index (0–7 normal, 8–15 bright).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermColor {
    Default,
    Ansi(u8),
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
}

impl Grid {
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
        }
    }

    fn idx(&self, r: usize, c: usize) -> usize {
        r * self.cols + c
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
        let s: String = self.cells[start..start + self.cols].iter().map(|c| c.ch).collect();
        s.trim_end().to_string()
    }

    /// Write a printable char at the cursor and advance (wrapping + scrolling as needed).
    pub fn put(&mut self, ch: char) {
        if self.cur_col >= self.cols {
            self.cur_col = 0;
            self.line_feed();
        }
        let i = self.idx(self.cur_row, self.cur_col);
        self.cells[i] = Cell { ch, style: self.pen };
        self.cur_col += 1;
    }

    /// Move down one line, scrolling if at the bottom.
    pub fn line_feed(&mut self) {
        if self.cur_row + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cur_row += 1;
        }
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

    /// Scroll the whole screen up one line; the top row goes to scrollback.
    pub fn scroll_up(&mut self) {
        let top: Vec<Cell> = self.cells[0..self.cols].to_vec();
        self.scrollback.push(top);
        if self.scrollback.len() > self.scrollback_cap {
            self.scrollback.remove(0);
        }
        self.cells.copy_within(self.cols.., 0);
        let last = self.cells.len() - self.cols;
        for c in &mut self.cells[last..] {
            *c = Cell::default();
        }
    }

    /// Clear the visible cells and home the cursor (scrollback is preserved).
    pub fn clear(&mut self) {
        for c in &mut self.cells {
            *c = Cell::default();
        }
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
    }

    /// Erase in display: 0 = cursor→end, 1 = start→cursor, 2 = whole screen.
    pub fn erase_in_display(&mut self, mode: u16) {
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

    /// Resize the grid, preserving the top-left content.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut next = vec![Cell::default(); cols * rows];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                next[r * cols + c] = self.cell(r, c);
            }
        }
        self.cells = next;
        self.cols = cols;
        self.rows = rows;
        self.cur_row = self.cur_row.min(rows - 1);
        self.cur_col = self.cur_col.min(cols - 1);
    }
}

// ── the VT/ANSI parser ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Csi,
}

/// A byte/char-stream VT parser that drives a [`Grid`]. Feed it decoded text (UTF-8); it
/// applies printable characters, C0 controls, and CSI sequences.
pub struct Parser {
    state: State,
    params: Vec<u16>,
    cur: Option<u16>,
}

impl Default for Parser {
    fn default() -> Self {
        Parser { state: State::Ground, params: Vec::new(), cur: None }
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
                    self.cur = None;
                    self.state = State::Csi;
                }
                _ => self.state = State::Ground, // ignore non-CSI escapes for v1
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
                    '@'..='~' => {
                        let params = self.take_params();
                        self.dispatch_csi(ch, &params, grid);
                        self.state = State::Ground;
                    }
                    _ => { /* intermediate bytes ignored */ }
                }
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
            'J' => grid.erase_in_display(params.first().copied().unwrap_or(0)),
            'K' => grid.erase_in_line(params.first().copied().unwrap_or(0)),
            'm' => self.apply_sgr(params, grid),
            _ => {} // unsupported CSI: ignore
        }
    }

    fn apply_sgr(&self, params: &[u16], grid: &mut Grid) {
        let ps = if params.is_empty() { &[0u16][..] } else { params };
        for &p in ps {
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
        _ => C::from_rgb(0xff, 0xff, 0xff),
    }
}

/// Resolve a cell's `(foreground, background)` egui colours, honouring inverse video and the
/// theme default (default fg = the palette text colour, default bg = the base background).
pub fn resolve_colors(style: Style, colors: &uikit_egui::PaletteColors) -> (egui::Color32, egui::Color32) {
    let fg = match style.fg {
        TermColor::Default => colors.text,
        TermColor::Ansi(i) => ansi_color(i),
    };
    let bg = match style.bg {
        TermColor::Default => colors.bg_base,
        TermColor::Ansi(i) => ansi_color(i),
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
pub fn render_grid(
    ui: &egui::Ui,
    grid: &Grid,
    colors: &uikit_egui::PaletteColors,
    display: &settings_kit::DisplayPrefs,
    origin: egui::Pos2,
    cell_w: f32,
    cell_h: f32,
    font_size: f32,
) {
    let painter = ui.painter();
    let filt = |c: egui::Color32| uikit_egui::filter_color(c, display);
    let font = egui::FontId::monospace(font_size);

    for r in 0..grid.rows {
        let y = origin.y + r as f32 * cell_h;
        let mut c = 0;
        while c < grid.cols {
            let start = grid.cell(r, c);
            // extend a run of identical style
            let mut end = c + 1;
            while end < grid.cols && grid.cell(r, end).style == start.style {
                end += 1;
            }
            let text: String = (c..end).map(|cc| grid.cell(r, cc).ch).collect();
            let (fg, bg) = resolve_colors(start.style, colors);
            let x0 = origin.x + c as f32 * cell_w;
            let rect = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2((end - c) as f32 * cell_w, cell_h));
            // paint background only when it differs from the base
            if start.style.bg != TermColor::Default || start.style.inverse {
                painter.rect_filled(rect, 0.0, filt(bg));
            }
            if !text.trim().is_empty() {
                painter.text(egui::pos2(x0, y), egui::Align2::LEFT_TOP, text, font.clone(), filt(fg));
            }
            c = end;
        }
    }
    // cursor block
    let cx = origin.x + grid.cur_col as f32 * cell_w;
    let cy = origin.y + grid.cur_row as f32 * cell_h;
    painter.rect_filled(
        egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cell_w, cell_h)),
        0.0,
        filt(colors.accent).gamma_multiply(0.6),
    );
}

/// A minimal voiceterm view: the terminal, current prefs, and settings adoption. The egui
/// `main` connects a PTY to `terminal.feed_bytes` and calls `render` each frame; the voice and
/// mail panes attach here later.
pub struct App {
    pub terminal: Terminal,
    pub prefs: settings_kit::Prefs,
    pub cell: (f32, f32),
    pub font_size: f32,
    /// Multitouch input (pinch-to-zoom the terminal, swipe, fling) via the shared kit bridge.
    pub touch: uikit_egui::touch::TouchSource,
    pub recognizer: uikit_egui::touch::GestureRecognizer,
}

impl App {
    pub fn new(cols: usize, rows: usize) -> App {
        App {
            terminal: Terminal::new(cols, rows),
            prefs: settings_kit::Prefs::default(),
            cell: (8.5, 17.0),
            font_size: 13.0,
            touch: uikit_egui::touch::TouchSource::new(),
            recognizer: uikit_egui::touch::GestureRecognizer::new(uikit_egui::touch::Config::default()),
        }
    }

    /// Adopt cross-app settings (theme/text/button + gesture feel handled by the kit).
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        self.prefs = prefs.clone();
        uikit_egui::apply_prefs(ctx, prefs);
        // scale the monospace cell to the text-size preference
        self.font_size = 13.0 * prefs.touch.text_scale.max(0.5);
        self.cell = (self.font_size * 0.62, self.font_size * 1.3);
    }

    /// Apply a pinch scale factor to the terminal font (pure; clamped to a readable range).
    pub fn apply_pinch(&mut self, rel: f32) {
        let f = (self.font_size * rel).clamp(7.0, 48.0);
        self.font_size = f;
        self.cell = (f * 0.62, f * 1.3);
    }

    /// Poll multitouch for this frame and apply it: pinch zooms the terminal font. Returns whether
    /// fling momentum is still gliding, so the caller can `ctx.request_repaint()`. Graceful — no
    /// touch input means no gestures and a no-op.
    pub fn handle_gestures(&mut self, ctx: &egui::Context) -> bool {
        let (gestures, gliding) = self.touch.poll(ctx, &mut self.recognizer);
        for g in gestures {
            if let uikit_egui::touch::Gesture::Pinch { rel, .. } = g {
                self.apply_pinch(rel);
            }
        }
        gliding
    }

    /// Draw the terminal into the current UI region.
    pub fn render(&self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        let origin = ui.min_rect().min;
        render_grid(ui, &self.terminal.grid, colors, &self.prefs.display, origin, self.cell.0, self.cell.1, self.font_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn snapclone_theme_palette() -> &'static snapclone_theme::Palette {
        snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0])
    }
}

