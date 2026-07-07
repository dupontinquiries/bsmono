# thread_tool — extractor / classifier / merger for the worker pipeline

Standalone Rust (no cargo/deps). Build once, run from repo root:
```
rustc -O tools/thread_tool.rs -o /tmp/tt
```

## Why
Fan work out to several sessions (e.g. 5 Claude sessions WITH a workspace + 1 local ollama
session WITHOUT one) in one shot, then fold all their returns into a single super zip — few
chat messages, normal workflow. Everything is zip-consolidated to keep file count down.

## extract — dump labeled instruction bundles
```
/tmp/tt extract --manifest threads.txt --out threadwork
```
Writes into a labeled working dir `threadwork/` plus `threadwork/INDEX.txt`
(`ROLE  MODE  ID  FILE`):
- `mode: zip`   → `threadwork/<ID>.instr.zip` = base (minus `target/`) + `WORKER_ID` +
  `WORKER_BRIEF.md`. Hand to a session that can run code. **One zip per worker** (low file count).
- `mode: embed` → `threadwork/<ID>.brief.md` = self-contained brief with the named files
  inlined; paste into the no-workspace (ollama) session. (Kept as `.md` because it's pasted,
  not unzipped.)

Manifest stanzas split by a line of `---`; `#` ignored; keys `id/mode/brief/embed`. Template:
`tools/threads.example.txt`.

## scan — classify a folder (instruction vs result)
```
/tmp/tt scan --dir changes [--write]
```
Sorts every file into:
- **INSTRUCTION** — a dispatch bundle (zip containing `WORKER_BRIEF.md`/`WORKER_ID`, or a brief
  `.md`). These are things you SENT, not results.
- **RESULT** — a worker return: `*.patch`/`*.diff`, a `*SOLUTION*` zip, a zip of changed
  sources, or a raw `.rs`.
- **IGNORE** — anything else.
`--write` saves `changes/INDEX.txt`. Use this to sanity-check a folder before merging, or just
let `merge` do it. You can safely dump dispatches AND returns into the same folder.

## merge — apply only the results, roll one super zip
```
/tmp/tt merge --returns changes --out unendlich-super-merged.zip
```
- Auto-classifies (same as `scan`); **INSTRUCTION bundles are skipped** (logged `SKIP-INSTR`).
- `*.patch` → `patch -p1 --forward` (dry-run-gated; cleanly-failing → `SKIP-FAIL`, never forced).
- result `*.zip` → if it contains a `.patch`, that's applied; otherwise its `crates/**.rs` /
  `src/**.rs` are copied over the base (paths anchored at `crates/`/`src/`, so wrapped or
  unwrapped zips both land correctly).
- raw loose `.rs` (unknown target path) → logged `MANUAL`, place by hand.
- Rolls the super zip (minus `target/`) + `MERGE_LOG.txt` inside it; leaves the work tree in
  `.tt_merge_work/` for inspection.

## ALWAYS compile-check after merge (mechanical step only)
```
cd .tt_merge_work && cp Cargo.lock /tmp/lock.v4 && rm -f Cargo.lock \
  && cargo check -p unendlich-engine --features headless ; cp /tmp/lock.v4 Cargo.lock
```
Resolve `SKIP-FAIL` patches by hand. Bin-only changes won't compile here (need `window`).

## Limits
- A return zip that re-includes the WHOLE base but omits `WORKER_BRIEF` classifies as RESULT and
  will copy all its `.rs` over base (same-content files are harmless; the compile-check is the
  backstop). Prefer workers return just changed files or a patch.
- The merger applies clean patches and logs the rest; it does not resolve conflicts.
- Embedded (ollama) returns as `// FILE: <path>` blocks: save each to its path, zip, feed to merge.
