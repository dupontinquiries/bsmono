# Path completion — clean-room from autocomplete-paths (v0.1.682)

## Provenance (clean-room)
Studied the MIT-licensed Atom `autocomplete-paths` package (provided as an
example zip) for its IDEAS only. src/path_complete.rs is an INDEPENDENT Rust
reimplementation — no code copied, no JS translated line-by-line. Borrowed
concepts (all re-derived + re-expressed):
1. Trigger on a scope + a prefix (theirs: `import ... from '`; ours: inside a
   ZBracket `[[` or a markdown link/image `](`).
2. Directory-aware listing when the typed prefix has `./`, `../`, or a
   trailing slash → list that directory; else fuzzy across the project.
3. Per-context extension filtering.
4. Fuzzy match weighted by DISTANCE-TO-FILE (closer files rank higher) —
   their `score = count - i + (maxDistance - distance)`; ours `fuzzy + 2/(1+
   distance)` (re-derived, not their exact formula).
5. Relative-path insert with optional leading `./` + suffix (extension)
   stripping.
Our fuzzy scorer + relative-path math are our own (subsequence streak scorer;
up/down segment diffing).

## Pure core (9 tests, GREEN)
parse_path_prefix, relative_path, fuzzy_score, complete(candidates,
current_file, raw_prefix, opts) -> ranked PathCompletion{insert, display,
score}, CompleteOpts{extensions, relative, include_current_dir,
strip_suffixes, max_results}.

## STATUS: NOT WIRED — awaiting greenlight
Per the user's "cleanroom then I will greenlight the impl." The module is
compiled (dead_code-allowed) + tested but unused. 

## Proposed wiring (for greenlight)
- A qt_method `path_completions(current_file, raw_prefix, exts, max)` →
  "insert\tdisplay" rows, fed by path_fuzzy::PathIndex::text_file_paths (or a
  broadened enumerator incl. images for `![](`).
- In the note editor: when zb_editing_context reports a `[[`-open OR the line
  matches a markdown-link/image prefix (`](` / `![](`), call path_completions
  and show the dropdown; accept inserts `insert`.
- Opts per context: ZBracket link → strip `.md`, include `./`; image `![](`
  → extensions [png,svg,jpg,gif,…], keep extension.
- Also usable in the search bar for `file:` path entry.

## Arrow keys (SHIPPED v0.1.682, separate ask)
urlSuggestPopup gained _selIndex; urlBar Keys.onPressed handles Down/Up
(wrap-around) + Enter (activate selected); the selected row is highlighted.
Independent of the path-completion core.
