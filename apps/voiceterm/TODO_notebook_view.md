# TODO: experimental notebook view (terminal as Jupyter-style cells)

User idea: run a shell like a Jupyter notebook — each command+output is a
"cell", structured output (tables, key/value maps) renders richly inline
via Qt, and touch gestures compartmentalize cells (collapse/expand/pin).
EXPERIMENTAL, behind a toggle in the terminal — must not disturb the
normal terminal view.

## Shipped (pure, tested — v0.1.608)
`src/notebook.rs`: `classify(output) -> Structured` where Structured is
Plain | Table(TableData) | Map(Vec<(String,String)>). Detects, in order:
markdown table (reuses table_ops::parse_markdown_table), CSV, TSV,
whitespace-aligned columns (ls -l / ps / df style), env-style KEY=value,
config-style key: value. CONSERVATIVE: returns Plain unless confident (a
false "table" would mangle normal output). 13 tests incl. the reject
cases (prose, inconsistent cols, one-row, single kv-line, url:// not
splitting). Reuses table_ops::TableData so the rest of the system (sort/
filter/markdown round-trip) already works on the result.

## The cell substrate ALREADY EXISTS
term_blocks::CommandBlock + BlockTracker already segment a session into
command+output blocks. A notebook "cell" = a CommandBlock. No new
segmentation needed — the notebook view iterates BlockTracker's blocks
and calls notebook::classify on each block's output.

## Build-gated wiring (needs the running build)

### 1. Backend: expose classified cells
- A qt_method like `notebook_cells() -> QString` (or a small model) that,
  when the experimental view is on, walks BlockTracker's blocks and for
  each emits {command, exit_code, classification, payload}. Serialize
  classification + payload (table → rows/cols; map → pairs; plain →
  text) over the existing wire-string convention.
- Gate this work behind the experimental flag so it costs nothing when
  off.

### 2. QML: the toggle + the view
- A toggle in the terminal toolbar / settings: `property bool notebookMode`.
  When false, the existing termRowsHost renders as today (zero change).
  When true, swap in a notebook ListView.
- ListView delegate per cell:
  - command line (monospace, like a prompt echo)
  - output rendered by classification:
    - Table → a Qt TableView / Grid (reuse any existing table rendering;
      table_ops::TableView exists for sort/filter affordances)
    - Map → a two-column key/value grid
    - Plain → the normal monospace text block
  - exit-code badge (green/red dot)

### 3. Touch gestures (the "compartmentalize" ask)
- Tap a cell header → collapse/expand its output (PropertyAnimation on
  height; keep collapsed state per cell id).
- Long-press / pin button → pin a cell to the top (keep it visible while
  scrolling — a pinnedCells list rendered above the ListView).
- Swipe a cell → dismiss/clear it from the notebook view (doesn't kill
  the underlying block, just hides in notebook mode).
- These are QML MouseArea/MultiPointTouchArea + state; the pure
  classification already gives stable per-cell content to attach state to.

## Sequencing (low-risk first, mirrors the ui_landmarks approach)
1. Backend notebook_cells() + the toggle, rendering Plain cells only
   (just compartmentalized command/output blocks — no rich tables yet).
   Dogfood that the cell segmentation + toggle feel right.
2. Add Table + Map rich rendering for classified cells.
3. Add touch gestures (collapse/expand, then pin, then swipe).
Each step is independently shippable + testable-by-eye; don't build all
three before dogfooding step 1.

## Why a toggle / experimental
Notebook mode reinterprets the whole terminal surface; keeping it a
toggle means the proven normal terminal is untouched and the experiment
can't regress daily use. Flip it on to try, off to fall back.

## Future (not v1)
- "expression mapping" could later mean evaluating a bare expression
  cell (type `2+3`, see `5`) — but that's just running it through the
  shell / a calc; the structured-output detection here is the part that
  makes results render richly. Revisit after the view exists.
- Re-running a cell in place (Jupyter's signature move) — needs the
  workflow-recorder replay machinery (TODO_workflow_recorder).
