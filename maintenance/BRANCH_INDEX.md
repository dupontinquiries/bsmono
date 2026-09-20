# Branch index after the five-artifact merge

Every uploaded artifact is on its own branch, so each is reviewable as one diff
and revertible as one commit. Nothing was flattened into a single import.

## Merged into `main`

| branch | contents |
|---|---|
| `incoming/monorepo-338` | the uploaded bundle, untouched (105 commits, all `apps/logscope`) |
| `session/local-91` | this session's work, untouched (91 commits, fourteen areas) |
| `merge/monorepo-338` | the reconciliation — **now `main`**. See `MERGE_338_AUDIT.md` |

Verified after merging: `gpu-gemm` compiles, `logscope-egui` **637/637** (up from
623 on the session side alone, so both sides' tests pass together), part-kit
scenes green, and this session's studio / rusty-board / snapclone / voiceterm /
gitmind work present and untouched.

## Imported, not yet merged

| branch | contents | why it waits |
|---|---|---|
| `import/kbm3` | 28 files: Python explorer + GPU component | needs a workspace/manifest decision |
| `import/skydribble` | v1.41 release drop, 40 Rust files + glTF | see `corpus/cadscope-skydribble` below |
| `import/cadscope` | three docs + the corpus refs | its bundle is a 760-commit fork; see `apps/cadscope/IMPORT_NOTE.md` |
| `import/cybertron` | 402 MB CMake C/C++ project + asset corpus | roughly doubles repo size — a deliberate decision, not an import side effect |

## Preserved refs (nothing lost)

| branch | contents |
|---|---|
| `corpus/cadscope-main` | the CADscope reference corpus, 760 commits, shares base `e71cc7c` |
| `corpus/cadscope-skydribble` | almost certainly skydribble's real development history |

## Follow-ups

**Corrected:** skydribble **v1.41 is the better keep** — `corpus/cadscope-skydribble`
is useful as *provenance* for a history that was never available, not as content
to merge back. And the CADscope corpus was built **on** logscope, using the
bundle as an egui-kit reference: a derived work, not an upstream. Neither needs
merging into `main`; both stay as refs so the lineage is not lost.

1. **Re-apply the granular `submit_cause` breakdown** over 338's frame logic. It
   is the one place the merge traded detail for coherence, and it was deliberate
   — recorded in `MERGE_338_AUDIT.md` rather than left to be rediscovered.

## Deliberate non-decisions

None of the four imports touches a workspace file or the manifest. An import
that also rewires the build makes it impossible to tell later which changes came
from upstream and which were integration choices. Integration is a separate
commit on top of each branch.

## upstream-492 (merged)

`20260719--monorepo-latest_492`, main at `370eedee`. 186 commits of logscope
work off the same `51d8c068` split as the 338 line. Merged into main; the seven
files both lines touched, the four textual conflicts and the one silent
theme.rs breakage are written up in `MERGE_492_AUDIT.md`.

## c19unified-016 — four drops merged 2026-09-19

Merged with `git merge`, not replayed. What that means for reading the history,
and what it cost:

- **Every incoming commit is present.** 161 commits between `2ebb81624` and
  `af0c8105f`, of which 127 came from the four bundles. `git log` shows them
  all, individually, with their original messages and hashes.
- **A first-parent view hides them.** `git log --first-parent` returns 34 —
  one line per merge instead of the 56/31/36/4 behind it. Most graph UIs
  default to that, so the history LOOKS collapsed while being intact. Use
  `git log --oneline --graph` or drop `--first-parent` to see the development.
- **What a replay would have bought and cost.** Rebasing the four branches onto
  a line would make the default view read as development, which is the
  legitimate complaint. It would also change every incoming hash. Three sibling
  sessions branched off these bundles — `logscope-speed` carries `2ebb81624` as
  its own `origin/HEAD` — so their bases would stop existing, which is exactly
  the failure CLAUDE.md §9 cites: "a prerequisite hash stopped existing and 81
  deltas became unrecoverable".

If a linear history is wanted, the time to replay is BEFORE the sending session
has branched again, and it should be agreed with them rather than done on
receipt.

### The drops are kept as branches

Each arrives unmodified at its original hashes, so the development reads
straight without `--first-parent` getting in the way:

| branch | tip | commits |
|---|---|---|
| `drop/clawbound-026` | `95b55e105` | 56 |
| `drop/gdb-017` | `f19fc7c958` | 39 |
| `drop/gdb-018` | `3e2583f8a` | 43 |
| `drop/logscope-speed-028` | `936e626e4` | 36 |
| `drop/forge-playable` | `7c431b962` | 4 |

`git log drop/logscope-speed-028` reads as that session wrote it. `git log
--graph main` shows how it landed.

`drop/gdb-017` is kept even though `gdb-018` strictly contains it, because
"this one was superseded" is a fact about the drops worth being able to see
rather than infer from a missing branch.

These are cheap — a branch is one ref — and they are what makes merging rather
than replaying an honest choice instead of a lossy one. `export-bundle.sh` uses
`--all`, so they travel.

### Author identity

Commits in this repo were being made as `c18unified agent` — a stale
`git config user.name` inherited from the c18 line. Corrected to
`c19unified agent` going forward. The 34 commits already made under the old
name are left alone: fixing them means rewriting the same hashes the point
above says not to rewrite, to correct a label that is wrong but not
misleading — the dates and content place them unambiguously.
