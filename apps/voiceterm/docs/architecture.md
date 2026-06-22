# Architecture

High-level layout of the codebase and how modules interact.

## Stack

- **Rust** for the backend (Cargo: `web2qml`)
- **Qt 6 + QML** for the UI (via `qmetaobject-rs` bridge)
- **whisper-rs 0.16** for speech recognition
- **cpal** for audio capture
- **scraper** for HTML parsing
- **ureq** for HTTP

Single binary. No services. `~/.config/web2qml/` for persistent data.

## Module map

```
src/
├── main.rs              ← Qt backend object + qt_methods (~6300 lines)
├── voice.rs             ← whisper, VAD, preview thread, worker pool init
├── voice_intent.rs      ← English intent parser
├── voice_intent_es.rs   ← Spanish parser
├── voice_intent_fr.rs   ← French parser
├── voice_lang.rs        ← Language dispatcher; whisper lang code
├── phonetic.rs          ← MFCC fingerprints + matcher + DB
├── phonetic_confusions.rs ← Static confusion table
├── confusion_table.rs   ← Learned confusion corrections
├── token_stats.rs       ← Learned fillers + per-token entropy stats
├── audio_profile.rs     ← Speaker profile (pitch hint, RMS profile)
├── speakers.rs          ← Multi-user registry (v0.1.116+)
├── separation.rs        ← Per-chunk speaker classification
├── whisper_pool.rs      ← 1-4 long-lived inference workers
├── workflow.rs          ← Record/replay command sequences
├── pty_term.rs          ← PTY emulation, cell grid, ANSI parsing
├── term_blocks.rs       ← Command block tracking + summary requests
├── log_summary.rs       ← AI summary worker, diff-aware
├── system_info.rs       ← ollama discovery, model priority list
├── qml_gen.rs           ← HTML → QML translator (web rendering)
├── data.rs              ← Static TOML loaders (project markers, etc)
├── decision_cache.rs    ← "this phrase → that command" learned cache
├── llm_cache.rs         ← LLM result cache (Q&A, etc)
├── chat.rs              ← Conversation history
├── archive.rs           ← Page archive
├── entity_extract.rs    ← Parse dust/rg/find output → entities
├── path_fuzzy.rs        ← File system fuzzy search
├── sudo_keepalive.rs    ← Keep sudo session warm
├── command_index.rs     ← Built-in + tldr fallback index
├── ad_filter.rs         ← Web ad blocking
└── ... (more)
```

## Process model

One process. Threads:

- **Qt main thread** — UI, qt_method dispatch
- **PTY reader** — read from PTY, push to Cell grid + block tracker
- **Voice capture (cpal)** — sample callback → ring buffer + bg ring buffer
- **Voice preview** — every 250ms phonetic match; every 1500ms whisper streaming on growing buffer
- **Background voice listen** — wake-word detection
- **Whisper pool workers** — 1-4 threads, each holds a WhisperState, pull from job channel (v0.1.116+)
- **Log summary worker** — processes queued blocks for AI summary
- **Sudo keepalive** — refreshes sudo timestamp on detected sudo commands

## Voice flow (high level)

```
mic audio (cpal callback)
  ↓
ring buffer (foreground, capturing-gated)
  ↓
VAD watcher → detects silence → finalize utterance
  ↓
transcribe_with_dual_model: parallel Tiny + Base + (HighAccuracy)
  ↓
phonetic::top_matches_with_hint (cross-check)
  ↓
voice_intent::parse / voice_lang::parse_for_lang (language-aware)
  ↓
QML voicePopup → user confirms → intent kind → action
```

See [pipeline.md](pipeline.md) for full detail.

## Terminal flow

```
PTY (pty_term::Pty)
  ↓ raw bytes
Grid::feed → ANSI parsing → Cell updates
  ↓
[QML reads per-row]
  Repeater calls backend.term_row_html(idx) for each visible row
  ↓
Cell grid → render_row_html → spans with ANSI colors
  ↓
User types command + Enter → block_tracker.submit_command(cmd)
  ↓ when prompt re-detected
block_tracker.finish() → log_summary::request_summary
  ↓
log_summary worker (separate thread)
  ↓
ollama → summary text → block.summary
  ↓
QML pulls block list → renders chip
```

