# Terminal

The terminal tab and its supporting machinery.

## Architecture

```
[user types / paste / voice → PTY input]
                ↓
        portable_pty Pty (child shell, master FD held)
                ↓ stdout bytes
        Grid::feed(bytes) — ANSI parser
                ↓
        Cell grid (rows × cols, each cell has char + colors + attrs)
                ↓
QML Repeater calls term_row_html(idx) for each visible row
        backend.term_row_html(idx) → render_row_html()
                ↓
        per-row colored HTML spans (or plain text in monochrome mode)
                ↓
        Text element renders inline, fixed charH height per row
```

## Module map

| Module | Responsibility |
|--------|----------------|
| `pty_term.rs` | PTY spawn, ANSI parser, Cell grid, render_row{_html} |
| `term_blocks.rs` | Command block tracking (one per `Enter`-submitted command) |
| `log_summary.rs` | Async AI summary worker |
| `entity_extract.rs` | Parse `rg`, `find`, `dust` output → suggested actions |
| `sudo_keepalive.rs` | Refresh sudo timestamp on sudo commands |
| `path_fuzzy.rs` | File system fuzzy search for cd suggestions |
| `command_index.rs` | Built-in + tldr fallback for command help |

## Cell model

```rust
struct Cell {
    ch: char,
    fg: u32,
    bg: u32,
    attrs: u8,    // bold, dim, italic, underline, strike, blink, reverse, phantom
}
```

Phantom bit (0x80) marks the right half of a wide character (CJK,
emoji). The wide glyph paints across both cells; the phantom isn't
rendered (so we don't double-print).

## Per-row rendering (v0.1.98+)

Each terminal row is its own QML Text element with height locked
to `charH`. No more single-TextEdit-with-RichText document. Cursor
at row N is at `y = N * charH` exactly.

`term_row(idx)` returns plain text (monochrome mode);
`term_row_html(idx)` returns colored HTML spans (color mode, default).

`render_row_html` (v0.1.113+) wraps every span (including default-
styled runs) in `<span style='white-space:pre'>` so HTML doesn't
collapse multiple spaces. This is what fixed dust/fastfetch alignment.

## Block tracking

`BlockTracker` tracks one command at a time. When the user submits
a command (Enter, or voice-confirmed term_cmd intent):

1. Previous block finishes (duration recorded)
2. New block opens
3. Subsequent PTY output captured into `block.output` (stripped of
   ANSI, capped at 32KB)
4. When next prompt detected (heuristic), block finishes

The "next prompt detected" heuristic looks for typical bash/zsh
prompt patterns (`$ `, `> `, `# `, `→ `) at start of line. Not
perfect but good enough.

Each block keeps: id, command, output, exit_code, duration_ms,
started_at, summary (optional, set asynchronously).

## AI summary

`log_summary.rs` processes finished blocks via a background worker:

1. Block submitted via `request_summary(id, cmd, output)`
2. Worker pulls from queue, builds prompt, runs through
   `system_info::summarizer_priority_list` (any model whose name
   contains "summariz" gets priority; falls back to gemma/phi3/llama/
   tinyllama if installed)
3. Result stored on block; UI shows a "summary" chip

Watchdog handling (v0.1.115):
- 30s timeout per attempt for purpose-built summarizer models;
  12s for general-purpose models
- 45s total budget across all attempts in one inference
- 2 consecutive slow failures (≥8s) → early bail, watchdog escalates
  global backoff (90s/180s/360s/600s pause)

Diff-aware (v0.1.107): if the current block's output is similar to
the previous one's, prompt the LLM with a diff instead of full output.
Faster + more relevant ("compared to previous run, X changed").

## Smart-cd (v0.1.107)

When `cd` runs, we track the previous CWD. `_smartCdUndoTimer` in
QML fires 300ms after cd is observed in the command, showing
"cd'd — back to <basename>" toast with an Undo button. Auto-dismisses
after 6s.

Backend method `term_previous_cwd()` returns the last directory
the user came from.

## Selection + copy menu

Region selection (v0.1.102): shift+drag or right-button-drag enters
selection mode. Selection state lives in QML
(`termScreen.selStartRow/Col`, `selEndRow/Col`).

Double-click opens the copy menu (v0.1.103) with chevron grid for
expanding selection by word/line/paragraph/whole screen. j/k/h/l
keyboard nav (v0.1.113 fixed Keys on Popup via FocusScope wrapper).

Path/folder tokens trigger file picker drawer instead.

## Wide character + ANSI color v2 (v0.1.101-102)

Wide chars (CJK, emoji) advance cursor by 2 cells. `unicode-width`
crate determines width. Right cell marked phantom (rendered empty).

ANSI colors v2 (v0.1.102): `render_row_html` emits span runs grouped
by identical styling. Theme defaults `[200,224,160]` fg / `[13,13,13]`
bg in the per-row HTML, matching QML th.tfg/th.tbg.

## Pager detection

The "in alt-screen" check (`is_alt_screen()` on Grid) detects
fullscreen TUIs (vim, less, helix). Some context-sensitive
behaviors:

- Smart-cd undo skipped in alt-screen
- Copy menu double-click WORKS in pagers (man, less, more — read-only
  ops don't disturb the pager); skipped in editors (vim, helix, nano)

`term_foreground_command()` returns the current foreground process
name for these decisions.

## Tunables

| Setting | Default | Notes |
|---------|---------|-------|
| Block output cap | 32 KB | per-block; truncation marker appended |
| Summary timeout (general) | 12s | per ollama call |
| Summary timeout (summariz) | 30s | "summariz" in model name |
| Total summary budget | 45s | across all attempts |
| Watchdog pause cascade | 90/180/360/600s | per consecutive failure cluster |
| Cursor mode | static (1) | also: blink (0), slide (3) |
| Color mode | on | toggle in Settings |
