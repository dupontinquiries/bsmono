# c4-245 reconciliation — three drops onto the monorepo

Four uploads arrived alongside the `logscope-c4-245` bundle. Three of them are
project drops that needed merging; one is redundant.

| Upload | Target | Outcome |
| --- | --- | --- |
| `logscope-c4-245.bundle` | the repo | git bundle, `main` @ `a8cd4e4` — the base for this work |
| `blackstar.zip` (Jul 26) | `games/blackstar` | merged, 5 conflicts, 1 design collision left open |
| `20260717blackstar.zip` (Jul 17) | `games/blackstar` | **not merged — redundant** |
| `kbm3.zip` (`598bce77`) | `tools/kbm3` | merged clean, kept a fix the drop predates |
| `unend-r746.zip` | `games/unendlich` | merged clean, 3 pre-merge decisions |

## The one thing that mattered most

**None of the drops are descendants of the monorepo copy.** Every one is a
divergent line that branched from an earlier import point and moved on while
the monorepo also moved on. Unpacking any of them over its directory — the
obvious reading of "merge the improvements in" — would have destroyed work:

- **blackstar**: neither zip contains `gates.c/h`, `bsgames_handoff.c/h`,
  `test_gates.c`, `test_handoff.c` or `arena_forge.py`. A copy would have
  deleted commits `d334fe3`, `6440ec5` and `3ca74d7` outright.
- **kbm3**: the drop's `population.rs` is byte-identical to the pre-`b3bc791`
  version — it branched before the fix rather than reverting it. A copy would
  have reintroduced an `E0433` build break on a virgin clone.
- **unendlich**: the archive lacks `pngutil.rs`, `sim/netview.rs` and
  `bin/unendlich-netview.rs`, which are monorepo work (`d59cf94`, `983b7ed`).

So each drop was committed to a `drop/<project>` branch **rooted at its own
import point**, then merged into `main`. Git then had a real merge base and
could tell "they changed it" apart from "they never had it". The three merge
commits each have two parents; the drop branches are kept.

Merge bases used — chosen by measuring divergence against every candidate, not
by assuming the latest:

| Drop | Base | Why |
| --- | --- | --- |
| blackstar | `927207a` | the games/ import; both lines diverge from it |
| kbm3 | `bf5cda2` | drop's `population.rs` is byte-identical to this commit's |
| unendlich | `000e92d` | the r742 adoption; this drop is r746 |

## blackstar

3805 paths: 3200 drop-only, 11 monorepo-only, 5 diverged on both sides.

- `menu.h`, `menu.c` — the drop is a strict superset. It already carries
  `EX_MODE_ESCALATION` plus nine further modes and an `ex_mode_desc()` table.
  Took the drop.
- `CMakeLists.txt` — unioned the ctest list: the drop's 12 new targets plus our
  `test_gates` and `test_handoff`, 54 total. Both sides' `CYBERTRON_MODULE_SRC`
  additions auto-merged (`bsgames_handoff.c`, `gates.c` ours; `combat_json.c`,
  `commander_ai.c`, `hive_ai.c`, `behavior.c` theirs).
- `SIGNATURES.md` — both lines appended a changelog starting at "Update 35".
  Kept both: the drop's 35–375 in place, ours renumbered 376/377.
- `game.c` region 1 (wave delay) — drop is a superset, took it.
- `game.c` region 2 — see below.

Verified: **cmake configure + full build + `ctest` all green — 64/64 tests
pass**, from a fresh clone of the output bundle. That covers `test_gates` and
`test_handoff` (ours) alongside `test_behavior`, `test_combat_json`,
`test_commander_ai`, `test_hive_ai`, `test_bodykind` and the drop's six
Python-driven tests (`content_validate`, `lint_dead_fields`, `silhouette`,
`map_score`, `openworld`, `gltf_roundtrip`). The `game` binary links. Six
compiler warnings, all in code neither side touched (`msdf.c`, `save_config`,
`test_arena_iqm`) and all present before the merge.

### OPEN: two escalation systems now coexist

Not a text conflict — a design collision. Both lines independently built a
"spend currency, push the arena perimeter out" mechanic for Escalation:

