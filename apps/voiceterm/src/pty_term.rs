// src/pty_term.rs — lean PTY terminal emulator
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

/// v0.1.434: is WEB2QML_PROFILE=1 set? Cached after first read so
/// the per-byte-burst check in the read loop is a cheap atomic
/// load, not a getenv each time. Used to gate the Helix-latency
/// diagnostic timing (alt-screen enter cost + read-burst timing).
fn profile_on() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    static CACHE: AtomicU8 = AtomicU8::new(0); // 0=unknown,1=off,2=on
    match CACHE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let on = std::env::var("WEB2QML_PROFILE").map(|v| v == "1").unwrap_or(false);
            CACHE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

/// v0.1.788: the native (QML Canvas) terminal renderer is now the DEFAULT.
/// When native, the reader SKIPS the expensive render_html() build entirely —
/// the Canvas paints glyphs directly from the grid (StyledRun), so the HTML
/// string is dead weight. Set WEB2QML_HTML_TERM=1 to fall back to the old
/// RichText HTML renderer (this MUST match native_term() on the QML side, which
/// decides whether the Canvas is shown / the HTML rows hidden). Cached like
/// profile_on so the hot reader loop doesn't re-read the environment.
fn native_render() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    static CACHE: AtomicU8 = AtomicU8::new(0); // 0=unknown,1=html,2=native
    match CACHE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let native = !matches!(std::env::var("WEB2QML_HTML_TERM").as_deref(), Ok("1"));
            CACHE.store(if native { 2 } else { 1 }, Ordering::Relaxed);
            native
        }
    }
}

/// v0.1.565: is WEB2QML_TAP=1 set? When on, the reader thread appends
/// every raw PTY byte burst (hex + escaped-ASCII) to
/// /tmp/web2qml-tap.log, so render glitches (e.g. the helix selection
/// off-by-one in TODO_selection_off_by_one.md) can be diagnosed from
/// the EXACT byte stream the program sent — the evidence the prior
/// session said it lacked. Off by default; zero cost on normal runs
/// (one cached atomic load). Same cache pattern as profile_on.
fn tap_dump_on() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    static CACHE: AtomicU8 = AtomicU8::new(0);
    match CACHE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let on = std::env::var("WEB2QML_TAP").map(|v| v == "1").unwrap_or(false);
            CACHE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

/// v0.1.565: append a raw byte burst to the tap log in a form that
/// makes ANSI/CSI sequences legible (hex pairs + a readable escaped
/// view where ESC shows as \e and control bytes as \xNN). Best-effort:
/// failures are silent (diagnostics must never disrupt the read loop).
fn tap_dump_write(bytes: &[u8]) {
    use std::io::Write;
    let mut readable = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        match b {
            0x1b => readable.push_str("\\e"),
            b'\n' => readable.push_str("\\n\n"),
            b'\r' => readable.push_str("\\r"),
            0x20..=0x7e => readable.push(b as char),
            _ => readable.push_str(&format!("\\x{:02x}", b)),
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true).append(true).open("/tmp/web2qml-tap.log")
    {
        let _ = writeln!(f, "--- burst {} bytes ---\n{}", bytes.len(), readable);
    }
}

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    /// Foreground: 0-255 = palette index, 256+ encodes 24-bit RGB
    pub fg: u16,
    pub bg: u16,
    /// bit 0=bold, 1=italic, 2=underline, 3=reverse, 4=strikethrough, 5=dim, 6=blink, 7=phantom
    ///
    /// "Phantom" (bit 7) marks a cell that is the right half of a
    /// wide character (CJK, emoji, etc.) whose left half lives in
    /// the cell to its immediate left. The phantom cell holds a
    /// space char and is SKIPPED during render — the wide glyph
    /// from the left cell paints over both cells visually. This
    /// is how we preserve column-precise cursor positioning even
    /// when the terminal contains mixed-width content.
    ///
    /// Without phantom-tracking, a 2-cell-wide char at column N
    /// would either (a) live in cells[N] only and visually overrun
    /// cells[N+1], creating offset drift, or (b) advance cursor
    /// by 2 cells but leave cells[N+1] uninitialized, so the next
    /// char written at cells[N+1] (e.g. via cursor positioning)
    /// would land somewhere unexpected. The phantom bit lets us
    /// model the cell as "occupied but render-skipped", giving
    /// correct cursor advances without rendering surprises.
    pub attrs: u8,
}
impl Default for Cell {
    fn default() -> Self { Cell { ch: ' ', fg: 256, bg: 256, attrs: 0 } }
}
impl Cell {
    #[inline] pub fn bold(&self)   -> bool { self.attrs & 0b0000_0001 != 0 }
    #[inline] pub fn italic(&self) -> bool { self.attrs & 0b0000_0010 != 0 }
    #[inline] pub fn under(&self)  -> bool { self.attrs & 0b0000_0100 != 0 }
    #[inline] pub fn rev(&self)    -> bool { self.attrs & 0b0000_1000 != 0 }
    #[inline] pub fn strike(&self) -> bool { self.attrs & 0b0001_0000 != 0 }
    #[inline] pub fn dim(&self)    -> bool { self.attrs & 0b0010_0000 != 0 }
    /// True if this cell is the right half of a wide character whose
    /// glyph is in the cell to its immediate left. Skip during render.
    #[inline] pub fn phantom(&self) -> bool { self.attrs & 0b1000_0000 != 0 }
}

const ATTR_BOLD:   u8 = 0b0000_0001;
const ATTR_ITALIC: u8 = 0b0000_0010;
const ATTR_UNDER:  u8 = 0b0000_0100;
const ATTR_REV:    u8 = 0b0000_1000;
const ATTR_STRIKE: u8 = 0b0001_0000;
const ATTR_DIM:    u8 = 0b0010_0000;
const _ATTR_BLINK: u8 = 0b0100_0000;
const ATTR_PHANTOM: u8 = 0b1000_0000;

/// FG/BG sentinel for "default"
const COLOR_DEFAULT: u16 = 256;

/// v0.1.785: native styled-run representation of a terminal row — the data the
/// (in-progress) native renderer consumes INSTEAD of an HTML string. A run is a
/// maximal span of cells with identical styling, with colours already resolved
/// to RGB and attributes reduced to a flag byte. This is the decoupling target:
/// the grid produces `Vec<StyledRun>`, and HTML becomes just one serializer of
/// runs (browser-style RichText) while the native renderer is another. Both go
/// through TermGrid::resolve_color, so the palette logic stays single-sourced.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledRun {
    pub text: String,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    /// bit0 bold, bit1 italic, bit2 underline, bit3 strikethrough, bit4 dim.
    pub flags: u8,
}
pub const RUN_BOLD: u8 = 0b0000_0001;
pub const RUN_ITALIC: u8 = 0b0000_0010;
pub const RUN_UNDER: u8 = 0b0000_0100;
pub const RUN_STRIKE: u8 = 0b0000_1000;
pub const RUN_DIM: u8 = 0b0001_0000;

pub struct TermGrid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Vec<Cell>>,
    /// v0.1.488: render dirty flag. Set true whenever bytes are fed
    /// into the grid (any mutation may have occurred); cleared by the
    /// main thread after it renders. Lets the frame driver SKIP the
    /// full grid→HTML serialization when nothing changed — the
    /// idle-terminal case (reading/thinking) that was paying a full
    /// render + string-compare every tick for no reason. Conservative:
    /// set on any byte processed, so it can never miss a real change.
    dirty: bool,
    /// v0.1.494 (dirty-rect stage b): inclusive range of rows changed
    /// since the last render, as (min, max). None = no rows dirty.
    /// CONSERVATIVE: precise for `print` (the hot path, marks one row);
    /// every bulk/uncertain mutation marks ALL rows (full range) so we
    /// can NEVER miss a changed row — over-marking only costs a
    /// redundant re-render, under-marking would paint stale content.
    /// A future per-row cache (dirty-rect arc stage c) renders only
    /// rows in this range; until then it's tracked but unused.
    dirty_rows: Option<(usize, usize)>,
    /// v0.1.892 (dirty-rect arc stage c): per-visible-row cache of the
    /// native renderer's packed run encoding. `term_screen_runs` repacks
    /// the WHOLE screen every frame; with this cache it repacks only the
    /// rows that actually changed and reuses cached strings for the rest.
    /// Correct by construction: entries are invalidated in `mark_row_dirty`
    /// / `mark_all_dirty`, the same hooks the existing render-gating trusts,
    /// and every content change (scrollback shift, resize, alt-screen swap,
    /// cell write) already routes through one of them. Indexed by visible
    /// row; `None` = needs recompute. Lazily (re)sized to `rows`.
    row_pack_cache: Vec<Option<String>>,
    /// How many times a row has actually been re-packed (cache miss). Exposed
    /// so tests can assert the SHAPE of the work, not just the output: a
    /// streaming `cat` must cost O(lines), not O(lines × rows). Wraps
    /// harmlessly; it is a diagnostic, not state.
    packs_computed: u64,
    pub cursor_row: usize,
    pub cursor_col: usize,
    scroll_top: usize,
    scroll_bot: usize,
    saved_row: usize,
    saved_col: usize,
    saved_attrs: u8,
    saved_fg: u16,
    saved_bg: u16,
    saved_charset_g0: Charset,
    saved_charset_active: u8,
    /// Cursor saved state for alt screen
    alt_saved_row: usize,
    alt_saved_col: usize,
    alt_saved_attrs: u8,
    alt_cells: Option<Vec<Vec<Cell>>>,
    cur_fg: u16, cur_bg: u16, cur_attrs: u8,
    /// Charsets — G0 and G1 selectable. Active is which one is currently used.
    charset_g0: Charset,
    charset_g1: Charset,
    /// 0 = G0, 1 = G1
    charset_active: u8,
    /// Tab stops (column indices that are tab targets)
    tab_stops: Vec<bool>,
    /// DEC private modes
    pub origin_mode: bool,    // DECOM (CSI ?6) — cursor relative to scroll region
    pub autowrap: bool,       // DECAWM (CSI ?7) — auto-wrap at end of line
    pub insert_mode: bool,    // IRM (CSI 4) — insert vs overwrite
    pub wrap_pending: bool,   // Cursor is in "last column, next char wraps" state
    pub mouse_mode: u16,      // 0=off, 1000=normal, 1002=button-event, 1003=any-event
    pub mouse_protocol: u8,   // 0=x10, 1=utf8, 2=sgr (1006), 3=urxvt (1015)
    // Scrollback
    scrollback: Vec<Vec<Cell>>,
    pub scrollback_pos: usize,
    /// v0.1.124: monotonic counter of total lines scrolled off the
    /// top of the screen. Combined with the current row index this
    /// gives an absolute line address that's stable across future
    /// scrolling. The QML selection state stores these absolute
    /// addresses so a selection made before content scrolls stays
    /// pinned to the same text (or clips cleanly if it scrolls
    /// off into scrollback).
    pub lines_emitted: u64,
    // OSC state
    pub title: String,
    /// v0.1.883: exit code from the most recent OSC 133 `D;<code>` shell-
    /// integration marker (the previous command's status). Drained by the
    /// main thread when a block closes, to surface ✓/✗ in notifications.
    /// `None` until a `D` marker with a numeric code is seen.
    pub osc133_exit: Option<i32>,
    pub cursor_shape: u8,
    pub bracketed_paste: bool,
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub cursor_visible: bool,
    /// v0.1.418: terminal-response buffer. Programs can query the
    /// terminal (e.g. DSR `ESC[6n` = "report cursor position");
    /// a real terminal replies on the input stream. We have no
    /// direct write-back from the grid, so csi_dispatch pushes
    /// the reply bytes here and the PTY read loop drains them to
    /// the master each tick. Without this, tools that send DSR
    /// and BLOCK waiting for the reply (ollama's spinner library,
    /// some prompt frameworks) stall until their timeout — fast
    /// in konsole, slow for us. This closes that gap.
    pub response_buf: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Ascii,
    DecLineDrawing,
}

impl Default for Charset {
    fn default() -> Self { Charset::Ascii }
}

/// Translate a character through the given charset
fn translate_charset(c: char, cs: Charset) -> char {
    match cs {
        Charset::Ascii => c,
        Charset::DecLineDrawing => match c {
            '`' => '◆',  '_' => ' ',
            'a' => '▒',  'b' => '␉',  'c' => '␌',  'd' => '␍',  'e' => '␊',
            'f' => '°',  'g' => '±',  'h' => '␤',  'i' => '␋',
            'j' => '┘',  'k' => '┐',  'l' => '┌',  'm' => '└',  'n' => '┼',
            'o' => '⎺',  'p' => '⎻',  'q' => '─',  'r' => '⎼',  's' => '⎽',
            't' => '├',  'u' => '┤',  'v' => '┴',  'w' => '┬',  'x' => '│',
            'y' => '≤',  'z' => '≥',
            '{' => 'π',  '|' => '≠',  '}' => '£',  '~' => '·',
            other => other,
        }
    }
}

impl TermGrid {
    /// v0.1.896: rough resident size of this grid's cell storage — visible
    /// grid + alt screen + scrollback. Counts allocated capacity (not len)
    /// so it reflects what's actually held. For the mem-footprint log only.
    pub fn approx_mem_bytes(&self) -> usize {
        let cell = std::mem::size_of::<Cell>();
        let row_hdr = std::mem::size_of::<Vec<Cell>>();
        let count = |rows: &Vec<Vec<Cell>>| -> usize {
            rows.iter().map(|r| r.capacity() * cell + row_hdr).sum::<usize>()
                + rows.capacity().saturating_sub(rows.len()) * row_hdr
        };
        let mut bytes = count(&self.cells) + count(&self.scrollback);
        if let Some(alt) = &self.alt_cells {
            bytes += count(alt);
        }
        bytes
    }

    /// v0.1.896: number of rows currently held in scrollback (the unbounded-
    /// growth axis worth watching in the footprint log).
    pub fn scrollback_rows(&self) -> usize {
        self.scrollback.len()
    }

    pub fn new(cols: usize, rows: usize) -> Self {
        // Default tab stops every 8 columns
        let mut tab_stops = vec![false; cols.max(1)];
        let mut i = 8;
        while i < cols { tab_stops[i] = true; i += 8; }

        TermGrid {
            cols, rows,
            cells: vec![vec![Cell::default(); cols]; rows],
            // start dirty so the initial screen renders once.
            dirty: true,
            dirty_rows: None,
            row_pack_cache: Vec::new(),
            packs_computed: 0,
            cursor_row: 0, cursor_col: 0,
            scroll_top: 0, scroll_bot: rows.saturating_sub(1),
            saved_row: 0, saved_col: 0,
            saved_attrs: 0, saved_fg: COLOR_DEFAULT, saved_bg: COLOR_DEFAULT,
            saved_charset_g0: Charset::Ascii, saved_charset_active: 0,
            alt_saved_row: 0, alt_saved_col: 0, alt_saved_attrs: 0,
            alt_cells: None,
            cur_fg: COLOR_DEFAULT, cur_bg: COLOR_DEFAULT, cur_attrs: 0,
            charset_g0: Charset::Ascii,
            charset_g1: Charset::Ascii,
            charset_active: 0,
            tab_stops,
            origin_mode: false,
            autowrap: true,    // DECAWM defaults ON
            insert_mode: false,
            wrap_pending: false,
            mouse_mode: 0,
            mouse_protocol: 0,
            scrollback: Vec::new(), scrollback_pos: 0, lines_emitted: 0,
            title: String::new(), osc133_exit: None, cursor_shape: 0,
            bracketed_paste: false, app_cursor: false,
            app_keypad: false, cursor_visible: true,
            response_buf: Vec::new(),
        }
    }

    #[inline] fn blank_line(&self) -> Vec<Cell> { vec![Cell::default(); self.cols] }