## Browser flow

```
backend.load_page(url, depth)
  ↓
fetch via ureq → HTML
  ↓
scraper::Html parse
  ↓
qml_gen::generate_qml → big string of QML elements
  ↓
write to /tmp/web2qml_temp.qml
  ↓
QML loader replaces browse tab content
```

## Data flow on disk

`~/.config/web2qml/`:

- `voice_fingerprints.bin` — phonetic DB (FP04 binary format)
- `audio_profile_v2.bin` — speaker tonal profile + noise samples
- `tokens.bin` — token stats (filler classification)
- `confusion_table.bin` — learned whisper-mistranscription corrections
- `decision_cache.bin` — phrase → command mappings learned from confirmations
- `llm_cache.bin` — LLM Q&A cache
- `archive.db` — page archive
- `chat.db` — conversation history
- `speakers.toml` — speaker registry (v0.1.116+, human-readable)
- `workflows/<name>.toml` — saved workflows (v0.1.114+, human-readable)
- `models/ggml-*.bin` — whisper models (downloaded via setup-voice.sh)
- `data/*.toml` — user overrides for built-in data tables

`/tmp/web2qml_temp.qml` — generated browser content (overwritten on
each page load)

## QML structure

`qml/main.qml` is a single file (~8400 lines). Organized roughly:

1. ApplicationWindow setup + themeIdx
2. Theme definitions
3. Keyboard shortcuts
4. Global timers (calibration, smart-cd undo, workflow playback)
5. SwipeView with two tabs (browser + terminal)
6. Browser tab: URL bar, scroll area, drawer
7. Terminal tab: per-row Repeater, selection overlay, copy menu
8. Voice popup (recording UI, recognized text, candidates, speakers, words)
9. Settings popup
10. Other popups (file picker, find-anywhere chooser, etc.)

The QML refers to backend qt_methods exposed by `main.rs`'s `Backend`
struct via `qmetaobject-rs` macros. Adding a new backend method
requires three things:
1. Declare in `qt_method!(...)` macro block
2. Add to `Default::default()` initializer
3. Implement `fn the_method(&self, ...) -> ...` in the impl

## Threading rules

- **Qt main thread is single-threaded.** All QML interactions happen
  here. Long ops on this thread freeze the UI.
- **Workers communicate back via shared Mutex<T> state** that the main
  thread polls via timers (typical pattern: poll every 100-200ms).
  Avoids signal/slot complexity from worker threads.
- **No callbacks from worker to UI directly.** Worker writes to a
  shared queue/cell; UI poll picks it up.

## Build features

- Default features: voice recognition (whisper-rs).
- `--no-default-features` for builds without whisper (faster compile,
  smaller binary, useful for terminal-only builds).
- `cargo r --release --features voice` is the typical dev command.

## Where to add code

- **New voice intent** → `voice_intent.rs` (English) + dispatch in
  `voice_lang.rs` if language-specific. New Intent variant if needed
  + arm in `describe()` and `to_qml_string()`.
- **New terminal feature** → `term_blocks.rs` for command tracking,
  `pty_term.rs` for parsing/rendering, `main.rs` for qt_method exposure.
- **New browser rendering** → `qml_gen.rs`.
- **New voice diagnostic** → `voice.rs` for capture-time logic,
  `phonetic.rs` for matching logic, `speakers.rs` for identity logic.
- **New persistent data** → new file under `~/.config/web2qml/`, new
  module with load/save, register in startup sequence in `main.rs`.

## Naming conventions

- `qt_method!` for QML-exposed methods, snake_case (QML accesses
  `backend.snake_case`).
- Internal Rust functions: snake_case.
- Module-level state: `static FOO: OnceLock<Mutex<T>>` is the
  typical pattern.
- Logs: `[module-name] message` so grep finds them. e.g.
  `[voice-preview] ...`, `[speakers] ...`, `[workflow] ...`.
