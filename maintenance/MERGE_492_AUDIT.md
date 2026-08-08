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

---

# Merge 605 (c4-541)

Second upstream merge. Same base as the first — `370eedee`, so this line has not
moved since — with **122 incoming commits, every one under `apps/logscope`**,
against 262 of ours.

## It merged clean, and this time clean meant clean

Five files were touched by both sides (`backend.rs`, `lib.rs`, `scene3d.rs`,
`tabs/chat.rs`, `logscope-fan/src/lib.rs`) and git auto-merged all five with no
conflicts. Last merge that was not the end of the story — `theme.rs` auto-merged
and then failed to compile with `E0428`, because both lines had added
`md_strong()` in different positions. So the merge is not accepted until it
builds:

| build | result |
|---|---|
| `--features gui` | **745 passed** (was 703 — 42 new upstream tests) |
| `--no-default-features` (lean) | **498 passed** (was 483) |
| `logscope-fan` | **247 passed**, 1 ignored |

All three of our earlier logscope changes survived: the `dev-views` gate, the
un-gated `workspace`/`merge`/`hud_pack` modules that make the lean build
compile, and `ensure_contrast_all`.

## Concerns

**1. Upstream added `apps/logscope/.cargo/config.toml` with
`-C link-arg=-fuse-ld=gold -C prefer-dynamic`.** Both work here — `ld.gold` is
present — but this is a new hard requirement on the build host that did not
exist before, and it is per-app rather than workspace-wide. A machine without
the gold linker now fails to build logscope and only logscope, which is a
confusing way to find out. `prefer-dynamic` also changes what a release binary
depends on at runtime; worth confirming that is intended for shipped builds
rather than a local iteration speed-up that escaped.

**2. A misleading error I previously mis-diagnosed.** `failed to run rustc to
learn about target-specific information` appeared during this merge check. The
audit above records that message as meaning *disk exhaustion*. That was
incomplete: here it was caused by redirecting `</dev/null` into cargo, which
feeds the rustc probe an empty stdin. Two different causes, one message, and
neither mentions the real problem. Check disk AND stdin before believing the
toolchain is broken.

**3. Two reverts came in, one of which is a correction of a bad measurement.**
`revert(arena): pull the slab unmap — and I compared two different code paths to
justify it` and `revert(q4k): ties-to-even made it 8.8x WORSE — Metal rounds
ties AWAY, whatever the spec says`. Both are healthy, but the first is worth
reading before building on that area: it says a previous optimisation was
justified by comparing two different code paths, which is the same class of
measurement error this port's own audit keeps finding.

**4. No new dependencies**, which is the good news in a 122-commit merge.

## Follow-up: was the quantization work adopted? Yes, and here is the proof

Asked directly, because that work was buggy, is now less so, and had compiled
upstream. Three checks, in increasing strength.

**1. What we touched.** Since the last merge, our line changed exactly FIVE
files under `apps/logscope`, in two commits:

```
check-features.sh
render-scene.sh
crates/logscope-egui/Cargo.toml     (dev-views feature)
crates/logscope-egui/src/lib.rs     (module gating for the lean build)
crates/logscope-egui/src/theme.rs   (ensure_contrast_all)
```

Nothing under `gpu-gemm`, nothing quantization-related, nothing a kernel or a
quantizer lives in.

**2. What differs from upstream.** The merged tree differs from upstream in 117
files under `apps/logscope` — which sounds alarming and is not. Upstream changed
16 files under `gpu-gemm`; our line changed 93 there, all pre-dating this merge.
Of the files **upstream actually touched**, only five differ, and they are
exactly the five both sides edited: `backend.rs`, `lib.rs`, `scene3d.rs`,
`tabs/chat.rs`, `logscope-fan/src/lib.rs`. A merge is supposed to differ from
both parents precisely there and nowhere else.

Every quantization file upstream touched — `gpu-gemm/src/quantref.rs` (new, 268
lines) and `docs/q4k-scale-ulp.md` — is **byte-identical to upstream** in the
merged tree.

**3. A gap in the original verification, now closed.** The merge commit reported
745 + 498 + 247 tests passing. **None of them compiled a line of gpu-gemm.** It
is `exclude`d from the workspace and only enters through the optional
`wgpu-engine` feature, so all three configurations skipped the crate the
quantization work lives in. Saying "it builds" on that evidence was too strong.

Built separately, and it holds:

| | |
|---|---|
| `cargo check --lib` in `gpu-gemm` | compiles, 7 warnings |
| `cargo test --lib` | **543 passed** |
| `cargo test --lib quantref` | **12 passed** |

