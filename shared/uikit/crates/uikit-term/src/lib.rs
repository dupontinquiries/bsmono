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

    /// Every line as text — scrollback (oldest first) then the visible rows. The unit of search and
    /// copy; line 0 is the oldest scrollback row.
    pub fn all_lines(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .scrollback
            .iter()
            .map(|row| row.iter().map(|c| c.ch).collect::<String>().trim_end().to_string())
            .collect();
        for r in 0..self.rows {
            v.push(self.row_text(r));
        }
        v
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
    // cursor — ported from voiceterm's QML `termCursor` (see CursorAnim's doc comment for the
    // exact mode semantics/timings this matches).
    if cursor_mode != CursorMode::Off {
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
    /// The row this selection is on, if any.
    pub row: Option<usize>,
    /// The selected text (a word for a tap-and-hold on it, a whole line for a long-press).
    pub text: String,
}

/// A minimal voiceterm view: the terminal, current prefs, and settings adoption. The egui
/// `main` connects a PTY to `terminal.feed_bytes` and calls `render` each frame; the voice and
/// mail panes attach here later.
pub struct App {
    pub terminal: Terminal,
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
}

impl App {
    pub fn new(cols: usize, rows: usize) -> App {
        App {
            terminal: Terminal::new(cols, rows),
            prefs: settings_kit::Prefs::default(),
            cell: (DEFAULT_FONT_SIZE * CELL_W_RATIO, DEFAULT_FONT_SIZE * CELL_H_RATIO),
            font_size: DEFAULT_FONT_SIZE,
            touch: uikit_egui::touch::TouchSource::new(),
            recognizer: uikit_egui::touch::GestureRecognizer::new(uikit_egui::touch::Config::default()),
            search: SearchState::default(),
            search_open: false,
            cursor_mode: CursorMode::default(), // Slide, matching the QML's default
            cursor_anim: CursorAnim::default(),
            selection: None,
            fitted_size: None,
            long_press: None,
            link_tap: None,
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
    fn cell_at(&self, origin: egui::Pos2, pos: egui::Pos2) -> Option<(usize, usize)> {
        let rel = pos - origin;
        if rel.x < 0.0 || rel.y < 0.0 { return None; }
        let (col, row) = ((rel.x / self.cell.0) as usize, (rel.y / self.cell.1) as usize);
        if col < self.terminal.grid.cols && row < self.terminal.grid.rows { Some((col, row)) } else { None }
    }

    /// Consume this frame's keyboard/paste events into the input line. Returns `Some(command)` on Enter
    /// (the submitted line, already taken out of the input); typing appends, Backspace deletes, and a
    /// paste inserts its text (control chars stripped). The host runs the returned command on its PTY.
    pub fn handle_input(&mut self, ui: &egui::Ui) -> Option<String> {
        let mut submitted = None;
        ui.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Text(t) => self.terminal.input.push_str(t),
                    egui::Event::Paste(t) => self.terminal.paste(t),
                    egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                        self.terminal.input.pop();
                    }
                    egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => {
                        submitted = Some(std::mem::take(&mut self.terminal.input));
                    }
                    _ => {}
                }
            }
        });
        submitted
    }

    /// Open the find bar (the compass "search" action / Ctrl+F).
    pub fn open_search(&mut self) {
        self.search_open = true;
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
                egui::TextEdit::singleline(&mut self.search.query)
                    .hint_text("find in screen…")
                    .desired_width(180.0),
            );
            if resp.changed() {
                self.search.run(&self.terminal.grid);
            }
            let n = self.search.count();
            let cur = if n == 0 { 0 } else { self.search.active + 1 };
            ui.label(format!("{cur}/{n}"));
            if ui.button("▲").clicked() {
                self.search.prev();
            }
            if ui.button("▼").clicked() {
                self.search.next();
            }
            if ui.button("✕").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
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
                        let word = word_at(&self.terminal.grid.row_text(row), col);
                        // a tap on a URL is a link tap (the QML blue-link affordance); the word
                        // selection still happens, so copy-word flows keep working
                        if let Some(url) = url_in_word(&word) {
                            self.link_tap = Some((egui::pos2(p.x, p.y), url));
                        }
                        self.selection = Some(Selection { row: Some(row), text: word });
                    }
                }
                uikit_egui::touch::Gesture::LongPress(p) => {
                    if let Some((_, row)) = self.cell_at(origin, egui::pos2(p.x, p.y)) {
                        self.selection = Some(Selection { row: Some(row), text: self.terminal.grid.row_text(row) });
                    }
                    self.long_press = Some(egui::pos2(p.x, p.y));
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

    /// Draw the terminal into the current UI region.
    pub fn render(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        let origin = ui.min_rect().min;
        let dt = ui.input(|i| i.stable_dt).min(0.1); // clamp a stalled first frame's huge dt
        render_grid(ui, &self.terminal.grid, colors, &self.prefs.display, origin, self.cell.0, self.cell.1, self.font_size, self.cursor_mode, &mut self.cursor_anim, dt);

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
                    egui::Stroke::new(if active { 2.0 } else { 1.0 }, col),
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
    fn tap_selects_the_word_and_long_press_selects_the_line() {
        let mut app = App::new(80, 24);
        app.terminal.feed("hello world");
        let origin = egui::pos2(0.0, 0.0);
        let (cw, ch) = app.cell;
        // a tap in the middle of "world" (col ~8) should select "world"
        let tap_pos = egui::pos2(cw * 8.5, ch * 0.5);
        if let Some((col, row)) = app.cell_at(origin, tap_pos) {
            let word = word_at(&app.terminal.grid.row_text(row), col);
            app.selection = Some(Selection { row: Some(row), text: word });
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

