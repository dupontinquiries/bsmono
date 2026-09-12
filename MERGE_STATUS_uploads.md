# The eleven uploaded artefacts, triaged

Every upload was checked against this history rather than merged on faith. Four outcomes.

## Already landed — nothing to do

| artefact | tip | evidence |
|---|---|---|
| `logscope-file-reading-check--c15unified-008` | `e14d503a1` | ancestor of HEAD |
| `check-if-included--voiceterm-session` | `8d9b0a1d6` | ancestor of HEAD |
| `to-check-if-landed--blackstar--files` (both nested bundles) | `369d5607c` | ancestor of HEAD |
| `to-check-if-landed--blackstar-2--files` | `95fb20548` | ancestor of HEAD |

Those four were named "check if landed". They had.

## Merged this session

* **`aperture-work-02--files.zip`** → the nested `aperture-entities.bundle`: 6 commits, 16
  files (entity, nav, bucket, and their handoff docs). One overlapping file, root `TODO.md`,
  merged without a marker.
* **`idk-logscope-dev--c15unified-029`** → `SESSION_CHANGES.md`, a 264-line changelog. Merged
  as DOCUMENTATION only, after confirming every commit it cites (`de60b1fc`, `70dd3889`,
  `c0a5dec1`) is already an ancestor and `MakeTool` is present in `workspace_exec.rs`. A
  merge-aid document is worthless if the merge happened and misleading if the code is absent;
  neither is the case, so what remains is a record of reasoning the commits do not carry.

## Cannot be merged — incomplete bundles

* **`voiceterm-something--c15unified-013-since-010-partial.bundle`**
* **`card-diagnoser.bundle`** — needs prerequisite `c7339651acb93ef54092be5fefc7a283610f12fa`

Both are INCREMENTAL bundles whose base commits are not in this repo, so their packs are
missing objects their trees reference. Nothing can extract them here; they need re-exporting
from the machine that made them, with `--all` or against a base this repo has.

## Merge attempted, real conflicts — left alone deliberately

* **`FAO-codec-01--files.zip`** → `d2x-12commits.bundle`: 4 commits, 12 files, **1 conflict**.
* **`blackstar-coldopen--files.zip`** → `c14unified-024-delta.bundle`: 49 commits, 32 files,
  8 of them touched on both sides, **3 conflicts**.

These share history and would merge with hand resolution. They were not forced: a conflicted
merge resolved carelessly across 49 commits is worse than an unmerged branch, because the
branch is still there and the bad resolution looks finished.

Both are fetched and available locally as `refs/remotes/z2-d2x-12commits/head` and
`refs/remotes/z2-c14unified-024-delta/head`, so resolving them is `git merge` plus reading —
no re-upload needed.

### A trap worth recording
Those two bundles carry ONLY `HEAD`, not `refs/heads/*`. A fetch with a `refs/heads/*`
refspec reports no error and creates no ref — it silently does nothing. I read that as "no
shared history" until I listed the refs and found none had been created. If a bundle appears
to have an unrelated history, check what refs it actually carries first.

## Not code
`blackstar-film-files-v5--files.zip` (a nested `blackstar-film-vault-v5.zip` plus MORPHER_MODE
and ELEMENTS docs) and the loose `.mkv` clips in the FAO zip are assets and prose, not commits.
Left where they are.


---

## Second pass, with two more uploads (2026-09-08)

### `compositor-full-session.bundle` — MERGED, and it unblocked another

This carries `c7339651`, the prerequisite `card-diagnoser.bundle` was missing. That was the
whole reason card-diagnoser could not be opened: not a corrupt bundle, just an incremental one
whose base lived in a session that had not been shipped yet.

With it present, card-diagnoser verifies — and turns out not to need merging separately, because
`git merge-base --is-ancestor` shows it is CONTAINED in this line. One merge got both.

24 commits: nfc-kit's `memory.rs` (READ BINARY / UPDATE BINARY, the ClaimTicket), a verified
card-name table replacing a hand-picked one, and a real false-negative bug found by checking
that table rather than trusting it.

The single conflict was `Cargo.lock`. Taken as ours and left to cargo, which reconciled it on
the next check — hand-merging a lockfile is how a tree ends up pinned to versions neither side
ever built. voiceterm-egui went 1584 -> 1605 tests.

### `aperture-todos-buckets-all--files-no-full.zip` — ALREADY LANDED

Both bundles inside (`aperture-entities.bundle` and `aperture-entities-since-eccca.bundle`)
have the same tip, `1e86c97f5`, which is an ancestor of HEAD — it was merged in the previous
pass. Checked past the hash as well: `entity.rs`, `nav.rs` and `bucket.rs` are all in the tree,
and the zip's `TODO_entities_buckets_paths.md` is byte-identical to the in-tree copy. Nothing
to do.

### Still blocked, and now precisely
`voiceterm-013-since-010-partial` was re-verified with `c7339651` present and STILL lacks its
prerequisites — a different missing base, not the same gap. It needs a re-export from whichever
session holds `c15unified-010`.
