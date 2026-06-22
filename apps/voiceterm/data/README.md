# Data tables

Runtime-loaded tables that drive smart navigation, voice intent recognition,
and other heuristics throughout web2qml.

## Files in this directory

These ship with the binary as defaults. Editing them means rebuilding the
project. For overrides without rebuilding, use the user-overlay path below.

- `project_markers.toml` — files that signal "this is a project root"
  (drives the `go deeper` voice command's stop condition)
- `skip_dirs.toml` — directories smart cd will never enter (node_modules
  etc.)
- `source_exts.toml` — file extensions that count as "source code"
  for scoring purposes

## Other files (generated, NOT user-editable)

These also live in `data/` but are **not** user-overridable tables — they're
build artifacts checked in for convenience (so contributors don't need every
tool installed to compile):

- `phoneme-db.txt` — phonetic transcriptions of bundled manpage paragraphs,
  produced by `scripts/gen-phoneme-db.py` from `data/manpages/*.md` via
  espeak-ng. Loaded via `include_str!` in `src/phoneme_db.rs`. Drives the
  reading-pace alignment system (PA arc, v0.1.242+). Regenerate after
  editing any bundled manpage:
  ```
  python3 scripts/gen-phoneme-db.py
  ```
  The file's header now carries a DO-NOT-EDIT banner as a reminder.

- `manpages/` — markdown source files for bundled documentation rendered
  at `web2qml://man/<name>`. Edit freely; regenerate phoneme-db.txt
  after changes if the PA arc is in use.

## User overlay

Create the same files at `~/.config/web2qml/data/<name>.toml` to extend
or override defaults. Format is identical.

The user file's top-level array EXTENDS the default list. Duplicate entries
are deduplicated (default order preserved, user additions appended).

To REMOVE a default, use the `[remove]` section:

```toml
# ~/.config/web2qml/data/skip_dirs.toml
# I'm a Go developer and I DO want to navigate into vendor/

[remove]
skip_dirs = ["vendor"]
```

To both add and remove:

```toml
# Add my own custom project markers
project_markers = [
  ".my-project",
  "my-config.yaml",
]

# But remove a default I don't care about
[remove]
project_markers = ["go.mod"]
```

## Format notes

- Each file has at least one top-level array assignment with the table name
  matching the filename (e.g. `project_markers.toml` contains
  `project_markers = [...]`)
- Strings are double-quoted. Escape `\"` and `\\` are supported.
- Trailing commas in arrays are fine.
- `#` line comments OK at start of any line or inline after a value.
- Multi-line arrays OK (closing `]` can be on a later line).

## Loading

Tables load eagerly at app startup via `data::init()`. A malformed user
file is logged to stderr and skipped — defaults still take effect. The
parser is intentionally minimal (no nested tables, no arrays of tables)
so error messages stay simple.

If a file is absent, the binary's compiled-in defaults are used.

## Adding new tables

To add a new runtime-loaded table:

1. Edit `src/data.rs`:
   - Add a field to `AllTables` struct
   - Add a `default_<name>()` function with the embedded fallback list
   - Add a match arm in `apply_dir()` for the table key
   - Add a `pub fn <name>()` accessor
2. Create `data/<name>.toml` with the same defaults (so users see the
   shipped data inline)
3. Wire callers to use the `data::<name>()` accessor instead of any
   hardcoded list

The pattern mirrors the existing three tables (`project_markers`,
`skip_dirs`, `source_exts`).
