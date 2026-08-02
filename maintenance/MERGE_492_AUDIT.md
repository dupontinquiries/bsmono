# MERGE 492 — the logscope line into c4

Merging `20260719--monorepo-latest_492` (main at `370eedee`) into the c4 line.

## What the two lines are

They are not ancestor and descendant. `git merge-base` puts the split at
`51d8c068` — `incoming/monorepo-338`, the same point the previous merge audits
start from — with **186 commits** on the upstream side and **211** on c4.

The reassuring part is what each side spent those commits on:

| | areas touched since the split |
|---|---|
| upstream-492 | `apps/logscope` — and nothing else |
| c4 | 37 areas: the egui port programme, the shared uikit, the parity tooling |

Upstream's 186 commits are logscope work. c4's 211 are the port programme. The
lines are close to orthogonal, which is why this is a merge and not the
reconciliation `MERGE_PLAN.md` describes.

## Conflict surface

104 files changed upstream in `apps/logscope`, 114 on c4, and **7 in both**:

```
crates/logscope-egui/src/app.rs          auto-merged
crates/logscope-egui/src/backend.rs      CONFLICT (2 hunks)
crates/logscope-egui/src/lib.rs          auto-merged
crates/logscope-egui/src/tabs/chat.rs    CONFLICT (2 hunks)
crates/logscope-egui/src/theme.rs        auto-merged — and WRONG, see below
crates/logscope-fan/src/coordinator.rs   auto-merged
crates/logscope-fan/src/lib.rs           auto-merged
```

## The four textual conflicts

Every one of them was two lines fixing the same code and each catching a
different half. None was a case of picking a winner.

**1. `backend.rs` — the autotune cache load.** Both sides found that a cached
decode config was being trusted too readily, and found different reasons:

* c4 found that the *value* was not validated. The field values that looked like
  poison (2147549188) are `0x8001_0004` — packed `DecodeConfig`s, high bit set,
  kernel id in the high half, tile in the low. A naive range check would have
  discarded every tuned config on every machine that had one.
* upstream found that the *kernel* was not re-checked. The autotune search skips
  subgroup kernels when `sg_ok` is false, but a restored choice skipped that
  check — so a cache written under a different build, driver, or
  `LOGSCOPE_MATVEC_SG=1` selects `V2WgSg` where the guard has since disabled it.
  Visible in a real log as "subgroup decode kernel DISABLED" three lines above
  "kernel V2WgSg, cached autotune".

Both guards are kept, in that order. A cache is a path that skips the code that
would have said no, and it needs every check the live path has.

**2. `backend.rs` — the bottleneck label.** upstream corrected an overstatement:
"GPU-execution-bound" was asserted whenever setup and sync were under 15% of wall
time, which says where the time *is not*. At the 0.1% seen in real runs, 99.9% is
unaccounted, and unaccounted is not a bound — hence the new `UNACCOUNTED` tier
and `perfverdict`. c4 had added a submit-cause breakdown naming *why* the submits
happened. Kept both: upstream's tiers and verdict, then c4's cause histogram.

**3. `chat.rs` — a type block.** Purely additive on both sides: c4 added `CtxMsg`,
upstream added `impl branchtree::Node for Msg`. Both kept.

**4. `chat.rs` — a comment.** Identical code (`theme::md_strong()`), two
descriptions of the same fix. Merged into one.

## The conflict git did not report

`theme.rs` **auto-merged cleanly and did not compile.** Both lines had added
`pub fn md_strong()` with an identical body and different doc comments, in
different places in the file, so git took both:

```
error[E0428]: the name `md_strong` is defined multiple times
```

This is the entry worth remembering from this merge. A textual merge tool cannot
see that two independent additions are the same definition, and a clean `git
merge` output is not evidence that the result builds. The duplicate was removed,
keeping the fuller comment — the one explaining why the ratio is 7:1 rather than
the 4.5:1 body floor, since egui's default fonts carry no bold weight in this
pipeline and colour is therefore the entire bold signal.

## Verification

Everything below was run on the merged tree, not on either parent.

| crate | tests |
|---|---|
| logscope-egui | 700 |
| logscope-fan | 239 |
| snapclone-egui | 264 |
| voiceterm-egui | 264 + 5 golden-corpus |
| bo-suite-egui | 170 |
| uikit-web | 35 |

`parity_audit.py` reports the same figures as before the merge — bo-suite 51%,
snapclone 55%, voiceterm 55% — which is the expected result, since upstream
touched none of the ported apps.

## One environment note for whoever runs this next

The container hit 96% disk during the logscope build (its `target/` alone is
2.9 GiB) and `cargo` failed with `failed to run rustc to learn about
target-specific information` — a message that has nothing to do with the real
cause. If cargo starts reporting impossible toolchain errors, check `df` before
anything else, and build with `CARGO_TARGET_DIR` outside the repo so the trees
can be reclaimed one at a time.
