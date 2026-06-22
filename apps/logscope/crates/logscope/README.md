# Logscope

A streaming log inspector for files of arbitrary size (tested design target: 150+ GB).
Built with Rust + GTK4/Adwaita. No file is ever fully loaded into RAM.

## Features

- **Reserved peek** — first 10,000 lines are indexed immediately for instant feedback
- **Chunked block-hash pass** — streams the rest of the file in 8 MB chunks using xxHash64
- **Live charts** — token frequency and block-repeat histograms rendered as SVG via `plotters`
- **Progress bar** — byte-level progress during the streaming pass
- **Pinned files** — `$HOME/.cache/helix/helix.log` auto-detected as a quick-open
- **Dark Adwaita theme** — Catppuccin Mocha palette, monospace peek view
- **Non-blocking UI** — analysis runs on a background thread; GTK idle loop drains results

## Architecture

```
src/
  main.rs          — app entry, CSS loading
  analyzer.rs      — streaming engine (peek + block-hash pass)
  charts.rs        — SVG chart generators (plotters)
  ui/
    mod.rs
    window.rs      — main window, file picker, message handler
    components.rs  — reusable widget helpers
logscope.css       — Catppuccin Mocha GTK stylesheet
```

## Build Requirements

```
# Arch / Manjaro
sudo pacman -S gtk4 libadwaita rust

# Fedora
sudo dnf install gtk4-devel libadwaita-devel rust cargo

# Ubuntu 24.04+
sudo apt install libgtk-4-dev libadwaita-1-dev cargo
```

## Build & Run

```bash
# Debug build (faster compile)
cargo run

# Release build (for large files — ~3x faster analysis)
cargo run --release

# Open a specific file directly
cargo run --release -- /path/to/huge.log
```

## Tuning

Edit constants in `src/analyzer.rs`:

| Constant       | Default | Effect                                      |
|----------------|---------|---------------------------------------------|
| `CHUNK_SIZE`   | 8 MB    | Larger = fewer HashMap entries, more RAM    |
| `PEEK_LINES`   | 10,000  | Lines read in phase 1                       |
| `MIN_TOKEN_LEN`| 3       | Minimum token length counted in freq map    |
| `TOP_TOKENS`   | 30      | How many tokens shown in the chart          |

## Planned extensions

- [ ] Regex search overlay (streaming, no full load)
- [ ] Line-level dedup view (show only unique lines)
- [ ] Export counts to CSV / JSON
- [ ] mmap backend for random access seeks
- [ ] Reverse-engineering mode: hex view, entropy chart per region
