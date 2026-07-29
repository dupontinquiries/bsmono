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

## The two follow-ups that matter

1. **Reconcile `corpus/cadscope-skydribble` with `import/skydribble`.** The
   import is a v1.41 archive with *no history*; that branch appears to hold the
   actual commits. Doing this replaces a single import commit with a real
   lineage, and it is self-contained.
2. **Re-apply the granular `submit_cause` breakdown** over 338's frame logic. It
   is the one place the merge traded detail for coherence, and it was deliberate
   — recorded in `MERGE_338_AUDIT.md` rather than left to be rediscovered.

## Deliberate non-decisions

None of the four imports touches a workspace file or the manifest. An import
that also rewires the build makes it impossible to tell later which changes came
from upstream and which were integration choices. Integration is a separate
commit on top of each branch.
