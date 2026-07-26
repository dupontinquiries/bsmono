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

Verified: all 54 module sources pass `gcc -fsyntax-only`, all 54 ctest targets
resolve to real sources. No cmake in the authoring sandbox, so configure +
ctest stay a hardware step.

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

Not verified: no Rust toolchain in the sandbox, so `cargo check` is a hardware
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

## Still open

- `TODO(design)` in `games/blackstar/engine_src/game.c` — pick one escalation
  mechanic and retire the other.
- cmake configure + ctest for blackstar (needs a cmake host).
- `cargo check` for kbm3 and unendlich (needs a Rust toolchain).
- `maintenance/gitmind-sync.sh` on hardware.