- the drop's **station/zone box** — `g_esc_box`, starts at 26, +18 per zone
  station bought, spends `g_energon_credits`;
- our **buy-a-door gate chain** — `gates.c`, extents 100/150/220, spends
  `g_waves.energon`.

Both survive the merge and both are live. They use "largest opened wins"
semantics, so they compose: the drop's box is the baseline and an opened gate
can only push further out. A plain `max()` was rejected — `ex_gates_extent()`
floors at `60.0f` and would have clobbered the drop's deliberately tight 26
start.

Nothing is lost, but **these are duplicate features for one game mode and one
should be retired.** That is a product decision, not a merge decision; it is
marked `TODO(design)` at the site in `engine_src/game.c`. Note also that
`EX_MODE_ESCALATION`'s ordinal moved from 2 to 4 — check anything that
persists a mode index to disk.

## kbm3

22 files unchanged, 5 drop-only, 1 ours-only, nothing diverged — the cleanest
of the three. The one-sided file is the whole point: `population.rs` keeps
`b3bc791`'s `#[path = "../audio.rs"] mod audio;`. `data.rs` is shared into
every bin via `#[path]`, so without that declaration the population bin's
crate root has no audio module.

Landed from the drop: RpGAN relativistic pairing loss (Rust and Python), R1/R2
zero-centered gradient penalties with lazy regularization on the Python side,
and Burn `fusion`/`autotune` feature flags plus an opt-in `spirv` backend.

Not verified: `cargo check` could not run. Ubuntu 24.04 ships cargo/rustc
1.75, and the dependency graph needs ≥1.85 — `zeroize v1.9.0`, pinned in the
existing lockfile, is itself edition-2024. `rustup` is unavailable and
`static.rust-lang.org` is outside the sandbox allowlist, so no newer toolchain
can be fetched. This gap is pre-existing and unrelated to the merge — the
monorepo already notes (`89f3c41`) that unendlich needs rust 1.96. Hardware
step.

## unendlich

Auto-merged with zero conflicts, but three things had to be settled *before*
the merge rather than during it.

1. **The 4 missing files.** Restored on the drop branch from the base, so the
   merge does not read "theirs deleted these". Committed as-is, the merge
   would have silently removed a refactor and a feature.

2. **35 `.spv` that only looked diverged.** Every `.spv.hash` sidecar is
   byte-identical between the two lines, so both sides compiled the same GLSL
   and the binaries differ only by compiler nondeterminism. No shader *source*
   diverged. `.spv` is gitignored as a build artifact, so the drop branch
   carried none and the merge kept ours — equivalent, and no churn. Those 37
   files are the **only** things git ignored out of the archive; nothing else
   was skipped.

3. **Stray `frag.spv` / `vert.spv` at the archive root** — build output that
   escaped `assets/shaders/`, matching no source and referenced by nothing.
   Not carried in.

`Cargo.toml` auto-merged: our `netcode` path dependency alongside the drop's
r742 → r746 version bump.

### MANIFEST.sha256 is a baseline, and it is stale

`RETURN_KIT.sh finish` uses `MANIFEST.sha256` to report which files a worker
changed relative to the drop they were handed (`+` new, `~` changed). It is a
record of the shipped state, not an integrity gate on the current tree.

It was already stale when it arrived: checked against the **raw archive**, 288
entries FAIL and 12 are missing, out of 1624. The merge adds exactly 15 more,
and all 15 are correct outcomes:

- 14 `assets/shaders/*.spv` — we kept ours, per the decision above; the
  manifest records the drop's bytes. Equivalent shaders, different bytes.
- `crates/unendlich-engine/src/sim/spawn.rs` — monorepo-side change the drop
  predates; the merge correctly kept ours.

**Left as-is deliberately.** Regenerating would make it a truthful baseline for
the merged tree, but it is the external line's worker-workflow artifact and
whether to re-baseline is their call, not the merge's. To do it:

    cd games/unendlich && find . -type f -not -path './.git/*' \
      -not -name MANIFEST.sha256 | sort | xargs sha256sum > MANIFEST.sha256

## gitmind