    /// Blank cell that preserves the current bg color (proper erase semantics)
    #[inline] fn blank_with_bg(&self) -> Cell {
        Cell { ch: ' ', fg: COLOR_DEFAULT, bg: self.cur_bg, attrs: 0 }
    }

    /// Move cursor to next tab stop
    fn tab_forward(&mut self) {
        let start = self.cursor_col + 1;
        for col in start..self.cols {
            if self.tab_stops.get(col).copied().unwrap_or(false) {
                self.cursor_col = col;
                return;
            }
        }
        self.cursor_col = self.cols.saturating_sub(1);
    }

    /// Move backward to previous tab stop (CBT)
    fn tab_backward(&mut self, n: usize) {
        for _ in 0..n.max(1) {
            if self.cursor_col == 0 { break; }
            let mut col = self.cursor_col - 1;
            loop {
                if self.tab_stops.get(col).copied().unwrap_or(false) || col == 0 {
                    self.cursor_col = col;
                    break;
                }
                col -= 1;
            }
        }
    }

    fn scroll_up(&mut self) {
        if self.scroll_top < self.cells.len() && self.scroll_bot < self.cells.len() {
            // Save scrolled-off line to scrollback (only in main screen, not alt)
            if self.alt_cells.is_none() && self.scroll_top == 0 {
                let line = self.cells[self.scroll_top].clone();
                self.scrollback.push(line);
                // Cap scrollback at 5000 lines
                if self.scrollback.len() > 5000 { self.scrollback.remove(0); }
                // v0.1.124: monotonic line counter. Bumps on every
                // top-line scroll-off so selection coordinates stored
                // by QML can be converted to/from absolute addresses.
                self.lines_emitted = self.lines_emitted.saturating_add(1);
            }
            self.cells.remove(self.scroll_top);
            self.cells.insert(self.scroll_bot, self.blank_line());
        }
        // A scroll SHIFTS rows; it does not change their content. The old code
        // marked everything dirty and dropped the whole packed cache, so every
        // newline at the bottom of the screen re-packed the entire screen —
        // O(rows × cols) per line of streaming output, which is the classic
        // terminal cliff (`cat` of a big file, a compile log). alacritty and
        // cosmic-term avoid it structurally by keeping the grid in a ring
        // buffer, where a scroll is an index bump and row identity survives.
        //
        // We get the same effect without changing the storage model: shift the
        // cache in lockstep with the rows it describes. Entry i still describes
        // whatever now sits at row i, so the cache stays CORRECT while the
        // repack cost drops to the one newly exposed blank row. The renderer is
        // still told every row is dirty — they all moved on screen — but being
        // dirty now costs a memcpy of a cached string instead of re-formatting
        // it from cells.
        self.scroll_cache_up();
        self.mark_all_dirty_keep_cache();
    }

    /// Shift the packed-row cache up one, mirroring `cells`: drop the entry for
    /// the line that scrolled off, and leave the newly exposed row uncached.
    fn scroll_cache_up(&mut self) {
        if self.row_pack_cache.len() == self.rows
            && self.scroll_top < self.row_pack_cache.len()
            && self.scroll_bot < self.row_pack_cache.len()
        {
            self.row_pack_cache.remove(self.scroll_top);
            self.row_pack_cache.insert(self.scroll_bot, None);
        } else {
            // shape mismatch (mid-resize): correctness first, drop it all
            self.row_pack_cache.clear();
        }
    }

    /// Shift the packed-row cache down one, mirroring `cells`.
    fn scroll_cache_down(&mut self) {
        if self.row_pack_cache.len() == self.rows
            && self.scroll_top < self.row_pack_cache.len()
            && self.scroll_bot < self.row_pack_cache.len()
        {
            self.row_pack_cache.remove(self.scroll_bot);
            self.row_pack_cache.insert(self.scroll_top, None);
        } else {
            self.row_pack_cache.clear();
        }
    }

    /// Mark every row dirty for the RENDERER without discarding the packed
    /// cache. Only correct when the cache has been shifted to match the rows
    /// (see the scroll paths); every other bulk mutation must keep using
    /// `mark_all_dirty`, which drops the cache.
    fn mark_all_dirty_keep_cache(&mut self) {
        self.dirty = true;
        let last = self.rows.saturating_sub(1);
        self.dirty_rows = Some((0, last));
    }
    fn scroll_down(&mut self) {
        if self.scroll_bot < self.cells.len() && self.scroll_top < self.cells.len() {
            self.cells.remove(self.scroll_bot);
            self.cells.insert(self.scroll_top, self.blank_line());
        }
        self.scroll_cache_down();
        self.mark_all_dirty_keep_cache();
    }

    fn enter_alt(&mut self) {
        if self.alt_cells.is_none() {
            // v0.1.434: profile alt-screen enter cost. Gated on
            // WEB2QML_PROFILE — helps diagnose the "Helix opens
            // slow vs konsole instant" report. enter_alt itself
            // should be cheap (one clone + a fresh grid alloc);
            // if THIS is fast but Helix still feels slow, the cost
            // is downstream (parse flood / QML re-render), which
            // the read-loop burst timing (below) will show.
            let t0 = if profile_on() { Some(std::time::Instant::now()) } else { None };
            self.alt_cells = Some(self.cells.clone());
            self.cells = vec![vec![Cell::default(); self.cols]; self.rows];
            // Save main-screen cursor state
            self.alt_saved_row = self.cursor_row;
            self.alt_saved_col = self.cursor_col;
            self.alt_saved_attrs = self.cur_attrs;
            self.cursor_row = 0; self.cursor_col = 0;
            self.wrap_pending = false;
            if let Some(t0) = t0 {
                crate::clog!("profile", "altscreen:enter {}x{} took {:?}",
                    self.cols, self.rows, t0.elapsed());
            }
        }
    }
    fn exit_alt(&mut self) {
        if let Some(saved) = self.alt_cells.take() {
            self.cells = saved;
            self.cursor_row = self.alt_saved_row.min(self.rows - 1);
            self.cursor_col = self.alt_saved_col.min(self.cols - 1);
            self.cur_attrs = self.alt_saved_attrs;
            self.wrap_pending = false;
        }
    }

    /// DECSC — save cursor state (full save)
    fn save_cursor(&mut self) {
        self.saved_row = self.cursor_row;
        self.saved_col = self.cursor_col;
        self.saved_attrs = self.cur_attrs;
        self.saved_fg = self.cur_fg;
        self.saved_bg = self.cur_bg;
        self.saved_charset_g0 = self.charset_g0;
        self.saved_charset_active = self.charset_active;
    }
    /// DECRC — restore cursor state
    fn restore_cursor(&mut self) {
        self.cursor_row = self.saved_row.min(self.rows - 1);
        self.cursor_col = self.saved_col.min(self.cols - 1);
        self.cur_attrs = self.saved_attrs;
        self.cur_fg = self.saved_fg;
        self.cur_bg = self.saved_bg;
        self.charset_g0 = self.saved_charset_g0;
        self.charset_active = self.saved_charset_active;
        self.wrap_pending = false;
    }

    /// Apply origin-mode-aware row positioning
    fn apply_row(&self, row: usize) -> usize {
        if self.origin_mode {
            (row + self.scroll_top).min(self.scroll_bot)
        } else {
            row.min(self.rows - 1)
        }
    }

    /// Standard 256-color palette → CSS hex
    /// 0-7:   standard
    /// 8-15:  bright
    /// 16-231: 6×6×6 RGB cube
    /// 232-255: greyscale
    fn palette_color(idx: u16) -> [u8; 3] {
        match idx {
            // Standard colors (low-key but readable on dark theme)
            0 => [40, 42, 54],     // black
            1 => [255, 85, 85],    // red
            2 => [80, 250, 123],   // green
            3 => [241, 250, 140],  // yellow
            4 => [98, 114, 164],   // blue
            5 => [255, 121, 198],  // magenta
            6 => [139, 233, 253],  // cyan
            7 => [248, 248, 242],  // white (default fg)
            // Bright
            8  => [98, 114, 164],
            9  => [255, 110, 110],
            10 => [120, 255, 150],
            11 => [255, 240, 165],
            12 => [148, 168, 224],
            13 => [255, 160, 220],
            14 => [180, 250, 255],
            15 => [255, 255, 255],
            // 6×6×6 cube
            16..=231 => {
                let i = (idx - 16) as u32;
                let r = i / 36;
                let g = (i / 6) % 6;
                let b = i % 6;
                let levels = [0u8, 95, 135, 175, 215, 255];
                [levels[r as usize], levels[g as usize], levels[b as usize]]
            }
            // Greyscale
            232..=255 => {
                let v = 8 + (idx - 232) as u32 * 10;
                let v = v.min(255) as u8;
                [v, v, v]
            }
            // 256 = default — caller decides
            _ => [200, 224, 160],
        }
    }

    /// v0.1.785: resolve a cell colour index to RGB. The SINGLE seam shared by
    /// the HTML serializer (write_color_hex, for the browser-style RichText path)
    /// and the native run builder (render_row_runs, for the in-progress native
    /// terminal renderer). Keeping both on this one function is how the palette /
    /// default-colour logic never forks as the terminal moves off HTML.
    #[inline]
    fn resolve_color(idx: u16, default: [u8; 3]) -> [u8; 3] {
        if idx == COLOR_DEFAULT { default } else { Self::palette_color(idx) }
    }

    fn write_color_hex(out: &mut String, idx: u16, default: [u8; 3]) {
        let rgb = Self::resolve_color(idx, default);
        let _ = std::fmt::Write::write_fmt(out, format_args!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]));
    }

    /// v0.1.490: read-only dirty check (does NOT clear). Used by the
    /// UI to detect a main-thread mutation that needs republishing.
    pub fn is_dirty(&self) -> bool { self.dirty }

    /// Diagnostic: how many row re-packs have happened. See `packs_computed`.
    pub fn packs_computed(&self) -> u64 { self.packs_computed }

    /// v0.1.494: mark a single row changed (extends the dirty-row
    /// range). Called from the precise hot path (`print`). Also sets
    /// the whole-grid dirty flag so existing render-gating still
    /// works unchanged.
    fn mark_row_dirty(&mut self, row: usize) {
        self.dirty = true;
        self.dirty_rows = Some(match self.dirty_rows {
            Some((lo, hi)) => (lo.min(row), hi.max(row)),
            None => (row, row),
        });
        // v0.1.892: drop this row's packed cache so it repacks next frame.
        if let Some(slot) = self.row_pack_cache.get_mut(row) {
            *slot = None;
        }
    }

    /// v0.1.494: mark EVERY row changed — the conservative fallback
    /// for bulk/uncertain mutations (clear, scroll, line insert/delete,
    /// resize). Correct by construction: never misses a changed row.
    fn mark_all_dirty(&mut self) {
        self.dirty = true;
        let last = self.rows.saturating_sub(1);
        self.dirty_rows = Some((0, last));
        // v0.1.892: blow away the whole packed cache — every row may differ.
        // Cleared (not None-filled) so the accessor re-sizes to the current
        // `rows` lazily, which also covers a resize that changed the count.
        self.row_pack_cache.clear();
    }

    /// v0.1.494: take the dirty-row range and clear it (range form of
    /// take_dirty). A future per-row cache uses this to know which
    /// rows to re-render. Returns None if no rows are dirty.
    pub fn take_dirty_rows(&mut self) -> Option<(usize, usize)> {
        self.dirty_rows.take()
    }

    /// v0.1.488: return whether the grid changed since the last call
    /// and clear the flag. The frame driver uses this to skip the
    /// (expensive) render_html + string-compare when nothing changed.
    /// Also reports dirty if scrollback is being viewed or the alt
    /// screen is active in a way the byte-feed flag wouldn't catch on
    /// a main-thread-only change — callers additionally force a render
    /// on cursor move / scrollback-pos change (cheap signals).
    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    /// Render as HTML with color spans for QML RichText.
    /// Compact: groups runs of same-styled cells into single spans.
    pub fn render_html(&self) -> String {
        // Pre-allocate generously: ~6 chars/cell average for styled output
        let mut out = String::with_capacity(self.rows * self.cols * 6 + 256);
        // Qt's RichText renderer wraps content in an implicit <body> with
        // default margin/padding. That leaks ~4-8px of dead space at the
        // top-left of the visible area, which OFFSETS every visible cell
        // from where our `2 + col*charW` cursor math expects them. The
        // result is the longstanding "click is off the mark / cursor is
        // misaligned" bug.
        //
        // Fix: emit an explicit <html><body style="margin:0;padding:0">
        // so the document origin is exactly the TextEdit's content rect
        // origin. Combined with the existing <pre style='margin:0'>
        // wrapper, every cell now sits at deterministic (col*charW, row*charH)
        // relative to the TextEdit's anchors.margins inset.
        out.push_str("<html><body style='margin:0;padding:0;'>");
        out.push_str("<pre style='margin:0;padding:0;white-space:pre;'>");

        let default_fg: [u8; 3] = [200, 224, 160]; // matches th.tfg default
        let default_bg: [u8; 3] = [13, 13, 13];    // matches th.tbg

        let view_rows: Vec<&Vec<Cell>> = if self.scrollback_pos > 0 && self.alt_cells.is_none() {
            // Show scrollback portion in upper part, live in lower
            let sb_take = self.scrollback_pos.min(self.rows);
            let sb_start = self.scrollback.len().saturating_sub(self.scrollback_pos);
            let mut v: Vec<&Vec<Cell>> = self.scrollback[sb_start..sb_start + sb_take].iter().collect();
            let live_show = self.rows - sb_take;
            for i in 0..live_show {
                v.push(&self.cells[i]);
            }
            v
        } else {
            self.cells.iter().collect()
        };

        let total_rows = view_rows.len();
        for (row_idx, row) in view_rows.iter().enumerate() {
            Self::render_row_into(&mut out, row, default_fg, default_bg);
            // Newline between rows, but NOT after the last row (avoids phantom row + selection bug)
            if row_idx + 1 < total_rows {
                out.push('\n');
            }
        }
        out.push_str("</pre></body></html>");
        out
    }

    /// v0.1.493: render ONE row's HTML into `out`. Extracted verbatim
    /// from render_html's inner loop (behavior-identical) so the row
    /// is an addressable unit — the foundation for per-row caching
    /// (re-render only changed rows) without touching the row logic.
    /// Appends the row's spans; does NOT append the trailing newline
    /// (the caller manages inter-row separators).
    fn render_row_into(out: &mut String, row: &[Cell],
                       default_fg: [u8; 3], default_bg: [u8; 3]) {
        // Trim trailing whitespace runs of default cells
        let end = row.iter().rposition(|c| {
            c.ch != ' ' || c.bg != COLOR_DEFAULT || c.attrs != 0
        }).map(|p| p + 1).unwrap_or(0);

        let mut i = 0;
        while i < end {
            let cell = row[i];
            // Find run of identical styling
            let mut j = i + 1;
            while j < end {
                let next = row[j];
                if next.fg != cell.fg || next.bg != cell.bg || next.attrs != cell.attrs { break; }
                j += 1;
            }

            let has_style = cell.fg != COLOR_DEFAULT || cell.bg != COLOR_DEFAULT || cell.attrs != 0;
            if has_style {
                out.push_str("<span style='");
                // Reverse swaps fg/bg
                let (eff_fg, eff_bg) = if cell.rev() {
                    (cell.bg, cell.fg)
                } else { (cell.fg, cell.bg) };

                out.push_str("color:");
                Self::write_color_hex(out,
                    eff_fg,
                    if cell.rev() { default_bg } else { default_fg });
                if eff_bg != COLOR_DEFAULT || cell.rev() {
                    out.push_str(";background:");
                    Self::write_color_hex(out,
                        eff_bg,
                        if cell.rev() { default_fg } else { default_bg });
                }
                if cell.bold()   { out.push_str(";font-weight:700"); }
                if cell.italic() { out.push_str(";font-style:italic"); }
                if cell.under() && cell.strike() { out.push_str(";text-decoration:underline line-through"); }
                else if cell.under()  { out.push_str(";text-decoration:underline"); }
                else if cell.strike() { out.push_str(";text-decoration:line-through"); }
                if cell.dim() { out.push_str(";opacity:0.6"); }
                out.push_str("'>");
            }

            for k in i..j {
                // Skip phantom cells — these are right-halves of
                // wide chars; the wide glyph from the cell to the
                // left already paints over both cells visually.
                if row[k].phantom() { continue; }
                match row[k].ch {
                    '<'  => out.push_str("&lt;"),
                    '>'  => out.push_str("&gt;"),
                    '&'  => out.push_str("&amp;"),
                    '\0' => out.push(' '),
                    c    => out.push(c),
                }
            }
            if has_style { out.push_str("</span>"); }
            i = j;
        }
    }

    /// v0.1.785: native styled-run builder — the run-coalescing twin of
    /// render_row_into, but emitting resolved RGB + a flag byte instead of HTML
    /// spans. SAME grouping (adjacent cells with identical fg/bg/attrs), SAME
    /// reverse-video swap, SAME palette resolution (via resolve_color), SAME
    /// phantom-skip and trailing-default trim — so the native renderer and the
    /// HTML path stay pixel-equivalent. Background is always resolved to a
    /// concrete colour (the HTML path omits a default-bg span and lets the
    /// container show through; the native renderer paints every cell, so the
    /// default bg is made explicit here — same visual result).
    pub fn render_row_runs(row: &[Cell], default_fg: [u8; 3], default_bg: [u8; 3]) -> Vec<StyledRun> {
        let end = row.iter().rposition(|c| {
            !c.phantom() && (c.ch != ' ' || c.bg != COLOR_DEFAULT || c.attrs != 0)
        }).map(|p| p + 1).unwrap_or(0);
        let mut runs: Vec<StyledRun> = Vec::new();
        let mut i = 0;
        while i < end {
            let cell = row[i];
            let mut j = i + 1;
            while j < end {
                let next = row[j];
                if next.fg != cell.fg || next.bg != cell.bg || next.attrs != cell.attrs { break; }
                j += 1;
            }
            // reverse-video swaps fg/bg; resolve both to RGB.
            let (raw_fg, raw_bg) = if cell.rev() { (cell.bg, cell.fg) } else { (cell.fg, cell.bg) };
            let fg = Self::resolve_color(raw_fg, if cell.rev() { default_bg } else { default_fg });
            let bg = Self::resolve_color(raw_bg, if cell.rev() { default_fg } else { default_bg });
            let mut flags = 0u8;
            if cell.bold()   { flags |= RUN_BOLD; }
            if cell.italic() { flags |= RUN_ITALIC; }
            if cell.under()  { flags |= RUN_UNDER; }
            if cell.strike() { flags |= RUN_STRIKE; }
            if cell.dim()    { flags |= RUN_DIM; }
            let mut text = String::with_capacity(j - i);
            for k in i..j {
                if row[k].phantom() { continue; } // right half of a wide glyph
                let c = row[k].ch;
                text.push(if c == '\0' { ' ' } else { c });
            }
            runs.push(StyledRun { text, fg, bg, flags });
            i = j;
        }
        runs
    }

    /// v0.1.785: compact transport encoding of a visible row's runs for the
    /// native renderer (QML splits it — far cheaper than parsing HTML or JSON).
    /// Runs are separated by US-rs (0x1e); within a run the fields are separated
    /// by US (0x1f): `<fg 6hex>·<bg 6hex>·<flags decimal>·<text>`. Terminal cells
    /// hold printable glyphs, so the 0x1e/0x1f separators can't collide. NOTE:
    /// this reads the grid directly; once the native path is the default it
    /// should be snapshot-backed (like the HTML render) so it never locks the
    /// grid on the UI thread — see the migration notes.
    pub fn row_runs_packed(&self, idx: usize) -> String {
        let row = match self.row_slice(idx) { Some(r) => r, None => return String::new() };
        let default_fg: [u8; 3] = [200, 224, 160];
        let default_bg: [u8; 3] = [13, 13, 13];
        let runs = Self::render_row_runs(row, default_fg, default_bg);
        let mut out = String::new();
        for (n, r) in runs.iter().enumerate() {
            if n > 0 { out.push('\u{1e}'); }
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!(
                "{:02x}{:02x}{:02x}\u{1f}{:02x}{:02x}{:02x}\u{1f}{}\u{1f}",
                r.fg[0], r.fg[1], r.fg[2], r.bg[0], r.bg[1], r.bg[2], r.flags));
            out.push_str(&r.text);
        }
        out
    }

    /// v0.1.892 (dirty-rect arc stage c): cached form of `row_runs_packed`.
    /// Returns the packed encoding for visible row `idx`, recomputing only on a
    /// cache miss (after the row was marked dirty) and reusing the cached string
    /// otherwise. The cache is invalidated wherever the grid marks rows dirty,
    /// so a hit is guaranteed to match a fresh `row_runs_packed(idx)` — see the
    /// `row_pack_cache` field docs. Requires `&mut self` to populate the cache;
    /// `term_screen_runs` holds the grid lock mutably, so this is on the hot
    /// path. Out-of-range rows return "".
    pub fn row_runs_packed_cached(&mut self, idx: usize) -> &str {
        // Lazily (re)size to the current row count — also covers a resize that
        // changed `rows` (mark_all_dirty clears the vec, so len != rows here).
        if self.row_pack_cache.len() != self.rows {
            self.row_pack_cache = vec![None; self.rows];
        }
        if idx >= self.rows {
            return "";
        }
        if self.row_pack_cache[idx].is_none() {
            self.packs_computed = self.packs_computed.wrapping_add(1);
            let packed = self.row_runs_packed(idx);
            self.row_pack_cache[idx] = Some(packed);
        }
        self.row_pack_cache[idx].as_deref().unwrap_or("")
    }

    /// v0.1.902: whole-screen packed-runs wire format, assembled from the
    /// per-row cache (`row_runs_packed_cached`). Extracted out of
    /// `main.rs`'s `term_screen_runs` so the READER thread can build this
    /// off the UI thread (see `RenderSnapshot::runs`) instead of the UI
    /// thread `try_lock`-ing the grid on every poll tick — the exact
    /// "should be snapshot-backed" migration flagged since v0.1.786.
    /// Header: 2 chars (`alt_screen_active`, `cursor_visible`) + 0x1c, then
    /// rows separated by 0x1d, matching `term_screen_runs`'s prior inline
    /// assembly exactly (verified: `screen_runs_header_and_rows_match_prior_format`).
    pub fn screen_runs_packed(&mut self) -> String {
        let rows = self.rows;
        let mut out = String::with_capacity(rows * 24 + 3);
        out.push(if self.alt_screen_active() { '1' } else { '0' });
        out.push(if self.cursor_visible { '1' } else { '0' });
        out.push('\u{1c}');
        for r in 0..rows {
            if r > 0 { out.push('\u{1d}'); }
            out.push_str(self.row_runs_packed_cached(r));
        }
        out
    }

    /// Plain text — supports scrollback viewing
    pub fn render_text(&self) -> String {
        let mut out = String::with_capacity(self.rows * (self.cols + 1));
        // Same phantom-skip strategy as render_row — see comment there.
        let row_str = |row: &[Cell]| -> String {
            row.iter()
                .filter(|c| !c.phantom())
                .map(|c| c.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        if self.scrollback_pos > 0 && self.alt_cells.is_none() {
            // Show scrollback content
            let sb_start = self.scrollback.len().saturating_sub(self.scrollback_pos);
            let sb_lines = self.scrollback_pos.min(self.rows);
            for i in 0..self.rows {
                let sb_idx = sb_start + i;
                if sb_idx < self.scrollback.len() {
                    out.push_str(&row_str(&self.scrollback[sb_idx]));
                } else {
                    // Show live lines for remaining rows
                    let live_row = i.saturating_sub(sb_lines);
                    if live_row < self.cells.len() {
                        out.push_str(&row_str(&self.cells[live_row]));
                    }
                }
                out.push('\n');
            }
        } else {
            for row in &self.cells {
                out.push_str(&row_str(row));
                out.push('\n');
            }
        }
        let lc = out.lines().count();
        for _ in lc..self.rows { out.push('\n'); }
        out
    }

    /// Per-row text accessor — returns just one row's plain-text content
    /// without trailing whitespace. Idx is 0-indexed from the top of the
    /// visible viewport (or current scrollback position when scrolled).
    /// Returns empty string for out-of-range idx.
    ///
    /// This is the foundation of the v0.1.98 per-row rendering model:
    /// QML calls this once per visible row instead of consuming a single
    /// HTML-rendered blob. Predictable cell math (each row positions at
    /// `idx * charH` with no ascent surprises) eliminates the entire class
    /// of alignment bugs we've been chasing.
    pub fn render_row(&self, idx: usize) -> String {
        if idx >= self.rows { return String::new(); }
        // Filter helper: skip phantom cells (right half of wide chars).
        // The wide glyph in the cell to the left visually fills both
        // cells; rendering the phantom's space char would push the
        // remaining row content rightward by 1 cell each occurrence,
        // creating exactly the misalignment fastfetch displays.
        let row_to_string = |row: &[Cell]| -> String {
            row.iter()
                .filter(|c| !c.phantom())
                .map(|c| c.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        if self.scrollback_pos > 0 && self.alt_cells.is_none() {
            let sb_start = self.scrollback.len().saturating_sub(self.scrollback_pos);
            let sb_lines = self.scrollback_pos.min(self.rows);
            let sb_idx = sb_start + idx;
            if sb_idx < self.scrollback.len() {
                return row_to_string(&self.scrollback[sb_idx]);
            }
            // Fall through to live lines for the remainder of the viewport
            let live_row = idx.saturating_sub(sb_lines);
            if live_row < self.cells.len() {
                return row_to_string(&self.cells[live_row]);
            }
            return String::new();
        }
        if idx < self.cells.len() {
            return row_to_string(&self.cells[idx]);
        }
        String::new()
    }

    /// Number of visible rows. Stable per resize. Used by the QML Repeater
    /// model count.
    pub fn visible_rows(&self) -> usize { self.rows }

    /// v0.1.765: the last `max_rows` rows of terminal output as plain text
    /// (scrollback tail + the live viewport), newest content last. Used by the
    /// build-error extractor to summarize a just-finished `cargo`/`cmake` run
    /// without the user scrolling. Trailing blank lines are trimmed.
    pub fn recent_text(&self, max_rows: usize) -> String {
        let row_to_string = |row: &[Cell]| -> String {
            row.iter().filter(|c| !c.phantom()).map(|c| c.ch)
                .collect::<String>().trim_end().to_string()
        };
        // live viewport rows (current screen), oldest-first
        let mut live: Vec<String> = self.cells.iter().map(|r| row_to_string(r)).collect();
        // drop trailing blank live rows (the cursor usually sits on a blank line)
        while live.last().map(|l| l.is_empty()).unwrap_or(false) { live.pop(); }
        let live_n = live.len();
        // how many scrollback rows to prepend
        let sb_take = max_rows.saturating_sub(live_n).min(self.scrollback.len());
        let sb_start = self.scrollback.len() - sb_take;
        let mut out: Vec<String> = self.scrollback[sb_start..].iter()
            .map(|r| row_to_string(r)).collect();
        out.extend(live);
        out.join("\n")
    }

    /// Per-row HTML accessor — returns one row rendered with ANSI color
    /// spans. Called by QML when the Repeater delegate uses RichText
    /// for per-row rendering with full color support.
    ///
    /// Output format: `<span style='color:#xxxxxx'>text</span>...` for
    /// each run of same-styled cells. Phantom cells skipped (right
    /// half of wide chars). The full <html><body><pre> wrapper from
    /// render_html() is NOT emitted here — Qt's RichText renderer
    /// applies sensible defaults when fed bare span markup, and skipping
    /// the wrapper saves ~50 bytes per row × 24 rows × N updates per
    /// second.
    ///
    /// Default-styled cells (no fg, no bg, no attrs) emit raw text
    /// without spans — fastest path. Colored cells get spans grouped
    /// by run.
    ///
    /// Theme integration: the per-cell colors come from the ANSI palette
    /// directly. The `default_fg` and `default_bg` parameters to
    /// `write_color_hex` come from the theme — the SAME theme tfg/tbg
    /// QML uses for monochrome rendering. Cells with COLOR_DEFAULT
    /// fall back to those, so default text matches the theme exactly.
    /// v0.1.768: the scrollback-aware cell slice shown at visible row `idx`
    /// (the exact bytes the native renderer packs into styled runs via
    /// row_runs_packed). Shared by the renderer so the screen reflects the grid.
    /// None when the row is off the end of available content.
    fn row_slice(&self, idx: usize) -> Option<&[Cell]> {
        if idx >= self.rows { return None; }
        if self.scrollback_pos > 0 && self.alt_cells.is_none() {
            let sb_start = self.scrollback.len().saturating_sub(self.scrollback_pos);
            let sb_lines = self.scrollback_pos.min(self.rows);
            let sb_idx = sb_start + idx;
            if sb_idx < self.scrollback.len() {
                Some(&self.scrollback[sb_idx])
            } else {
                let live_row = idx.saturating_sub(sb_lines);
                self.cells.get(live_row).map(|r| r.as_slice())
            }
        } else {
            self.cells.get(idx).map(|r| r.as_slice())
        }
    }


    /// Scroll back into history
    /// Scroll backward into history. Returns true if the position
    /// actually moved (caller can detect "already at top" to surface
    /// a rubber-band bounce). Returns false in alt-screen (no
    /// scrollback) or when already at the oldest line.
    pub fn scroll_back(&mut self, lines: usize) -> bool {
        if self.alt_cells.is_some() { return false; }
        let prev = self.scrollback_pos;
        self.scrollback_pos = (self.scrollback_pos + lines).min(self.scrollback.len());
        if self.scrollback_pos != prev { self.mark_all_dirty(); }
        self.scrollback_pos != prev
    }

    /// Scroll forward toward live. Returns true if position moved.
    pub fn scroll_forward(&mut self, lines: usize) -> bool {
        let prev = self.scrollback_pos;
        self.scrollback_pos = self.scrollback_pos.saturating_sub(lines);
        if self.scrollback_pos != prev { self.mark_all_dirty(); }
        self.scrollback_pos != prev
    }

    /// Return to live view
    pub fn scroll_to_bottom(&mut self) {
        if self.scrollback_pos != 0 { self.mark_all_dirty(); }
        self.scrollback_pos = 0;
    }

    /// True when a TUI app (helix/vim/less/man/htop) has switched to the
    /// alternate screen buffer. Caller can use this to decide whether
    /// touch-drag-to-scroll should send arrow keys (which only move the cursor
    /// in TUIs, not the view) or Ctrl-U/Ctrl-D (which actually scroll).
    pub fn alt_screen_active(&self) -> bool { self.alt_cells.is_some() }

    /// Returns true if the foreground program has enabled mouse
    /// reporting (modes 1000, 1002, or 1003). When true, terminal
    /// wheel events should be sent as mouse reports rather than
    /// PgUp/PgDn keystrokes — TUIs like helix in mouse-tracking
    /// mode swallow keystrokes silently.
    pub fn mouse_reporting_enabled(&self) -> bool {
        self.mouse_mode != 0
    }

    /// Format a mouse wheel event as a bytes sequence in whatever
    /// protocol the foreground program is using.
    ///
    /// Args:
    ///   - col, row: 1-indexed terminal cell coordinates of the cursor
    ///     when the wheel event happened
    ///   - direction: -1 = up (away from user), +1 = down (toward user)
    ///
    /// Returns the encoded byte sequence, or empty if mouse reporting
    /// is off. Caller writes to PTY.
    ///
    /// Wheel buttons in xterm encoding:
    ///   - Wheel-up = button 64 (X10/normal: code 96 = 64+32 due to
    ///     the +32 offset that all X10/normal buttons use; SGR uses
    ///     64 directly)
    ///   - Wheel-down = button 65 (similar +1)
    pub fn encode_wheel_event(&self, col: u16, row: u16, direction: i8) -> Vec<u8> {
        if self.mouse_mode == 0 { return Vec::new(); }
        // Clamp coords to reasonable terminal-cell range.
        let col = col.max(1);
        let row = row.max(1);
        let button: u8 = if direction < 0 { 64 } else { 65 };
        match self.mouse_protocol {
            // SGR (1006) — \x1B[<button;col;rowM (M for press, m for release)
            // Wheel events only have "press" — there's no release.
            2 => {
                format!("\x1B[<{};{};{}M", button, col, row).into_bytes()
            }
            // urxvt (1015) — \x1B[button;col;rowM with button + 32
            3 => {
                format!("\x1B[{};{};{}M", button as u32 + 32, col, row).into_bytes()
            }
            // X10 / normal (default) — \x1B[M Cb Cx Cy with each + 32
            _ => {
                let cb = button + 32;
                let cx = (col.min(223) as u8) + 32;
                let cy = (row.min(223) as u8) + 32;
                vec![0x1B, b'[', b'M', cb, cx, cy]
            }
        }
    }

    /// v0.1.266 (helix tap-to-position): Format a mouse press/release
    /// pair as bytes for the foreground program. Used by terminal
    /// tap/click handling to dispatch button events to TUIs like
    /// helix, vim, less that consume mouse input.
    ///
    /// Args:
    ///   - col, row: 1-indexed terminal cell coordinates of the tap
    ///   - button: 0 = left, 1 = middle, 2 = right
    ///
    /// Returns concatenated press+release bytes, or empty if mouse
    /// reporting is off. Caller writes the result to PTY.
    ///
    /// In SGR (1006) encoding:
    ///   Press:   \x1B[<0;COL;ROW M     (capital M)
    ///   Release: \x1B[<0;COL;ROW m     (lowercase m)
    ///
    /// In X10 (default) encoding:
    ///   Press:   \x1B[M CB CX CY       (button code, x+32, y+32)
    ///   Release: \x1B[M CB CX CY       (button code = 3 = release-marker
    ///                                   in X10, ambiguous about which
    ///                                   button)
    ///
    /// The whole sequence is "tap" — both events fire at the same
    /// (col, row). Helix uses this to position the cursor; some
    /// programs (e.g. button-event-only mode 1000) discard releases.
    /// Always sending both is fine because reading the release-only
    /// is safe in modes that report only presses.
    pub fn encode_tap_event(&self, col: u16, row: u16, button: u8) -> Vec<u8> {
        let mut out = self.encode_press_event(col, row, button);
        out.extend_from_slice(&self.encode_release_event(col, row, button));
        out
    }

    /// v0.1.268 (drag-to-select): just the PRESS event. Used at
    /// the start of a drag, paired with motion events during, and
    /// a release at the end. Returns empty if mouse reporting off.
    pub fn encode_press_event(&self, col: u16, row: u16, button: u8) -> Vec<u8> {
        if self.mouse_mode == 0 { return Vec::new(); }
        let col = col.max(1);
        let row = row.max(1);
        let b: u8 = button.min(2);
        match self.mouse_protocol {
            // SGR (1006): button is naked; M = press.
            2 => format!("\x1B[<{};{};{}M", b, col, row).into_bytes(),
            // urxvt (1015): button + 32 with press marker.
            3 => format!("\x1B[{};{};{}M", b as u32 + 32, col, row).into_bytes(),
            // X10 / normal: button + 32.
            _ => {
                let cx = (col.min(223) as u8) + 32;
                let cy = (row.min(223) as u8) + 32;
                let cb_press = b + 32;
                vec![0x1B, b'[', b'M', cb_press, cx, cy]
            }
        }
    }

    /// v0.1.268 (drag-to-select): just the RELEASE event.
    pub fn encode_release_event(&self, col: u16, row: u16, button: u8) -> Vec<u8> {
        if self.mouse_mode == 0 { return Vec::new(); }
        let col = col.max(1);
        let row = row.max(1);
        let b: u8 = button.min(2);
        match self.mouse_protocol {
            // SGR (1006): button is naked; m = release.
            2 => format!("\x1B[<{};{};{}m", b, col, row).into_bytes(),
            // urxvt (1015): release-marker 3 + 32.
            3 => {
                let cb_release: u32 = 32 + 3;
                format!("\x1B[{};{};{}M", cb_release, col, row).into_bytes()
            }
            // X10 / normal: release = 3 + 32 (button id lost).
            _ => {
                let cx = (col.min(223) as u8) + 32;
                let cy = (row.min(223) as u8) + 32;
                let cb_release: u8 = 3 + 32;
                vec![0x1B, b'[', b'M', cb_release, cx, cy]
            }
        }
    }

    /// v0.1.268 (drag-to-select): MOTION event with a button held.
    /// Emitted while the user holds button-1 and drags. Helix uses
    /// these to extend a visual selection cell-by-cell.
    ///
    /// In mouse modes 1000 (button-event only), motion is NOT
    /// reported — returns empty. Only modes 1002 (cell-motion) and
    /// 1003 (any-event) deliver motion. Caller can probe via
    /// `mouse_mode` if it matters.
    ///
    /// SGR encoding for motion-with-button: button | 32 (the +32
    /// bit signals motion). So button-1 motion = 33 in SGR.
    pub fn encode_motion_event(&self, col: u16, row: u16, button: u8) -> Vec<u8> {
        if self.mouse_mode == 0 { return Vec::new(); }
        // Motion only reported in 1002/1003. In 1000 (button-event-
        // only), the TUI hasn't asked for motion and ignoring it
        // is the right move — emitting it could confuse some
        // programs.
        if self.mouse_mode == 1000 { return Vec::new(); }
        let col = col.max(1);
        let row = row.max(1);
        let b: u8 = button.min(2);
        // Motion modifier: button code OR'd with 32 (bit 5).
        let b_motion = b | 32;
        match self.mouse_protocol {
            // SGR (1006): emit motion-flagged button as press.
            2 => format!("\x1B[<{};{};{}M", b_motion, col, row).into_bytes(),
            // urxvt (1015): button + 32 + motion-bit + 32 base.
            3 => format!("\x1B[{};{};{}M", b_motion as u32 + 32, col, row).into_bytes(),
            // X10 / normal: button + 32, motion bit set.
            _ => {
                let cx = (col.min(223) as u8) + 32;
                let cy = (row.min(223) as u8) + 32;
                let cb = b_motion + 32;
                vec![0x1B, b'[', b'M', cb, cx, cy]
            }
        }
    }

    /// Return the plain-text content of a single grid row, with trailing
    /// blanks stripped. Used by track_pty_command to scrape the prompt line
    /// when the typed-buffer capture is shorter than the actual command
    /// (e.g. tab completion expanded what the user typed).
    pub fn row_plain_text(&self, row_idx: usize) -> Option<String> {
        if row_idx >= self.rows { return None; }
        let row = self.cells.get(row_idx)?;
        let s: String = row.iter().map(|c| c.ch).collect();
        Some(s.trim_end().to_string())
    }

    pub fn scrollback_len(&self) -> usize { self.scrollback.len() }

    /// v0.1.349: search scrollback + live cells for `query`. Returns
    /// matches as (abs_line, col_start, col_end, row_text) tuples,
    /// ordered top-to-bottom (oldest match first; reverse at the
    /// caller if you want newest-first).
    ///
    /// `abs_line` is the absolute line index, the same address the
    /// selection layer uses for its anchors:
    /// `abs_line = lines_emitted - rows - scrollback_pos + render_row`.
    /// Caller can compute the render_row to scroll to via
    /// `target_render_row = abs_line - (lines_emitted - rows - scrollback_pos)`.
    ///
    /// `case_sensitive=false` lowercases both query and row text
    /// before comparing. Matches are non-overlapping per row (after
    /// finding a match, search resumes from match_end).
    ///
    /// Empty query returns empty. Queries that don't match anywhere
    /// return empty.
    ///
    /// Phantom cells (right half of wide chars) are skipped during
    /// the row-to-string conversion, matching what's displayed; col
    /// indices in returned matches refer to GLYPH columns (i.e. the
    /// position in the rendered string), not raw cell positions.
    /// For most uses this matches what the user expects.
    ///
    /// Alt-screen content is intentionally excluded: TUIs like vim
    /// or less are responsible for their own internal search, and
    /// scrollback search snooping inside their alt-screen would be
    /// surprising.
    pub fn search_scrollback(
        &self,
        query: &str,
        case_sensitive: bool,
    ) -> Vec<(u64, usize, usize, String)> {
        if query.is_empty() { return Vec::new(); }
        if self.alt_cells.is_some() { return Vec::new(); }
        let needle = if case_sensitive { query.to_string() } else { query.to_lowercase() };
        let mut results: Vec<(u64, usize, usize, String)> = Vec::new();

        // Helper: search one row, push hits.
        // abs_idx is the absolute line index of this row.
        let row_text = |row: &[Cell]| -> String {
            row.iter()
                .filter(|c| !c.phantom())
                .map(|c| c.ch)
                .collect::<String>()
        };
        let push_matches_for = |abs_idx: u64,
                                text: String,
                                results: &mut Vec<(u64, usize, usize, String)>|
        {
            let hay: String = if case_sensitive { text.clone() } else { text.to_lowercase() };
            let mut start = 0;
            while let Some(off) = hay[start..].find(&needle) {
                let s = start + off;
                let e = s + needle.len();
                results.push((abs_idx, s, e, text.clone()));
                start = e;
                if start >= hay.len() { break; }
            }
        };

        // The scrollback Vec is indexed [0..scrollback.len()). The
        // absolute line index of scrollback[i] is:
        //   abs = lines_emitted - rows - (scrollback.len() - i)
        //       = lines_emitted - rows - scrollback.len() + i
        // (Because lines_emitted counts lines that have been pushed
        // off the top; scrollback holds the most recent N of them;
        // the OLDEST scrollback line is at index 0 and its abs index
        // is the smallest.)
        let rows_u64 = self.rows as u64;
        let sb_len = self.scrollback.len();
        let scrollback_base_abs: u64 = self
            .lines_emitted
            .saturating_sub(rows_u64)
            .saturating_sub(sb_len as u64);

        for (i, row) in self.scrollback.iter().enumerate() {
            let abs = scrollback_base_abs + i as u64;
            let text = row_text(row);
            push_matches_for(abs, text, &mut results);
        }

        // Live cells. The first live row's abs index is
        // lines_emitted - rows (i.e. just past the last scrollback line).
        let live_base_abs: u64 = self.lines_emitted.saturating_sub(rows_u64);
        for (i, row) in self.cells.iter().enumerate() {
            let abs = live_base_abs + i as u64;
            let text = row_text(row);
            push_matches_for(abs, text, &mut results);
        }

        results
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let blank = self.blank_with_bg();
        self.cols = cols; self.rows = rows;
        self.cells.resize(rows, vec![blank; cols]);
        for row in &mut self.cells { row.resize(cols, blank); }
        // Rebuild tab stops for new width
        let mut tabs = vec![false; cols.max(1)];
        let mut i = 8;
        while i < cols { tabs[i] = true; i += 8; }
        self.tab_stops = tabs;
        self.scroll_top = 0; self.scroll_bot = rows.saturating_sub(1);
        if self.cursor_row >= rows { self.cursor_row = rows - 1; }
        if self.cursor_col >= cols { self.cursor_col = cols - 1; }
        self.wrap_pending = false;
        self.dirty = true; // geometry changed → must re-render
        self.mark_all_dirty();
    }
}

impl vte::Perform for TermGrid {
    fn print(&mut self, c: char) {
        // Apply active charset translation
        let charset = if self.charset_active == 0 { self.charset_g0 } else { self.charset_g1 };
        let translated = translate_charset(c, charset);

        // Pending wrap: cursor was parked at last col after previous print
        if self.wrap_pending {
            self.wrap_pending = false;
            if self.autowrap {
                self.cursor_col = 0;
                self.cursor_row += 1;
                if self.cursor_row > self.scroll_bot { self.scroll_up(); self.cursor_row = self.scroll_bot; }
            }
            // If autowrap off, we keep overwriting last column
        }

        if self.cursor_row >= self.rows { return; }
        if self.cols == 0 { return; }
        // Defensive: if the row vec is shorter than cols (resize race), grow it
        if self.cells[self.cursor_row].len() < self.cols {
            let blank = self.blank_with_bg();
            self.cells[self.cursor_row].resize(self.cols, blank);
        }
        let col = self.cursor_col.min(self.cols - 1).min(self.cells[self.cursor_row].len() - 1);

        // Determine display width of the character. CJK ideographs,
        // emoji, fullwidth forms etc. take 2 cells. ASCII and most
        // Latin/Cyrillic/Greek take 1. Combining marks (zero width)
        // we treat as 1 here — proper combining-mark handling would
        // need a separate "fold into previous cell" path that we
        // don't implement yet (rare in terminal output anyway).
        use unicode_width::UnicodeWidthChar;
        let cw = translated.width().unwrap_or(1).max(1);

        if self.insert_mode {
            // Push cells right
            let row = &mut self.cells[self.cursor_row];
            row.insert(col, Cell { ch: translated, fg: self.cur_fg, bg: self.cur_bg, attrs: self.cur_attrs });
            if cw == 2 && col + 1 < self.cols {
                // Insert phantom for wide char's right half
                row.insert(col + 1, Cell {
                    ch: ' ',
                    fg: self.cur_fg,
                    bg: self.cur_bg,
                    attrs: self.cur_attrs | ATTR_PHANTOM,
                });
            }
            row.truncate(self.cols);
        } else {
            // Bounds-safe (col already clamped to row.len()-1 above)
            if col < self.cells[self.cursor_row].len() {
                self.cells[self.cursor_row][col] = Cell {
                    ch: translated, fg: self.cur_fg, bg: self.cur_bg, attrs: self.cur_attrs
                };
                // Wide char: mark next cell as phantom (right half of glyph)
                if cw == 2 && col + 1 < self.cells[self.cursor_row].len() {
                    self.cells[self.cursor_row][col + 1] = Cell {
                        ch: ' ',
                        fg: self.cur_fg,
                        bg: self.cur_bg,
                        attrs: self.cur_attrs | ATTR_PHANTOM,
                    };
                }
            }
        }

        // Advance cursor by the display width. For wide chars we move
        // 2 columns; for normal 1. Wrap logic checks against cols.
        let advance = cw.max(1);
        if self.cursor_col + advance >= self.cols {
            self.wrap_pending = true;
            // Park at last col (don't overshoot)
            self.cursor_col = self.cols.saturating_sub(1);
        } else {
            self.cursor_col += advance;
        }
        // v0.1.494: precise per-row dirty mark for the hot path. The
        // write above (insert or overwrite) touched cursor_row.
        let cr = self.cursor_row;
        self.mark_row_dirty(cr);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b /* VT */ | 0x0c /* FF */ => {
                self.wrap_pending = false;
                if self.cursor_row == self.scroll_bot { self.scroll_up(); }
                else if self.cursor_row < self.rows - 1 { self.cursor_row += 1; }
            }
            b'\r' => { self.cursor_col = 0; self.wrap_pending = false; }
            b'\t' => { self.tab_forward(); self.wrap_pending = false; }
            0x08 => { // Backspace
                if self.wrap_pending { self.wrap_pending = false; }
                else if self.cursor_col > 0 { self.cursor_col -= 1; }
            }
            0x07 => {} // Bell
            0x0e => { self.charset_active = 1; } // SO — Shift Out (G1)
            0x0f => { self.charset_active = 0; } // SI — Shift In (G0)
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _ignore: bool, action: char) {
        let p: Vec<u16> = params.iter().flat_map(|s| s.iter().copied()).collect();
        let p1 = *p.first().unwrap_or(&1) as usize;
        let p2 = *p.get(1).unwrap_or(&1) as usize;
        let has_q = intermediates.contains(&b'?');

        // v0.1.453: defensive clamp. Several handlers below index
        // self.cells[self.cursor_row] directly (J, K, P, @, X, …).
        // A malformed/edge sequence or a resize race could leave
        // cursor_row == self.rows (one past the last valid index),
        // which panicked: "index out of bounds: len is 26, index
        // is 26" at the 'J' handler when helix quit. A terminal
        // emulator must never crash on input, so we re-establish
        // the cursor_row-in-bounds invariant once, here, covering
        // every handler instead of guarding each indexing site.
        // Clamp against the ACTUAL grid height (self.cells.len()),
        // not just self.rows, in case the two ever desync.
        //
        // v0.1.487: ALSO re-sync the grid height itself. The crash
        // recurred at 'J' (len 22, index 22) because that handler
        // looped to self.rows while indexing self.cells, and rows >
        // cells.len() during an alt-screen teardown race. Rather
        // than only patch each loop, restore the cells.len()==rows
        // invariant here so every handler can trust it.
        let blank_fill = self.blank_with_bg();
        if self.cells.len() != self.rows {
            self.cells.resize(self.rows, vec![blank_fill; self.cols]);
        }
        let grid_h = self.cells.len();
        if grid_h == 0 { return; }
        if self.cursor_row >= grid_h {
            self.cursor_row = grid_h - 1;
        }

        match action {
            'A' => {
                let n = p1.max(1);
                let min = if self.origin_mode { self.scroll_top } else { 0 };
                self.cursor_row = self.cursor_row.saturating_sub(n).max(min);
                self.wrap_pending = false;
            }
            'B' => {
                let n = p1.max(1);
                let max = if self.origin_mode { self.scroll_bot } else { self.rows - 1 };
                self.cursor_row = (self.cursor_row + n).min(max);
                self.wrap_pending = false;
            }
            'C' => { self.cursor_col = (self.cursor_col + p1.max(1)).min(self.cols - 1); self.wrap_pending = false; }
            'D' => { self.cursor_col = self.cursor_col.saturating_sub(p1.max(1)); self.wrap_pending = false; }
            'E' => { self.cursor_row = (self.cursor_row + p1.max(1)).min(self.rows - 1); self.cursor_col = 0; self.wrap_pending = false; }
            'F' => { self.cursor_row = self.cursor_row.saturating_sub(p1.max(1)); self.cursor_col = 0; self.wrap_pending = false; }
            'G' => { self.cursor_col = (p1.max(1) - 1).min(self.cols - 1); self.wrap_pending = false; }
            'H' | 'f' => {
                let row = (p1.max(1) - 1).min(self.rows - 1);
                self.cursor_row = self.apply_row(row);
                self.cursor_col = (p2.max(1) - 1).min(self.cols - 1);
                self.wrap_pending = false;
            }
            'd' => {
                let row = (p1.max(1) - 1).min(self.rows - 1);
                self.cursor_row = self.apply_row(row);
                self.wrap_pending = false;
            }
            // CSI I — Cursor Forward Tabulation (CHT): tab forward N stops
            'I' => { for _ in 0..p1.max(1) { self.tab_forward(); } }
            // CSI Z — Cursor Backward Tabulation (CBT): tab backward N stops
            'Z' => { self.tab_backward(p1.max(1)); }
            // v0.1.418: CSI n — Device Status Report (DSR). Tools
            // query the terminal and BLOCK for the reply; we push
            // it to response_buf which the PTY loop writes back.
            //   - DSR 6 → Report Cursor Position: reply
            //     ESC [ <row> ; <col> R  (1-based).
            //   - DSR 5 → Report device status: reply ESC [ 0 n
            //     ("terminal OK").
            // Handles both standard (CSI 6 n) and DEC-private
            // (CSI ? 6 n) forms — some tools send the private
            // variant; the reply format is the same.
            'n' => {
                let req = *p.first().unwrap_or(&0);
                match req {
                    6 => {
                        let row = self.cursor_row + 1;
                        let col = self.cursor_col + 1;
                        let reply = format!("\x1b[{};{}R", row, col);
                        self.response_buf.extend_from_slice(reply.as_bytes());
                    }
                    5 => {
                        self.response_buf.extend_from_slice(b"\x1b[0n");
                    }
                    _ => {}
                }
            }
            // v0.1.784: CSI c — Device Attributes. crossterm's
            // supports_keyboard_enhancement() (called on startup by gitui, helix,
            // and other ratatui/crossterm TUIs) sends CSI ? u THEN CSI c and
            // BLOCKS waiting for the DA1 reply as a sync marker. With no reply it
            // stalls ~2-3s before falling back — the "3 second lock" both apps hit.
            //   - DA1 (CSI c / CSI 0 c): reply ESC [ ? 1 ; 2 c (VT100 + Advanced
            //     Video). Content is irrelevant to crossterm; kept conservative so
            //     apps don't assume features we lack. NOT replying to CSI ? u is
            //     correct — it means "no kitty keyboard support", which is true.
            //   - DA2 (CSI > c): reply ESC [ > 0 ; 276 ; 0 c (VT100-ish, firmware).
            //   - DA3 (CSI = c): left unanswered (rare; no known blocker).
            'c' => {
                if intermediates.contains(&b'>') {
                    self.response_buf.extend_from_slice(b"\x1b[>0;276;0c");
                } else if !intermediates.contains(&b'=') {
                    self.response_buf.extend_from_slice(b"\x1b[?1;2c");
                }
            }
            'J' => {
                let mode = *p.first().unwrap_or(&0);
                let blank = self.blank_with_bg();
                // v0.1.487: bound these loops on the ACTUAL grid
                // height (self.cells.len()), NOT self.rows. The
                // v0.1.453 clamp fixed cursor_row, but these loops
                // indexed self.cells[r] while iterating up to
                // self.rows — and rows can exceed cells.len() during
                // a resize/alt-screen-teardown race (helix :q), which
                // panicked here: "len is 22, index is 22". A terminal
                // emulator must never crash on input.
                let grid_h = self.cells.len();
                match mode {
                    0 => { if self.cursor_row < grid_h {
                            for c in self.cursor_col..self.cols { self.cells[self.cursor_row][c] = blank; } }
                        for r in (self.cursor_row+1)..grid_h { self.cells[r] = vec![blank; self.cols]; } }
                    1 => { for r in 0..self.cursor_row.min(grid_h) { self.cells[r] = vec![blank; self.cols]; }
                        if self.cursor_row < grid_h {
                            for c in 0..=self.cursor_col.min(self.cols-1) { self.cells[self.cursor_row][c] = blank; } } }
                    2|3 => { for r in 0..grid_h { self.cells[r] = vec![blank; self.cols]; } }
                    _ => {}
                }
            }
            'K' => {
                let mode = *p.first().unwrap_or(&0);
                let blank = self.blank_with_bg();
                match mode {
                    0 => { for c in self.cursor_col..self.cols { self.cells[self.cursor_row][c] = blank; } }
                    1 => { for c in 0..=self.cursor_col.min(self.cols-1) { self.cells[self.cursor_row][c] = blank; } }
                    2 => { self.cells[self.cursor_row] = vec![blank; self.cols]; }
                    _ => {}
                }
            }
            'L' => { let n = p1.max(1); let blank = self.blank_with_bg(); for _ in 0..n { if self.cursor_row <= self.scroll_bot { self.cells.insert(self.cursor_row, vec![blank; self.cols]); if self.cells.len() > self.rows { self.cells.remove(self.scroll_bot + 1); } } } }
            'M' => { let n = p1.max(1); let blank = self.blank_with_bg(); for _ in 0..n { if self.cursor_row < self.cells.len() { self.cells.remove(self.cursor_row); self.cells.insert(self.scroll_bot.min(self.rows-1), vec![blank; self.cols]); } } }
            'P' => { let n = p1.max(1).min(self.cols - self.cursor_col); let blank = self.blank_with_bg(); let row = &mut self.cells[self.cursor_row]; for _ in 0..n { if self.cursor_col < row.len() { row.remove(self.cursor_col); } } row.resize(self.cols, blank); }
            '@' => { let n = p1.max(1).min(self.cols - self.cursor_col); let blank = self.blank_with_bg(); let row = &mut self.cells[self.cursor_row]; for _ in 0..n { row.insert(self.cursor_col, blank); } row.truncate(self.cols); }
            'X' => { let n = p1.max(1); let blank = self.blank_with_bg(); for i in 0..n { let c = self.cursor_col + i; if c < self.cols { self.cells[self.cursor_row][c] = blank; } } }
            'S' => { for _ in 0..p1.max(1) { self.scroll_up(); } }
            'T' => { for _ in 0..p1.max(1) { self.scroll_down(); } }
            'r' => { // DECSTBM — set scrolling region
                let top = (p1.max(1) - 1).min(self.rows - 1);
                let bot_raw = if p.len() >= 2 && p[1] != 0 {
                    (p[1] as usize).min(self.rows) - 1
                } else { self.rows - 1 };
                let bot = bot_raw.max(top);
                self.scroll_top = top; self.scroll_bot = bot;
                // Cursor moves to home — respecting origin mode
                self.cursor_row = if self.origin_mode { self.scroll_top } else { 0 };
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            'm' => { // SGR
                if p.is_empty() { self.cur_fg = COLOR_DEFAULT; self.cur_bg = COLOR_DEFAULT; self.cur_attrs = 0; return; }
                let mut i = 0;
                while i < p.len() {
                    match p[i] {
                        0  => { self.cur_fg = COLOR_DEFAULT; self.cur_bg = COLOR_DEFAULT; self.cur_attrs = 0; }
                        1  => { self.cur_attrs |= ATTR_BOLD; }
                        2  => { self.cur_attrs |= ATTR_DIM; }
                        3  => { self.cur_attrs |= ATTR_ITALIC; }
                        4  => { self.cur_attrs |= ATTR_UNDER; }
                        7  => { self.cur_attrs |= ATTR_REV; }
                        9  => { self.cur_attrs |= ATTR_STRIKE; }
                        22 => { self.cur_attrs &= !(ATTR_BOLD | ATTR_DIM); }
                        23 => { self.cur_attrs &= !ATTR_ITALIC; }
                        24 => { self.cur_attrs &= !ATTR_UNDER; }
                        27 => { self.cur_attrs &= !ATTR_REV; }
                        29 => { self.cur_attrs &= !ATTR_STRIKE; }
                        30..=37 => { self.cur_fg = (p[i] - 30) as u16; }
                        38 => {
                            if i+2 < p.len() && p[i+1] == 5 { self.cur_fg = p[i+2] as u16; i += 2; }
                            else if i+4 < p.len() && p[i+1] == 2 {
                                // 24-bit truecolor encoded as fg = 256 + (r<<16 | g<<8 | b) packed in u32
                                // We store the actual RGB index in fg via lookup table extension.
                                // For simplicity: pack RGB into u16 by quantizing to 6x6x6 (256-color cube)
                                let r = p[i+2] as u8; let g = p[i+3] as u8; let b = p[i+4] as u8;
                                self.cur_fg = rgb_to_256(r, g, b) as u16;
                                i += 4;
                            }
                        }
                        39 => { self.cur_fg = COLOR_DEFAULT; }
                        40..=47 => { self.cur_bg = (p[i] - 40) as u16; }
                        48 => {
                            if i+2 < p.len() && p[i+1] == 5 { self.cur_bg = p[i+2] as u16; i += 2; }
                            else if i+4 < p.len() && p[i+1] == 2 {
                                let r = p[i+2] as u8; let g = p[i+3] as u8; let b = p[i+4] as u8;
                                self.cur_bg = rgb_to_256(r, g, b) as u16;
                                i += 4;
                            }
                        }
                        49 => { self.cur_bg = COLOR_DEFAULT; }
                        90..=97  => { self.cur_fg = (p[i] - 90 + 8) as u16; }
                        100..=107 => { self.cur_bg = (p[i] - 100 + 8) as u16; }
                        _ => {}
                    }
                    i += 1;
                }
            }
            'h' => {
                if has_q {
                    // DEC private modes — set
                    for &mode in p.iter() {
                        match mode {
                            1    => { self.app_cursor = true; }       // DECCKM — application cursor keys
                            6    => { self.origin_mode = true;        // DECOM — origin mode
                                       self.cursor_row = self.scroll_top; self.cursor_col = 0; }
                            7    => { self.autowrap = true; }          // DECAWM
                            25   => { self.cursor_visible = true; }    // DECTCEM
                            47 | 1047 | 1049 => { self.enter_alt(); }
                            66   => { self.app_keypad = true; }        // DECNKM
                            1000 => { self.mouse_mode = 1000; }        // X11 mouse: button events
                            1002 => { self.mouse_mode = 1002; }        // Cell-motion mouse tracking
                            1003 => { self.mouse_mode = 1003; }        // Any-event mouse tracking
                            1005 => { self.mouse_protocol = 1; }       // UTF-8 mouse encoding
                            1006 => { self.mouse_protocol = 2; }       // SGR mouse encoding
                            1015 => { self.mouse_protocol = 3; }       // urxvt mouse encoding
                            2004 => { self.bracketed_paste = true; }
                            _ => {}
                        }
                    }
                } else {
                    // ANSI modes — set
                    for &mode in p.iter() {
                        match mode {
                            4 => { self.insert_mode = true; }   // IRM
                            20 => {}                            // LNM — auto linefeed
                            _ => {}
                        }
                    }
                }
            }
            'l' => {
                if has_q {
                    // DEC private modes — reset
                    for &mode in p.iter() {
                        match mode {
                            1    => { self.app_cursor = false; }
                            6    => { self.origin_mode = false;
                                       self.cursor_row = 0; self.cursor_col = 0; }
                            7    => { self.autowrap = false; }
                            25   => { self.cursor_visible = false; }
                            47 | 1047 | 1049 => { self.exit_alt(); }
                            66   => { self.app_keypad = false; }
                            1000 | 1002 | 1003 => { self.mouse_mode = 0; }
                            1005 | 1006 | 1015 => { self.mouse_protocol = 0; }
                            2004 => { self.bracketed_paste = false; }
                            _ => {}
                        }
                    }
                } else {
                    for &mode in p.iter() {
                        match mode {
                            4 => { self.insert_mode = false; }
                            _ => {}
                        }
                    }
                }
            }
            // CSI g — tab clear (TBC)
            'g' => {
                let mode = *p.first().unwrap_or(&0);
                match mode {
                    0 => { // Clear tab at cursor
                        if let Some(slot) = self.tab_stops.get_mut(self.cursor_col) { *slot = false; }
                    }
                    3 => { // Clear all tabs
                        for slot in self.tab_stops.iter_mut() { *slot = false; }
                    }
                    _ => {}
                }
            }
            // CSI s — save cursor (SCO variant of DECSC)
            's' => { self.save_cursor(); }
            // CSI u — restore cursor (SCO variant of DECRC)
            'u' => { self.restore_cursor(); }
            ' ' if action == 'q' => {}
            _ => {
                if action == 'q' && !intermediates.is_empty() && intermediates[0] == b' ' {
                    self.cursor_shape = match p1 { 0|1|2 => 0, 3|4 => 2, 5|6 => 1, _ => 0 };
                }
            }
        }
        // v0.1.494: conservative per-row dirty for CONTENT-mutating CSI
        // ops (erase, line insert/delete, char insert/delete/erase).
        // These touch ranges we don't track precisely, so mark all —
        // correct-by-construction (never stale). Listed explicitly so
        // cursor-only / mode-only CSIs (the majority) DON'T over-mark.
        // print() handles the precise hot path; this covers the rest.
        match action {
            'J' | 'K' | 'L' | 'M' | 'P' | '@' | 'X' => self.mark_all_dirty(),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        // ESC ( <char> — designate G0 charset
        // ESC ) <char> — designate G1 charset
        if intermediates.len() == 1 {
            match (intermediates[0], byte) {
                (b'(', b'0') => { self.charset_g0 = Charset::DecLineDrawing; return; }
                (b'(', b'B') | (b'(', b'A') => { self.charset_g0 = Charset::Ascii; return; }
                (b')', b'0') => { self.charset_g1 = Charset::DecLineDrawing; return; }
                (b')', b'B') | (b')', b'A') => { self.charset_g1 = Charset::Ascii; return; }
                _ => {}
            }
        }
        match byte {
            b'7' => { self.save_cursor(); }                 // DECSC
            b'8' => { self.restore_cursor(); }              // DECRC
            b'=' => { self.app_keypad = true; }             // DECKPAM
            b'>' => { self.app_keypad = false; }            // DECKPNM
            b'H' => {                                       // HTS — set tab stop at cursor
                if let Some(slot) = self.tab_stops.get_mut(self.cursor_col) { *slot = true; }
            }
            b'M' => { // RI — Reverse index
                self.wrap_pending = false;
                if self.cursor_row == self.scroll_top { self.scroll_down(); }
                else if self.cursor_row > 0 { self.cursor_row -= 1; }
            }
            b'D' => { // IND — Index
                self.wrap_pending = false;
                if self.cursor_row == self.scroll_bot { self.scroll_up(); }
                else if self.cursor_row < self.rows - 1 { self.cursor_row += 1; }
            }
            b'E' => { // NEL — Next line
                self.wrap_pending = false;
                self.cursor_col = 0;
                if self.cursor_row == self.scroll_bot { self.scroll_up(); }
                else if self.cursor_row < self.rows - 1 { self.cursor_row += 1; }
            }
            b'c' => { // RIS — full reset
                let cols = self.cols; let rows = self.rows;
                let scrollback = std::mem::take(&mut self.scrollback);
                *self = TermGrid::new(cols, rows);
                self.scrollback = scrollback;
            }
            _ => {}
        }
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() { return; }
        let cmd = params[0];
        match cmd {
            // OSC 0: set icon name + title, OSC 2: set title
            b"0" | b"2" => {
                if params.len() >= 2 {
                    self.title = String::from_utf8_lossy(params[1]).to_string();
                }
            }
            // OSC 52: clipboard — we can't access system clipboard from here but log it
            b"52" => {}
            // v0.1.883: OSC 133 shell integration (FinalTerm/iTerm2). The
            // `D;<code>` marker reports the previous command's exit status;
            // capture it so notifications can show ✓/✗. A/B/C lifecycle
            // markers carry no code and are ignored here.
            b"133" => {
                if let Some(code) = parse_osc133_exit(&params[1..]) {
                    self.osc133_exit = Some(code);
                }
            }
            _ => {}
        }
    }
    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn unhook(&mut self) {}
    fn put(&mut self, _byte: u8) {}
}

/// Parse the params of an OSC 133 shell-integration sequence (the slice
/// AFTER the leading `"133"`). Returns `Some(code)` only for a command-
/// finished marker `D;<code>` with a numeric exit code; returns `None`
/// for the A/B/C lifecycle markers, for a bare `D` with no code, and for
/// anything unparseable. OSC params are `;`-split by the VTE parser, so
/// `OSC 133 ; D ; 0` arrives as params `[b"133", b"D", b"0"]` and `rest`
/// here is `[b"D", b"0"]`.
fn parse_osc133_exit(rest: &[&[u8]]) -> Option<i32> {
    let marker = rest.first().copied()?;
    if marker != b"D".as_slice() {
        return None;
    }
    let code = rest.get(1).copied()?;
    std::str::from_utf8(code).ok()?.trim().parse::<i32>().ok()
}

/// Map 24-bit RGB to nearest 256-color index
fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    // Check if it's close to a standard color (0-15)
    if r < 8 && g < 8 && b < 8 { return 0; }
    if r > 248 && g > 248 && b > 248 { return 15; }
    // Map to 6x6x6 color cube (indices 16-231)
    let ri = if r > 47 { (r - 35) / 40 } else { 0 };
    let gi = if g > 47 { (g - 35) / 40 } else { 0 };
    let bi = if b > 47 { (b - 35) / 40 } else { 0 };
    16 + 36 * ri.min(5) + 6 * gi.min(5) + bi.min(5)
}

/// v0.1.490 (Option B — full decouple): a consistent, self-contained
/// snapshot of the terminal's renderable state, produced BY THE
/// READER THREAD after parsing and published into a slot the UI reads
/// without ever touching the grid lock. The UI's read is just an
/// Arc::clone (a pointer bump), so it never waits on the parser —
/// the catch-up is fully hidden from UI smoothness.
#[derive(Default)]
pub struct RenderSnapshot {
    pub html: String,
    /// v0.1.902: packed-runs wire format for the native Canvas renderer
    /// (see `TermGrid::screen_runs_packed`), built off the UI thread
    /// exactly like `html` above. Empty when native rendering is off
    /// (`WEB2QML_HTML_TERM=1`) — mirrors how `html` is empty when native
    /// IS on. `term_screen_runs` reads this via a cheap Arc::clone instead
    /// of locking the grid on the UI thread.
    pub runs: String,
    pub cursor_row: i32,
    pub cursor_col: i32,
    pub scrollback_len: i32,
    pub scrollback_pos: i32,
    pub title: String,
    /// Monotonic generation; the UI uses it to detect "new frame
    /// available" without comparing the (large) html string.
    pub generation: u64,
}

pub struct PtyTerminal {
    pub grid: Arc<Mutex<TermGrid>>,
    /// v0.1.490: the latest finished render, published by the reader
    /// thread. UI reads via a cheap Arc::clone — no grid-lock wait.
    render_slot: Arc<Mutex<Arc<RenderSnapshot>>>,
    /// v0.1.491: SINGLE source of frame generations, shared by the
    /// reader thread and the main-thread republish path. Both draw a
    /// strictly-increasing number from here via fetch_add, so two
    /// publishers can never collide on a generation (the v0.1.490
    /// race, now closed). The UI compares this against the last
    /// applied generation to detect a new frame.
    render_gen: Arc<std::sync::atomic::AtomicU64>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master_fd: Option<i32>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    #[allow(dead_code)]
    child: Box<dyn portable_pty::Child + Send + Sync>,
    output_tap: Arc<Mutex<Vec<u8>>>,
    /// Secondary tap activated only during a completion-query window.
    /// When `Some(buf)`, the reader thread ALSO appends incoming bytes to
    /// `buf` (in addition to the main tap and the grid). When `None`, no
    /// completion capture is happening. The completion query orchestrator
    /// activates this, sends `<partial><TAB>`, waits ~150ms for shell to
    /// respond, deactivates, parses what was captured, then sends Ctrl-U
    /// to clean the prompt.
    ///
    /// We do this as a separate buffer rather than draining the main tap
    /// because the main tap is consumed by block_tracker on a regular
    /// timer — we'd race for the bytes we want.
    completion_capture: Arc<Mutex<Option<Vec<u8>>>>,
}

impl PtyTerminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, Box<dyn std::error::Error>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.arg("-l");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let master_fd = pair.master.as_raw_fd();
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        // v0.1.709: wrap the writer in its Arc<Mutex> NOW (was created inline in
        // Self{} below) so the reader thread can hold a clone and flush queued
        // terminal-query responses (DSR/DA1) IMMEDIATELY after parsing, instead
        // of waiting up to one UI poll (~50ms) for drain_responses(). TUIs like
        // helix block on these query round-trips at startup; the poll-latency
        // was the "launches slow vs konsole" sluggishness.
        let writer_arc: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let writer_reader = Arc::clone(&writer_arc);
        let grid = Arc::new(Mutex::new(TermGrid::new(cols as usize, rows as usize)));
        let gc = Arc::clone(&grid);
        // Tap: raw output bytes accumulate here until drained by block tracker
        let output_tap: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::with_capacity(8192)));
        let tap_clone = Arc::clone(&output_tap);
        // Completion capture: when activated, the reader appends bytes here
        // too. None means inactive (the common case).
        let completion_capture: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let cc_clone = Arc::clone(&completion_capture);
        // v0.1.490: the render slot the reader publishes finished
        // snapshots into; the UI reads via Arc::clone (no grid wait).
        let render_slot: Arc<Mutex<Arc<RenderSnapshot>>> =
            Arc::new(Mutex::new(Arc::new(RenderSnapshot::default())));
        let render_slot_w = Arc::clone(&render_slot);
        // v0.1.491: shared generation source (replaces the reader's
        // private counter + republish's slot-derived counter, which
        // could collide). Both publishers fetch_add from this.
        let render_gen: Arc<std::sync::atomic::AtomicU64> =
            Arc::new(std::sync::atomic::AtomicU64::new(0));
        let render_gen_w = Arc::clone(&render_gen);
        thread::spawn(move || {
            let mut parser = vte::Parser::new();
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // v0.1.434: time the parse of this chunk
                        // when profiling. A Helix startup floods
                        // the PTY with a full-screen redraw; if the
                        // latency is in our parse+grid path, slow
                        // bursts show here (grid lock held during
                        // parse). Compare against altscreen:enter
                        // timing to localize the cost.
                        let t0 = if profile_on() { Some(std::time::Instant::now()) } else { None };
                        // v0.1.489: parse in sub-chunks, releasing the
                        // grid lock between each, so the UI thread can
                        // acquire it in the gaps. Previously a big
                        // burst (helix full-screen redraw, fast build
                        // log) held the lock for the ENTIRE n-byte
                        // chunk, blocking the UI tick on lock
                        // acquisition — the visible stutter. Now the
                        // longest the UI can wait is one sub-chunk's
                        // parse, not the whole flood. 4 KiB balances
                        // lock-churn against responsiveness.
                        const PARSE_CHUNK: usize = 4096;
                        let mut off = 0;
                        let mut resp: Vec<u8> = Vec::new();
                        while off < n {
                            let endb = (off + PARSE_CHUNK).min(n);
                            let mut g = gc.lock().unwrap();
                            for &b in &buf[off..endb] { parser.advance(&mut *g, b); }
                            g.dirty = true;
                            // v0.1.709: collect any query responses the grid
                            // queued (DSR/DA1/etc.) while parsing this sub-chunk
                            // so we can write them back without waiting for the
                            // UI poll.
                            if !g.response_buf.is_empty() {
                                resp.extend_from_slice(&std::mem::take(&mut g.response_buf));
                            }
                            drop(g); // release between sub-chunks — UI can interleave
                            off = endb;
                        }
                        // flush queued query replies right now — TUIs block on
                        // these at startup; poll-latency was the slow-launch.
                        if !resp.is_empty() {
                            if let Ok(mut w) = writer_reader.lock() {
                                let _ = w.write_all(&resp);
                                let _ = w.flush();
                            }
                        }
                        if let Some(t0) = t0 {
                            let dt = t0.elapsed();
                            // Only log non-trivial bursts to avoid
                            // spamming on every keystroke echo.
                            if dt.as_micros() > 500 {
                                crate::clog!("profile", "pty:parse {} bytes in {:?}", n, dt);
                            }
                        }
                        // Append to tap, capped at 256KB to avoid runaway memory
                        if let Ok(mut tap) = tap_clone.lock() {
                            if tap.len() < 256 * 1024 {
                                tap.extend_from_slice(&buf[..n]);
                            }
                        }
                        // v0.1.565: opt-in raw-byte capture (WEB2QML_TAP=1)
                        // → /tmp/web2qml-tap.log, for diagnosing render
                        // glitches from the exact program output. Gated +
                        // best-effort; no effect on normal runs.
                        if tap_dump_on() {
                            tap_dump_write(&buf[..n]);
                        }
                        // Mirror to completion capture if active. Bounded
                        // separately at 64KB — completion output is small
                        // and if we got that much something is wrong.
                        if let Ok(mut cc) = cc_clone.lock() {
                            if let Some(cap) = cc.as_mut() {
                                if cap.len() < 64 * 1024 {
                                    cap.extend_from_slice(&buf[..n]);
                                }
                            }
                        }
                        // v0.1.490 (Option B): render on THIS (reader)
                        // thread and publish a finished snapshot. The
                        // expensive render_html happens here, off the UI
                        // thread; the UI later reads the snapshot with a
                        // cheap Arc::clone and never blocks on the grid
                        // lock. We re-lock the grid briefly to render —
                        // but that's the reader's own lock cycle; the UI
                        // is decoupled because it reads render_slot, not
                        // the grid. Only render when dirty (the idle
                        // skip from v0.1.488 still applies).
                        // v0.1.607: COALESCE renders during an output
                        // burst. A full-screen redraw (helix open, syntax
                        // repaint, fast build log) arrives as MULTIPLE
                        // read() chunks; rendering after each one
                        // serialized the whole grid + woke the UI to
                        // re-parse a full HTML doc N times, when only the
                        // SETTLED state matters — the visible "lackluster"
                        // burst cost. If this read filled the entire
                        // buffer, more bytes are almost certainly queued,
                        // so SKIP the render this iteration and loop to
                        // drain them; `dirty` stays set. The render then
                        // happens on the read that DOESN'T fill the buffer
                        // (burst settled). Safety net: if no further read
                        // comes, the UI's 50ms pty_read_screen poll sees
                        // is_dirty() and republishes — so a render is
                        // deferred by at most one poll (~50ms), never lost.
                        let burst_likely_continuing = n == buf.len();
                        let snap_opt = if burst_likely_continuing {
                            None // defer render until the burst settles
                        } else {
                            let mut g = gc.lock().unwrap();
                            if g.take_dirty() {
                                // v0.1.491: draw from the shared atomic
                                // so reader + republish never collide.
                                let gen = render_gen_w.fetch_add(
                                    1, std::sync::atomic::Ordering::SeqCst) + 1;
                                // v0.1.627: time render_html separately
                                // from pty:parse so the profile log shows
                                // WHERE helix-open / pager-scroll cost goes
                                // (parse vs render). Gated on WEB2QML_PROFILE.
                                let rt0 = if profile_on() { Some(std::time::Instant::now()) } else { None };
                                // v0.1.788: native renderer is default → skip the
                                // HTML build entirely (the Canvas paints from the
                                // grid). WEB2QML_HTML_TERM=1 restores it.
                                let native = native_render();
                                let html = if native { String::new() } else { g.render_html() };
                                // v0.1.902: mirror image of the html gate above —
                                // build the packed-runs snapshot off-thread when
                                // native IS on, so term_screen_runs (called every
                                // UI poll tick) never has to lock the grid itself.
                                let runs = if native { g.screen_runs_packed() } else { String::new() };
                                if let Some(rt0) = rt0 {
                                    let dt = rt0.elapsed();
                                    if dt.as_micros() > 500 {
                                        crate::clog!("profile", "render:html {}x{} in {:?}", g.cols, g.rows, dt);
                                    }
                                }
                                Some(RenderSnapshot {
                                    html,
                                    runs,
                                    cursor_row: g.cursor_row as i32,
                                    cursor_col: g.cursor_col as i32,
                                    scrollback_len: g.scrollback_len() as i32,
                                    scrollback_pos: g.scrollback_pos as i32,
                                    title: g.title.clone(),
                                    generation: gen,
                                })
                            } else {
                                None
                            }
                        };
                        if let Some(snap) = snap_opt {
                            if let Ok(mut slot) = render_slot_w.lock() {
                                *slot = Arc::new(snap);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(PtyTerminal {
            grid,
            render_slot,
            render_gen,
            writer: writer_arc,
            master_fd,
            _master: pair.master,
            child,
            output_tap,
            completion_capture,
        })
    }

    /// v0.1.490: the UI reads the latest finished render here. Cheap:
    /// locks the slot only long enough to Arc::clone the snapshot (a
    /// pointer bump), then releases — it never holds the grid lock,
    /// so a parsing burst can't stall it. Returns the shared snapshot.
    pub fn latest_snapshot(&self) -> Arc<RenderSnapshot> {
        self.render_slot.lock()
            .map(|s| Arc::clone(&*s))
            .unwrap_or_else(|_| Arc::new(RenderSnapshot::default()))
    }

    /// v0.1.490: force a re-render + publish from the CURRENT grid
    /// state. The main thread calls this after mutating the grid
    /// directly (scrollback scroll, resize) — those changes don't go
    /// through the reader thread's byte path, so without this the new
    /// state would never be published until the next output burst.
    /// Bumps the generation so the UI picks it up. Cheap relative to
    /// the mutation it follows; runs on the caller's thread.
    pub fn republish(&self) {
        let mut g = match self.grid.lock() { Ok(g) => g, Err(_) => return };
        g.dirty = false; // we're rendering now; clear so reader doesn't double
        // v0.1.902: this was unconditionally building `html` even when native
        // rendering is on (wasted work — the Canvas never reads it), and never
        // built `runs` at all, so a scroll/resize (the two callers of
        // `republish`, which mutate the grid OUTSIDE the reader thread's byte
        // path) left the native snapshot stale until the next PTY output burst
        // triggered a reader-thread render. Mirror the reader thread's gating.
        let native = native_render();
        let html = if native { String::new() } else { g.render_html() };
        let runs = if native { g.screen_runs_packed() } else { String::new() };
        let snap = RenderSnapshot {
            html,
            runs,
            cursor_row: g.cursor_row as i32,
            cursor_col: g.cursor_col as i32,
            scrollback_len: g.scrollback_len() as i32,
            scrollback_pos: g.scrollback_pos as i32,
            title: g.title.clone(),
            // v0.1.491: draw from the shared atomic so reader +
            // republish never collide on a generation.
            generation: self.render_gen.fetch_add(
                1, std::sync::atomic::Ordering::SeqCst) + 1,
        };
        drop(g);
        if let Ok(mut slot) = self.render_slot.lock() {
            *slot = Arc::new(snap);
        }
    }

    /// Drain the output tap, returning all bytes captured since last drain
    pub fn drain_output(&self) -> Vec<u8> {
        if let Ok(mut tap) = self.output_tap.lock() {
            let bytes = std::mem::take(&mut *tap);
            return bytes;
        }
        Vec::new()
    }

    #[inline] pub fn write_bytes(&self, data: &[u8]) {
        if let Ok(mut w) = self.writer.lock() { let _ = w.write_all(data); let _ = w.flush(); }
    }
    #[inline] pub fn write_str(&self, s: &str) { self.write_bytes(s.as_bytes()); }

    /// v0.1.418: drain any terminal-response bytes the grid
    /// queued (e.g. DSR cursor-position replies) and write them
    /// back to the PTY master, as a real terminal would on its
    /// input stream. Called once per read tick. Drains under the
    /// grid lock, releases, then writes (writer is a separate
    /// mutex, so no deadlock, but we avoid holding grid across IO
    /// regardless).
    pub fn drain_responses(&self) {
        let bytes = {
            match self.grid.lock() {
                Ok(mut g) if !g.response_buf.is_empty() => {
                    std::mem::take(&mut g.response_buf)
                }
                _ => return,
            }
        };
        if !bytes.is_empty() {
            self.write_bytes(&bytes);
        }
    }
    /// v0.1.883: take (and clear) any exit code the reader thread
    /// captured from an OSC 133 `D;<code>` marker since the last call.
    /// Non-blocking: uses `try_lock` so the UI tick never stalls on the
    /// reader during a parsing burst — the code persists on the grid
    /// until taken, so a contended tick just defers it one frame.
    /// Returns `None` when there's no pending code (the common case for
    /// shells without OSC 133 integration).
    pub fn take_osc133_exit(&self) -> Option<i32> {
        match self.grid.try_lock() {
            Ok(mut g) => g.osc133_exit.take(),
            Err(_) => None,
        }
    }
    pub fn get_screen_text(&self) -> String { self.grid.lock().unwrap().render_html() }
    /// Plain-text render of current screen (no scrollback, no HTML)
    pub fn render_text(&self) -> String {
        self.grid.lock().unwrap().render_text()
    }

    /// Per-row plain-text. See `Grid::render_row` doc for the full
    /// rationale — this is the foundation of the v0.1.98 per-row
    /// rendering model that replaces the single-TextEdit RichText
    /// approach.
    pub fn render_row(&self, idx: usize) -> String {
        self.grid.lock().unwrap().render_row(idx)
    }

    /// v0.1.765: last `max_rows` of output as plain text (for build summary).
    pub fn recent_text(&self, max_rows: usize) -> String {
        self.grid.lock().map(|g| g.recent_text(max_rows)).unwrap_or_default()
    }

    /// Per-row HTML with ANSI color spans. v0.1.102: gives the per-row
    /// renderer color support (helix, vim, ls --color, etc.). QML's
    /// v0.1.124: monotonic line counter. Returns None if grid lock
    /// is poisoned (shouldn't happen in practice).
    pub fn grid_lines_emitted(&self) -> Option<u64> {
        self.grid.lock().ok().map(|g| g.lines_emitted)
    }

    /// v0.1.349: scrollback search delegate. See `Grid::search_scrollback`
    /// for the algorithm. Returns matches in oldest-first order; caller
    /// reverses if it wants newest-first.
    pub fn search_scrollback(
        &self,
        query: &str,
        case_sensitive: bool,
    ) -> Vec<(u64, usize, usize, String)> {
        self.grid
            .lock()
            .map(|g| g.search_scrollback(query, case_sensitive))
            .unwrap_or_default()
    }

    /// Number of visible rows (matches the PTY row size). Stable
    /// per resize.
    pub fn visible_rows(&self) -> usize {
        self.grid.lock().unwrap().visible_rows()
    }

    /// v0.1.896: rough resident size of this terminal — grid+scrollback cells
    /// plus the output-tap buffer. Uses `try_lock` so the mem-logger never
    /// blocks the reader thread; whatever it can't sample this tick counts 0.
    pub fn approx_mem_bytes(&self) -> usize {
        let grid = self.grid.try_lock().map(|g| g.approx_mem_bytes()).unwrap_or(0);
        let tap = self.output_tap.try_lock().map(|t| t.capacity()).unwrap_or(0);
        grid + tap
    }

    /// v0.1.896: scrollback row count, for the footprint breakdown.
    pub fn scrollback_rows(&self) -> usize {
        self.grid.try_lock().map(|g| g.scrollback_rows()).unwrap_or(0)
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        // Try portable-pty resize first
        match self._master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }) {
            Ok(()) => {}
            Err(_) => {
                // Fallback: raw ioctl
                if let Some(fd) = self.master_fd {
                    #[repr(C)] struct Ws { r: u16, c: u16, xp: u16, yp: u16 }
                    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &Ws { r: rows, c: cols, xp: 0, yp: 0 }); }
                }
            }
        }
        self.grid.lock().unwrap().resize(cols as usize, rows as usize);
    }

    /// Returns true if the child shell has exited. Used by the auto-respawn
    /// logic so that Ctrl-D doesn't leave the user with a dead PTY they
    /// can't recover from. portable-pty's Child::try_wait gives us a
    /// non-blocking check.
    pub fn child_exited(&mut self) -> bool {
        // try_wait returns Ok(Some(_)) when the child has exited.
        // We require &mut self because portable_pty::Child::try_wait does.
        self.child.try_wait().map(|s| s.is_some()).unwrap_or(false)
    }

    /// PID of the shell process. Used by the cwd-tracker to read
    /// /proc/<pid>/cwd as a fallback when OSC 7 isn't being emitted.
    /// Returns None on platforms where portable-pty doesn't expose it.
    pub fn shell_pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Returns the basename of the command currently running in the
    /// foreground of this PTY. "shell" if it's the shell itself (zsh,
    /// bash, fish), "man" / "less" / "vim" / "helix" / etc. if a
    /// child program is in the foreground.
    ///
    /// Mechanism: tcgetpgrp(master_fd) returns the foreground process
    /// group ID. We then read /proc/<pgid>/comm to get the command
    /// name. The kernel keeps this up to date as processes are
    /// spawned and exit, so we get a real-time read.
    ///
    /// Returns None if:
    ///   - master_fd unavailable
    ///   - tcgetpgrp() fails (rare, indicates bad fd)
    ///   - /proc/<pgid>/comm read fails (process exited between
    ///     tcgetpgrp and the read; very rare race)
    ///   - non-Linux (we read /proc; macOS would need different probe)
    ///
    /// Cost: one ioctl + one /proc read = ~5-50µs. Cheap enough to
    /// poll on every voice utterance for pager-aware command routing.
    pub fn foreground_command(&self) -> Option<String> {
        let fd = self.master_fd?;
        let pgid = unsafe { libc::tcgetpgrp(fd) };
        if pgid <= 0 { return None; }
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", pgid)).ok()?;
        // /proc/<pid>/comm has trailing newline; strip it. Also strip
        // any nul bytes that ancient kernels sometimes append.
        let trimmed = comm.trim_end_matches(|c: char| c == '\n' || c == '\0');
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    }

    /// v0.1.668: the foreground process's wait state for the "why so slow"
    /// probe: (wchan symbol, process state char). Reads /proc/<pgid>/wchan
    /// and /proc/<pgid>/stat for the terminal foreground process group.
    /// Returns None if there's no foreground pgid or the reads fail (e.g.
    /// the process just exited). Linux-only (we read /proc). Cheap: two
    /// small /proc reads.
    pub fn foreground_wait_state(&self) -> Option<(String, char)> {
        let fd = self.master_fd?;
        let pgid = unsafe { libc::tcgetpgrp(fd) };
        if pgid <= 0 { return None; }
        // skip if the foreground IS the shell itself (pgid == shell pid):
        // an idle prompt is never "slow".
        if let Some(sp) = self.shell_pid() {
            if pgid as u32 == sp { return None; }
        }
        let wchan = std::fs::read_to_string(format!("/proc/{}/wchan", pgid)).ok()?;
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", pgid)).ok()?;
        // state is the first token after the final ')' (comm is parenthesized
        // and may itself contain spaces/parens, so split on the LAST ')').
        let state = stat.rsplit(')').next()
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.chars().next())
            .unwrap_or('?');
        Some((wchan.trim().to_string(), state))
    }

    /// Convenience: true when foreground command is a known pager
    /// (man, less, more, view, most). Used by the voice intent
    /// router to decide whether to send `q` before a fresh command.
    /// Conservative — does NOT include vim, helix, nano because
    /// `q` doesn't quit those (and could trigger unwanted behavior).
    pub fn foreground_is_pager(&self) -> bool {
        match self.foreground_command().as_deref() {
            Some("man") | Some("less") | Some("more") | Some("view")
            | Some("most") | Some("pg") => true,
            _ => false,
        }
    }

    /// True if the terminal is currently asking for a password and we
    /// should NOT echo what the user types.
    ///
    /// Detection is layered:
    ///
    /// 1. Foreground command is one of the well-known
    ///    password-asking programs (sudo, ssh, scp, sftp, passwd,
    ///    su, doas, gpg with --passphrase prompts). Most reliable
    ///    signal — when sudo is in the foreground waiting on a
    ///    password, this is true even before any prompt text appears.
    /// 2. The most recent non-empty visible line ends with a known
    ///    password-prompt phrase. This catches custom prompts and
    ///    scripts that read passwords without using a known binary.
    ///    We look at the LAST visible line because the cursor is
    ///    typically positioned right after the prompt text.
    ///
    /// We deliberately don't check ECHO state on the slave PTY's
    /// termios — we don't have stable access to that from the
    /// portable-pty handle in the way the rest of this module is
    /// structured. The two-layered heuristic catches the common
    /// cases without that complexity.
    ///
    /// Used to suppress:
    ///   - The bouncy typed-char overlay (would render password
    ///     characters visible to anyone watching the screen)
    ///   - Any keystroke logging or analytics that records actual
    ///     bytes (we still log COUNT, just not the chars themselves)
    pub fn foreground_is_password_prompt(&self) -> bool {
        // v0.1.562 (helix slow-startup fix): a password prompt is a
        // line-based prompt — it NEVER occurs inside an alt-screen TUI
        // (helix/vim/gdb-tui/less). But this method runs EVERY read
        // tick (security::set_password_mode), and Layer 2 below calls
        // render_text() which allocates + renders the whole grid. During
        // helix's startup redraw flood the tick fires repeatedly, so we
        // were re-rendering the full grid to a string over and over for
        // a check whose answer is trivially "no". Short-circuit in
        // alt-screen: skip both the /proc read AND the full-grid render.
        // (alt_screen_active lives on TermGrid, reached via the lock.)
        if self.grid.lock().map(|g| g.alt_screen_active()).unwrap_or(false) {
            return false;
        }
        // Layer 1: foreground command name.
        let fg = self.foreground_command();
        if let Some(name) = fg.as_deref() {
            // Strip path prefix if present — foreground_command returns
            // basename already, but be defensive.
            let base = name.rsplit('/').next().unwrap_or(name);
            match base {
                "sudo" | "ssh" | "scp" | "sftp" | "rsync"
                | "passwd" | "su" | "doas"
                | "ssh-add" | "ssh-keygen"
                | "gpg" | "gpg2"
                | "ksshaskpass" | "ssh-askpass" | "systemd-ask-password"
                | "smbclient" | "vncpasswd"
                => return true,
                _ => {}
            }
        }
        // Layer 2: recent visible text ends with a password-y prompt.
        // Read the last non-empty line and check its tail.
        let text = self.render_text();
        // Find the last non-empty line. We scan from the END backward
        // because the cursor is at the bottom of the visible area
        // when waiting for input.
        let last_line = text.lines().rev()
            .find(|l| !l.trim().is_empty());
        if let Some(line) = last_line {
            let lower = line.to_lowercase();
            let trimmed_end = lower.trim_end_matches(|c: char|
                c == ' ' || c == ':' || c == ']');
            // Common password prompt suffixes. The substring check
            // catches "Password:", "password for kesslerd:",
            // "[sudo] password for X:", "Enter passphrase for key X:",
            // and various translated forms. Keep this list short
            // and high-signal — false positives here mean a normal
            // utterance gets silently dropped from the typing
            // overlay, which is annoying but not dangerous.
            let prompt_markers = [
                "password",
                "passphrase",
                "[sudo]",
                "verification code",
            ];
            if prompt_markers.iter().any(|m| trimmed_end.contains(m)) {
                return true;
            }
        }
        false
    }

    /// Read the shell's current working directory from /proc/<pid>/cwd.
    /// This is a Linux-only fallback for shells that don't emit OSC 7
    /// (which is most shells out of the box). The /proc symlink
    /// readlink() is cheap (microseconds) so we can call this every time
    /// we need the cwd without worrying about cost.
    pub fn shell_cwd(&self) -> Option<std::path::PathBuf> {
        let pid = self.shell_pid()?;
        // The shell may have spawned a foreground child (e.g. less, vim,
        // helix). Reading /proc/<shell_pid>/cwd gives the SHELL's cwd, not
        // the foreground child's — which is what we want for completion
        // because tab completion happens at the shell prompt level.
        let p = format!("/proc/{}/cwd", pid);
        std::fs::read_link(&p).ok()
    }

    /// Query the visible shell for completions of `partial` by injecting
    /// `<partial><TAB>`, capturing the response, and cleaning the prompt
    /// with two Ctrl-U keystrokes.
    ///
    /// Returns the raw bytes captured during the wait window, or None if
    /// the query was unsafe to run (alt-screen active, etc.). Caller is
    /// responsible for parsing — formats vary by shell config.
    ///
    /// SAFETY CONSTRAINTS (all checked, all critical):
    ///
    ///  - Never run while the shell is in alt-screen mode. A TAB byte to
    ///    helix/vim/less is interpreted as a literal tab keypress, not a
    ///    completion request. The visible TUI would receive an unwanted
    ///    edit. We check `alt_screen_active()` before doing anything and
    ///    return None if true.
    ///
    ///  - Caller should only invoke this when the user is NOT actively
    ///    typing — otherwise our injected bytes interleave with theirs.
    ///    The intended trigger is voice popup open, where the user has
    ///    handed focus to us.
    ///
    ///  - Cleanup (Ctrl-U twice) always runs, even on early-return paths,
    ///    so we never leave the prompt with our injected partial still
    ///    typed. The two Ctrl-Us are belt-and-suspenders: one for the
    ///    typed text, one for any partial completion zsh already filled in.
    pub fn query_completion(&self, partial: &str, wait_ms: u64) -> Option<Vec<u8>> {
        // Refuse when in alt-screen — TAB would go to the running TUI.
        if self.grid.lock().ok().map(|g| g.alt_screen_active()).unwrap_or(true) {
            return None;
        }
        // Refuse if the partial contains anything dangerous. Newlines
        // would commit the line; control bytes could mess up shell state.
        // Reject the request rather than try to sanitize.
        if partial.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return None;
        }
        // Length sanity — completion queries are short. If the caller is
        // sending us a screen full of text something is wrong.
        if partial.len() > 200 {
            return None;
        }

        // Activate completion capture FIRST so any bytes that arrive in
        // response to our TAB land in our buffer. Then write partial,
        // then TAB. Capture, then deactivate. Always send Ctrl-U twice
        // afterward to clean the prompt — even on the cleanup path.
        if let Ok(mut cc) = self.completion_capture.lock() {
            *cc = Some(Vec::with_capacity(8192));
        } else {
            return None;
        }

        // Write partial + TAB. We use single bytes via write_bytes to
        // avoid any newline/encoding surprises.
        if !partial.is_empty() {
            self.write_bytes(partial.as_bytes());
        }
        self.write_bytes(&[0x09]); // TAB

        // Wait for the shell to respond. zsh menu rendering is fast (~10ms);
        // we give it a generous window because the first TAB after a fresh
        // prompt sometimes triggers compinit which takes longer.
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));

        // Capture what we got. Take the buffer out so future activations
        // start fresh.
        let captured = if let Ok(mut cc) = self.completion_capture.lock() {
            cc.take().unwrap_or_default()
        } else {
            Vec::new()
        };

        // Cleanup: two Ctrl-U (kill-line). One handles the partial text
        // we typed. A second handles any partial completion zsh inserted.
        // We send these even if capture failed — must not leave the
        // prompt in a polluted state.
        self.write_bytes(&[0x15]); // Ctrl-U
        self.write_bytes(&[0x15]); // Ctrl-U

        if captured.is_empty() { None } else { Some(captured) }
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    /// Helper: build a TermGrid with seeded live cells from rows of text.
    /// Each row is wrapped to `cols` width (right-padded with spaces).
    fn grid_with_rows(rows: &[&str], cols: usize) -> TermGrid {
        let n = rows.len();
        let mut g = TermGrid::new(cols, n.max(1));
        for (r, line) in rows.iter().enumerate() {
            for (c, ch) in line.chars().take(cols).enumerate() {
                g.cells[r][c] = Cell { ch, ..Cell::default() };
            }
        }
        g
    }

    // v0.1.785: native run builder mirrors the HTML path's coalescing, reverse
    // video, palette resolution, attribute flags, phantom-skip and trailing trim.
    #[test]
    fn render_row_runs_coalesces_and_resolves() {
        let dfg = [200u8, 224, 160];
        let dbg = [13u8, 13, 13];
        let cc = |ch: char, fg: u16, bg: u16, attrs: u8| Cell { ch, fg, bg, attrs };
        // coalescing: "ab" red then "cd" default -> 2 runs
        let row = vec![cc('a',1,256,0), cc('b',1,256,0), cc('c',256,256,0), cc('d',256,256,0)];
        let r = TermGrid::render_row_runs(&row, dfg, dbg);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].text, "ab"); assert_eq!(r[0].fg, [255,85,85]);
        assert_eq!(r[1].text, "cd"); assert_eq!(r[1].fg, dfg);
        // reverse video swaps fg/bg
        let row = vec![cc('X',1,4,ATTR_REV)];
        let r = TermGrid::render_row_runs(&row, dfg, dbg);
        assert_eq!(r[0].fg, [98,114,164]); // blue (was bg)
        assert_eq!(r[0].bg, [255,85,85]);  // red  (was fg)
        // attribute flags: bold|italic|under|strike
        let row = vec![cc('Z',256,256, ATTR_BOLD|ATTR_ITALIC|ATTR_UNDER|ATTR_STRIKE)];
        let r = TermGrid::render_row_runs(&row, dfg, dbg);
        assert_eq!(r[0].flags, RUN_BOLD|RUN_ITALIC|RUN_UNDER|RUN_STRIKE);
        // phantom right-half of a wide glyph is skipped
        let row = vec![cc('W',256,256,0), cc(' ',256,256,ATTR_PHANTOM)];
        let r = TermGrid::render_row_runs(&row, dfg, dbg);
        assert_eq!(r.len(), 1); assert_eq!(r[0].text, "W");
        // trailing default whitespace trimmed; all-default row -> no runs
        let row = vec![cc('h',256,256,0), cc('i',256,256,0), cc(' ',256,256,0), cc(' ',256,256,0)];
        let r = TermGrid::render_row_runs(&row, dfg, dbg);
        assert_eq!(r.len(), 1); assert_eq!(r[0].text, "hi");
        assert!(TermGrid::render_row_runs(&vec![cc(' ',256,256,0); 8], dfg, dbg).is_empty());
    }

    // v0.1.892 (dirty-rect arc stage c): the per-row packed cache must be
    // byte-identical to a fresh render — that invariant is what makes it safe
    // to reuse cached rows on the native draw path without visual drift.
    #[test]
    fn packed_cache_matches_fresh_render() {
        let mut g = grid_with_rows(&["hello world", "second line", "third  row"], 12);
        let fresh: Vec<String> = (0..g.rows).map(|r| g.row_runs_packed(r)).collect();
        // first read (miss → compute) and second read (hit) both match fresh
        for r in 0..g.rows {
            assert_eq!(g.row_runs_packed_cached(r), fresh[r], "miss row {r}");
        }
        for r in 0..g.rows {
            assert_eq!(g.row_runs_packed_cached(r), fresh[r], "hit row {r}");
        }
    }

    #[test]
    fn packed_cache_hit_then_invalidate() {
        let mut g = grid_with_rows(&["aaa", "bbb"], 3);
        let before = g.row_runs_packed_cached(0).to_string();
        // Mutate row 0 WITHOUT marking it dirty → the accessor must still serve
        // the cached value (proves it's actually caching, not recomputing).
        g.cells[0][0] = Cell { ch: 'Z', ..Cell::default() };
        assert_eq!(g.row_runs_packed_cached(0), before, "stale read should be a cache hit");
        // mark_row_dirty invalidates → recompute, and it matches a fresh render.
        g.mark_row_dirty(0);
        let after = g.row_runs_packed_cached(0).to_string();
        assert_ne!(after, before, "invalidated cache must recompute");
        assert_eq!(after, g.row_runs_packed(0), "recompute must equal fresh render");
    }

    // v0.1.902: screen_runs_packed replaces the inline whole-screen assembly
    // that used to live in main.rs's term_screen_runs (2-char header + 0x1c +
    // rows joined by 0x1d). Pin the exact wire format so moving the assembly
    // off the UI thread (into the reader thread / republish snapshot build)
    // can't silently drift from what the QML Canvas parser expects.
    #[test]
    fn screen_runs_packed_matches_prior_inline_format() {
        let mut g = grid_with_rows(&["hello world", "second line"], 12);
        g.cursor_visible = true;
        let whole = g.screen_runs_packed();
        // Header: 2 chars + 0x1c separator.
        let hsep = whole.find('\u{1c}').expect("must contain header separator");
        let header = &whole[..hsep];
        assert_eq!(header.chars().count(), 2, "header must be exactly 2 chars");
        assert_eq!(&header[1..2], "1", "cursor_visible=true must encode as '1'");
        // Body: rows joined by 0x1d, each row byte-identical to the cached
        // per-row accessor (the same one the old inline loop called).
        let body = &whole[hsep + '\u{1c}'.len_utf8()..];
        let rows: Vec<&str> = body.split('\u{1d}').collect();
        assert_eq!(rows.len(), g.rows);
        for (r, row) in rows.iter().enumerate() {
            assert_eq!(*row, g.row_runs_packed_cached(r), "row {r} mismatch");
        }
    }

    #[test]
    fn screen_runs_packed_alt_screen_bit() {
        let mut g = grid_with_rows(&["x"], 3);
        assert_eq!(&g.screen_runs_packed()[..1], "0", "alt screen inactive → '0'");
        g.enter_alt();
        assert_eq!(&g.screen_runs_packed()[..1], "1", "alt screen active → '1'");
    }

    #[test]
    fn packed_cache_mark_all_dirty_clears_every_row() {
        let mut g = grid_with_rows(&["one", "two", "six"], 3);
        for r in 0..g.rows { let _ = g.row_runs_packed_cached(r); } // warm the cache
        for r in 0..g.rows { g.cells[r][0] = Cell { ch: 'X', ..Cell::default() }; }
        g.mark_all_dirty();
        for r in 0..g.rows {
            let cached = g.row_runs_packed_cached(r).to_string();
            assert_eq!(cached, g.row_runs_packed(r), "row {r} stale after mark_all_dirty");
        }
    }

    #[test]
    fn packed_cache_out_of_range_is_empty_and_safe() {
        let mut g = grid_with_rows(&["x"], 1);
        assert_eq!(g.row_runs_packed_cached(99), ""); // no panic, empty
    }

    #[test]
    fn empty_query_returns_no_matches() {
        let g = grid_with_rows(&["hello world"], 80);
        let r = g.search_scrollback("", false);
        assert!(r.is_empty());
    }

    // v0.1.453: regression for the panic at the 'J' handler —
    // "index out of bounds: len is 26, index is 26" when
    // cursor_row had drifted to rows (one past the last index)
    // and an erase-in-display sequence arrived (helix quitting).
    // The csi_dispatch entry clamp must prevent the OOB.
    #[test]
    fn csi_erase_display_with_out_of_range_cursor_does_not_panic() {
        let mut g = TermGrid::new(99, 26);
        // Force the buggy state: cursor parked one past the last row.
        g.cursor_row = 26; // rows == 26, valid indices 0..=25
        g.cursor_col = 0;
        // Feed ESC [ 0 J (erase from cursor to end of display) —
        // this is exactly what crashed. Must not panic.
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[0J" {
            parser.advance(&mut g, b);
        }
        // Cursor should have been clamped into range.
        assert!(g.cursor_row < g.rows, "cursor_row clamped: {}", g.cursor_row);
    }

    #[test]
    fn csi_erase_line_with_out_of_range_cursor_does_not_panic() {
        let mut g = TermGrid::new(40, 10);
        g.cursor_row = 999; // wildly out of range
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[2K" { // erase entire line
            parser.advance(&mut g, b);
        }
        assert!(g.cursor_row < g.rows);
    }

    #[test]
    fn finds_single_match_case_insensitive() {
        let g = grid_with_rows(&["The Quick Brown Fox"], 80);
        let r = g.search_scrollback("quick", false);
        assert_eq!(r.len(), 1, "expected 1 hit, got {:?}", r);
        let (_abs, s, e, text) = &r[0];
        assert_eq!(*s, 4);
        assert_eq!(*e, 9);
        assert!(text.starts_with("The Quick"));
    }

    #[test]
    fn case_sensitive_filters_out_lowercase() {
        let g = grid_with_rows(&["Quick", "quick", "QUICK"], 80);
        let r = g.search_scrollback("Quick", true);
        assert_eq!(r.len(), 1, "case-sensitive should only match Quick");
    }

    #[test]
    fn non_overlapping_matches_in_one_row() {
        let g = grid_with_rows(&["abab abab"], 80);
        let r = g.search_scrollback("aba", false);
        assert_eq!(r.len(), 2, "expected 2 non-overlapping matches in {:?}", r);
        // First match at col 0..3, second at col 5..8 (the second "aba" in
        // the second "abab"). Important: NOT 3 — the "bab abab" overlap
        // is excluded by the non-overlapping iteration.
        assert_eq!(r[0].1, 0);
        assert_eq!(r[1].1, 5);
    }

    #[test]
    fn alt_screen_skips_search() {
        // Construct a grid in alt-screen mode (alt_cells = Some).
        let mut g = grid_with_rows(&["hello"], 80);
        g.alt_cells = Some(vec![vec![Cell { ch: '\u{0000}', ..Cell::default() }; 80]; 1]);
        // Even though "hello" is in live cells, alt-screen excludes
        // scrollback search entirely.
        let r = g.search_scrollback("hello", false);
        assert!(r.is_empty(), "alt-screen should suppress search");
    }

    // v0.1.418: DSR (Device Status Report) replies. Tools send
    // ESC[6n and block for the cursor-position reply; without it
    // they stall (the ollama-ls-is-slow bug).
    #[test]
    fn dsr_cursor_position_report() {
        let mut g = grid_with_rows(&["hello world"], 80);
        // Move cursor to a known spot (row 0, col 3 → reports
        // 1-based 1;4).
        g.cursor_row = 0;
        g.cursor_col = 3;
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[6n" { parser.advance(&mut g, b); }
        assert_eq!(g.response_buf, b"\x1b[1;4R",
            "DSR 6 should reply ESC[<row>;<col>R (1-based)");
    }

    #[test]
    fn dsr_device_status_report() {
        let mut g = grid_with_rows(&["x"], 80);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[5n" { parser.advance(&mut g, b); }
        assert_eq!(g.response_buf, b"\x1b[0n",
            "DSR 5 should reply terminal-OK ESC[0n");
    }

    #[test]
    fn dsr_unknown_param_no_reply() {
        let mut g = grid_with_rows(&["x"], 80);
        let mut parser = vte::Parser::new();
        // DSR with an unsupported param produces no reply (rather
        // than a wrong one, which could confuse the querying tool).
        for &b in b"\x1b[99n" { parser.advance(&mut g, b); }
        assert!(g.response_buf.is_empty(),
            "unsupported DSR param should not reply");
    }

    #[test]
    fn osc133_exit_parse() {
        // D;<code> → the exit code; non-D markers and bare D → None.
        assert_eq!(parse_osc133_exit(&[b"D", b"0"]), Some(0));
        assert_eq!(parse_osc133_exit(&[b"D", b"1"]), Some(1));
        assert_eq!(parse_osc133_exit(&[b"D", b"130"]), Some(130));
        assert_eq!(parse_osc133_exit(&[b"D"]), None, "bare D has no code");
        assert_eq!(parse_osc133_exit(&[b"A"]), None, "prompt-start marker");
        assert_eq!(parse_osc133_exit(&[b"B"]), None);
        assert_eq!(parse_osc133_exit(&[b"C"]), None, "output-start marker");
        assert_eq!(parse_osc133_exit(&[b"D", b"abc"]), None, "non-numeric");
        assert_eq!(parse_osc133_exit(&[]), None);
        // Extra params after the code are ignored; the code still reads.
        assert_eq!(parse_osc133_exit(&[b"D", b"2", b"foo=bar"]), Some(2));
    }

    #[test]
    fn osc133_d_marker_captured_on_grid() {
        // End-to-end through the VTE parser: OSC 133;D;1 ST sets
        // osc133_exit on the grid; an A marker leaves it untouched.
        let mut g = grid_with_rows(&["x"], 80);
        let mut parser = vte::Parser::new();
        for &b in b"\x1b]133;D;1\x07" { parser.advance(&mut g, b); }
        assert_eq!(g.osc133_exit, Some(1));
        // A subsequent prompt-start marker must NOT clobber the captured
        // code (it's drained by the main thread, not reset here).
        for &b in b"\x1b]133;A\x07" { parser.advance(&mut g, b); }
        assert_eq!(g.osc133_exit, Some(1), "A marker must not reset exit");
    }
}


#[cfg(test)]
mod scroll_blit_tests {
    use super::*;

    /// Stream `lines` of text through the parser, reading every visible row each
    /// frame exactly as the renderer does, and report how many row re-packs it
    /// cost.
    fn stream(rows: usize, cols: usize, lines: usize) -> u64 {
        let mut g = TermGrid::new(cols, rows);
        let mut parser = vte::Parser::new();
        for i in 0..lines {
            let text = format!("line {i:5} the quick brown fox jumps over the lazy dog\r\n");
            for b in text.as_bytes() {
                parser.advance(&mut g, *b);
            }
            for r in 0..rows {
                let _ = g.row_runs_packed_cached(r);
            }
        }
        g.packs_computed()
    }

    #[test]
    fn streaming_output_does_not_repack_the_whole_screen_per_line() {
        // The property that matters: a scroll SHIFTS rows, so the cost of a line
        // of streaming output must not scale with how tall the window is.
        // Before the cache learned to shift with its rows, 500 lines cost 11,494
        // repacks at 24 rows and 22,648 at 50 — i.e. O(lines × rows), the
        // classic `cat a big file` cliff. After, both cost 1,000.
        let short = stream(24, 80, 500);
        let tall = stream(50, 80, 500);
        assert!(short < 500 * 4, "24-row stream should be O(lines), got {short} repacks");
        assert!(tall < 500 * 4, "50-row stream should be O(lines), got {tall} repacks");
        // and the real invariant: doubling the rows must not double the work
        assert!(tall <= short + 500,
                "repacks must not scale with row count ({short} at 24 rows vs {tall} at 50)");
    }

    #[test]
    fn the_shifted_cache_still_tells_the_truth() {
        // Correctness guard for the blit: after scrolling, every cached row must
        // still equal a freshly packed one. A cache that is fast and wrong is
        // worse than the slow one it replaced.
        let (rows, cols) = (12usize, 40usize);
        let mut g = TermGrid::new(cols, rows);
        let mut parser = vte::Parser::new();
        for i in 0..60 {
            let text = format!("row {i} abcdefghijklmnop\r\n");
            for b in text.as_bytes() {
                parser.advance(&mut g, *b);
            }
            for r in 0..rows {
                let _ = g.row_runs_packed_cached(r);
            }
        }
        for r in 0..rows {
            let fresh = g.row_runs_packed(r);
            let cached = g.row_runs_packed_cached(r).to_string();
            assert_eq!(cached, fresh, "row {r}: cached form drifted from the truth");
        }
    }
}