The twelve are the tolerance contract from `feat(quantref)` — the q1/q2/q3
bounds and the corrected tie rule from `fix(gate-quantref): the gate asserted
the spec's tie rule too — corrected to the measured one`.

**Standing concern.** gpu-gemm being outside the workspace means a normal
`cargo test` in `apps/logscope` will never build it, so a regression there is
invisible to anyone who has not read this note. `check-features.sh` should grow
a gpu-gemm arm — it already exists to stop exactly this class of "the flag
nobody exercises" problem.

---

# Merge 670 (c4-573)

73 upstream commits from `f498feb8` — the 605 tip — against 519 of ours.
Auto-merged clean, no conflicts. Two files were touched by both sides
(`backend.rs`, `scene3d.rs`) and git resolved both.

| build | result |
|---|---|
| `--features gui` | **778 passed** (was 745) |
| `--no-default-features` (lean) | **527 passed** (was 498) |
| `logscope-fan` | **247 passed** |
| voiceterm egui-frontend | **613 passed** |
| `gpu-gemm` | **NOT RUN — see below** |

voiceterm matters here because upstream changed `src/table_ops.rs`, which the
egui port pulls in with `#[path]`. Upstream engine changes flow straight into
the port, and its tests are what confirm they still hold.

## The gpu-gemm arm cannot run in this container, and I had the cause wrong

`check-features.sh` gained a gpu-gemm arm last merge precisely so the
quantization work would not go unbuilt. It fails here, and the message is the
same misleading one:

```
error: failed to run `rustc` to learn about target-specific information
  ...
  error: couldn't read from stdin, as it did not contain valid UTF-8
```

**The audit above blames this on redirecting `</dev/null`. That is wrong.**
Tested directly: `< /dev/null` does not fix it, foreground does not fix it,
`RUSTFLAGS=""` does not fix it, and naming `--target` explicitly does not fix
it. `rustc - --print=cfg` works fine when run by hand with either empty or real
input, so rustc and the toolchain are healthy.

What is actually happening: gpu-gemm is `exclude`d from the workspace, so cargo
runs a **host probe** for it — `rustc -` reading the source from stdin — and in
this execution environment the inherited stdin is not valid UTF-8. The
workspace crates never hit this path because their target info is already
resolved.

So the third cause is neither disk nor a redirect: it is *this container's
stdin*, and the arm is unrunnable here rather than broken. It should still run
on a normal host. **The quantization crate is therefore unverified in this
merge**, and that is a gap, not a pass — the last merge did verify it (543
tests) and this one could not.

## Concerns

**1. The prefer-dynamic config is still there** and now has a second
consequence. `-C prefer-dynamic` and `-C link-arg=-fuse-ld=gold` appear in the
failing probe command, which is how they came to my attention twice. They are
not the cause here, but a per-app `.cargo/config.toml` that only affects one
directory tree keeps producing confusing failures in the crate that sits outside
the workspace.

**2. Upstream now touches snapclone and voiceterm**, which it did not before —
`snapclone-dht/src/identity.rs`, both NAT and DHT manifests, and
`voiceterm/src/table_ops.rs`. Those are engine crates the egui ports depend on
or include, so future merges need the port test suites run as part of accepting
them, not just logscope's. Done here; worth keeping.

## A third cause for "failed to run rustc to learn about target-specific information"

Already recorded twice: disk exhaustion, and redirecting `</dev/null` into cargo.
There is a third, and it cost a chunk of a session before I found it — **a
corrupted CARGO_TARGET_DIR**. The same crate that failed this way from
`/tmp/uk` built and tested clean from a fresh `/tmp/uk2` with no source change
between the two.

The tell is that another crate builds fine at the same moment with a different
target dir. If disk is healthy and stdin is a terminal, delete the target
directory before believing anything about the toolchain — the message names
rustc and means nothing of the sort in any of its three causes.

### The corrupted-target-dir failure is frequent, not a curiosity

Recorded once as a third cause. It has now happened three times in three
sessions, always the same way: a workspace that tested clean minutes earlier
fails with `exit status: 101` and no diagnostic, or with a rustc probe error,
and a fresh CARGO_TARGET_DIR builds it with no source change.

It correlates with the disk filling — this container runs at 90%+ and a target
directory written during an out-of-space moment stays broken afterwards. So the
first response to an inexplicable build failure here is `rm -rf` the target dir
and retry BEFORE reading the error, not after.