Nothing to commit. `.gitmind/store.redb` is gitignored and regenerated from the
git graph, which these merges have just changed. On hardware:

    maintenance/gitmind-sync.sh

`gin` cannot be built in the authoring sandbox (no Rust toolchain), as
`LAYOUT.md` anticipates.

One process note for `.gitmind/README.md`'s "Receiving a new bundle" recipe:
it assumes the new history either fast-forwards or that you rebase onto it.
Neither applied here — these drops are *zip archives from divergent lines*,
not bundles of descendant history, so the sibling-branch merge above is the
shape to reuse for that case. Take the `gin snapshot` checkpoint first either
way.

### Restoring files is not enough — the DECLARATIONS go too

Caught on a post-merge audit, after the merge had already been committed.

Restoring the 4 missing files was necessary but insufficient. The drop's
`lib.rs` also lacks the *module declarations* for them, for the same reason it
lacks the files. Taking the drop's `lib.rs` therefore deleted
`pub mod pngutil;` (added by `d59cf94`) and orphaned a file that is present and
is public API — `unendlich_engine::pngutil::write_png`. Fixed in `9925b22`.

The general lesson, and the reason the `.gitmind` note above matters: when a
drop lacks something the monorepo has, the loss shows up in **two** places —
the file, and every place that wires it in. Restoring only the first leaves a
tree that looks complete and is not.

Audited the whole class rather than the one symptom — diffed every `mod X;`
declaration, every `[[bin]]` target and every `[features]` entry between the
pre-merge bundle and the merged tree:

- `pub mod pngutil;` — real loss, restored.
- `pub mod infusion;` — false positive. The drop moved the r594 comment onto a
  new adjacent `infusion_catalog` module; `infusion` is still declared.
- No `[[bin]]` targets or features lost.
- kbm3: no declarations lost or gained.
- blackstar: the equivalent surface is the CMake source list, already proven by
  the 64/64 ctest run.

### PRE-EXISTING: sim/netview.rs is wired to nothing

Found while auditing the above; **not** caused by this merge and not fixed
here. `crates/unendlich-engine/src/sim/netview.rs` is present, and the
auto-discovered `src/bin/unendlich-netview.rs` does
`use unendlich_engine::sim::netview::snapshot;` — but `pub mod netview;`
appears nowhere in `sim/mod.rs`, on the pre-merge bundle or after. The bin has
no `[[bin]]` entry, so cargo auto-discovers and builds it, which means
`cargo check --all-targets` should already have been failing.

The likely cause is this exact failure mode one drop earlier, at `000e92d`
(the r742 adoption) — which is why it is worth writing the pattern down rather
than just patching it.

Left alone deliberately: there is no rust toolchain here (see above) to
determine whether declaring the module builds clean or merely surfaces further
errors in a file that has been dead through two drops. One line in
`sim/mod.rs` if it turns out to be the former.

## Whole-tree verification

`./verify.sh` on the merged tree reports `projects-with-real-problems=1`
(`rusty-board`). Running it on the **pre-merge bundle** gives byte-identical
output — that problem predates this work, and these merges introduce no new
findings. Note `verify.sh` covers `apps/`, `system/` and `tools/{peek,drift}`
only; it does not reach `games/`, which is why the blackstar ctest run above
carries the weight for that project.

`git fsck` clean. The output bundle verifies as complete history and
round-trips through a fresh clone, from which the blackstar build and 64/64
ctest were run.

## Still open

- `sim::netview` is wired to nothing (pre-existing, see above) — one line in
  `sim/mod.rs`, once a rust host can confirm it builds clean.
- `TODO(design)` in `games/blackstar/engine_src/game.c` — pick one escalation
  mechanic and retire the other. Also check anything persisting a mode index:
  `EX_MODE_ESCALATION` moved from ordinal 2 to 4.
- `cargo check` for kbm3 and unendlich, and `apps/gitmind`'s `gin` — all need
  a rust ≥1.85 host (see above).
- `maintenance/gitmind-sync.sh` on hardware, once `gin` builds. `.gitmind`
  needs nothing committed; `store.redb` is gitignored and regenerates from the
  git graph these merges just changed.
- Optionally re-baseline `games/unendlich/MANIFEST.sha256` (see above).
