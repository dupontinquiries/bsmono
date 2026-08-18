# Two launchers arrived, and only one can be the launcher

Two incoming branches both add a fullscreen Plasma app launcher — an egui
shell plus a QML plasmoid — and both were written on 2026-08-16 from the same
base commit (`71163e4e`). Merging both would leave the monorepo shipping two
launchers that do the same job, register separate manifest entries, and each
claim `primary = true` for their own port.

Both are real work and both pass their tests on this host:

| | `incoming/launcher` → `apps/launcher` | `incoming/hatch2` → `apps/hatch` |
|---|---|---|
| commits | 5 | 10 |
| insertions | 9,168 | 9,602 |
| tests here | 36 (egui-frontend) | 42 core + 24 egui = 66 |
| headless core | none — `desktop.rs`/`icontheme.rs` live in the frontend | `crates/hatch-core`, std-only |
| plasmoid | `plasmoid/` + config UI | `qml/` + `packaging/` |

Neither dominates, which is why this is written down instead of decided by
whoever merged first.

## What each one has that the other does not

**`launcher` has the presentation work.**

* `icontheme.rs` — real freedesktop lookup: `index.theme` parsing, directory
  size-matching, the `Inherits` chain, hicolor fallback. Nine tests run
  against this build host's actual Adwaita and hicolor trees.
* `anim.rs` — a headless tick-based `Board` driving slide-in and restack, so
  filtering and restack convergence run under `cargo test` with no display.
  This is the animation system the `fullscreen-launcher` QML docs describe
  (eight icon styles, tile-geometry slide-in patterns).
* SVG rasterized at the tile's on-screen size via resvg, rather than at the
  icon's often-tiny intrinsic viewBox.
* A plasmoid config surface — `main.xml` plus `ConfigAppearance.qml` — which
  is what makes the animation styles user-settable.

**`hatch` has the architecture.**

* A separate std-only `hatch-core` crate: `.desktop` scan, `Exec` spawn, icon
  resolve, fuzzy rank, favorites and recents through config-kit. This is the
  headless-core-then-shell seam the rest of the monorepo uses, and it is why
  hatch has 42 tests that need no egui at all.
* Single-instance with a poke-file toggle (`hatch-egui --toggle`), which a
  launcher bound to Meta actually needs.
* A resident process that notices installs — directory stamps and the cache
  they invalidate.
* Node tests for the plasmoid's `tools.js` and slide geometry: the QML logic
  tested off the phone.
* Favorites seeded from Plasma rather than synced to it — a deliberate call,
  documented in its commit.

## Recommendation

Take **hatch as the base** and port launcher's presentation work onto it.

The reason is not test count. It is that hatch's split matches what this
monorepo is for: a headless core that runs under `cargo test` with no
display, and shells over it. `launcher` puts `.desktop` parsing and icon
theming inside the egui frontend, so a second shell — the plasmoid — cannot
reuse any of it, which is the exact problem the fleet-wide port strategy
exists to avoid.

Ported in this order, each step independently testable:

1. ~~`launcher/icontheme.rs` → `hatch-core/icons.rs`.~~ **Done.** Ported whole
   with its nine tests, as `hatch-core/src/icontheme.rs`, and `icons::resolve`
   now tries it first. The grid walk stays behind it as the fallback for
   themes that ship no `index.theme` — the spec implementation cannot resolve
   those at all, so replacing rather than layering would have lost a case.
   hatch-core: 42 → 53 tests.
2. `launcher/anim.rs` (the `Board`) → `hatch-core`, or a sibling crate if it
   pulls anything egui-shaped. It is already headless and tick-based, so it
   belongs beside the model rather than in a shell.
3. The plasmoid config surface — `main.xml` and `ConfigAppearance.qml` — onto
   hatch's `qml/`, so the animation settings have somewhere to live.
4. `apps/launcher` is then deleted, and its manifest entries with it. One
   launcher, one `primary = true`.

## Do not

Merge both branches and rename one. The monorepo would carry two `.desktop`
scanners, two icon-theme resolvers and two plasmoids, and the next person to
touch a launcher bug would have to find out which one is installed before
they could reproduce it.

## Status

**hatch is merged** (`8444e17b`), clean. Steps 1 and 2 are done: `icontheme`
and now the animation `Board` plus its `grid`, in `hatch-core` at 67 tests.
Steps 3 and 4 — the plasmoid config surface, and deleting `apps/launcher` —
remain.

The Board moved with one change. It imported `uikit_egui::motion::ease`, and
`motion.rs` pulls `egui::{Color32, Context, Id, Response}` at file scope — so
importing it would put the whole UI crate into a crate that is headless on
purpose. It uses exactly ONE curve, `out_cubic`, four lines of arithmetic,
which is inlined with a test pinning it to the shared values. If `ease` is
ever split into an egui-free module the copy should go away; two copies of a
curve is a real cost, just smaller than the dependency.

`grid` needed no change at all — 257 lines with no egui import.

`incoming/launcher` stays as a ref until they are, because it is still the
only copy of that work:

```
git log incoming/launcher     # apps/launcher
git log incoming/hatch2       # apps/hatch
```

Neither is merged into `main`. The base commit both fork from is
`71163e4e`, which `main` is well ahead of, so whichever lands will be a real
merge rather than a fast-forward.

## Also in the same upload, not yet assessed

* `fullscreen-launcher/` — QML sources and two design docs (icon animations,
  tile geometry) for the Plasma plasmoid. Source material for step 3 above
  rather than a separate thing to merge.
* `studio-trap/` — a DAW, `apps/studio`, and it DOES fetch here. I wrote the
  opposite in this file's first commit: `git bundle verify` printed a base ref
  and I read "requires" as "is missing" without checking, when
  `git cat-file -t` on that ref answers `commit` in one command. Corrected
  the same day, but the lesson is that "requires" in that output names a
  prerequisite, not a failure.

  Three bundles, one lineage: `studio-trap-work` (6 commits) is an ancestor
  of `autotune-bundle-02` (23), and `studio-session.bundle` inside the v2 zip
  has the identical tip `f47ca49e` — a duplicate, not a third branch. All
  three fork from `3e6b64d9`, a commit this repo has.

  Fetched as `incoming/trap`, `incoming/trap-autotune`, `incoming/trap-session`.
  The tip adds twelve crates under `apps/studio/crates` — core, engine, audio,
  fx, io, live, media, command, collab, copilot, app, egui — so it is a
  separate product from the launchers, not a competing implementation, and
  merging it is an independent decision from the one above. Its own
  assessment still to do.

* `studio-session.mbox` — an mbox alongside the bundles, unexamined.

---

# Addendum: the two blackstar engines (2026-08-17)

Both engines stay — the C exengine build and blackstar-rs — and the goal is to
port each one's niceties to the other. Two findings from looking at what is
already there.

## Rust → C is written and stranded

`games/blackstar/engine_src/botpack.h` is a complete 176-line C reader for
blackstar-rs's `bots.pack`, with the format documented faithfully (including
the one-box-per-chassis rule and the u32-index bug that motivated it).
**Nothing includes it.** Not `game.c`, not `CMakeLists.txt`. It is the same
unreachable-module problem this session spent most of its time on, in C.

Its own header corrects something we believed: `imported_models.h` lists 1,012
`data/imported_*.iqm` and **none of them are in the repo** — only 21 survive
under `data/examples/`. So the C engine's imported roster already loads
nothing, and has for some time. The 68 IQMs under `engine_src/data` are the
arena and the survivors, not the roster.

That reframes the IQM question. Removing IQM from the C engine is not the
prerequisite it looked like: the roster is already dead, and `botpack.h` gives
it 173 chassis it wants and cannot load, at ~33% of what the IQM would have
cost. Wiring that header in is the highest-value C-side task available, and it
needs a machine that can build the CMake target.

## C → Rust is the bigger prize

exengine's IQM pipeline is a mature skeletal path that blackstar-rs does not
have. blackstar-rs has `rig.rs` (glTF skins, animation channels, measured
against a real 83-joint clip) and `examples/rigpose.rs` proving runtime pose
composition works — but its shipped fleet is 1,454 BAKED pose glTFs, because
the rigged sources are absent.

If blackstar-rs gains a real skinned-model path, baked poses stop being archive
and become regenerable. That is the 216 MB of `bots.pack` plus 46 MB of pose
JSON — an order of magnitude more than any compression work discussed here.
It is blocked on the same missing `.iqm` sources, so it is a question of
whether those exist outside the repo, not of engine work.

## What was decided and not done

* Clawbound's 17 animal IQMs were shelved (18 MB): the spirits are a staging
  harness, not a gamemode, and the models are re-obtainable. Loader, roster and
  parser all stay; three tests skip when the directory is absent and are fully
  strict when it is present.
* Applying the parts-web to clawbound was considered and dropped. The web's win
  is dedup — 40,742 parts from 1,012 vehicles, a roadwheel stored once — and 17
  unique organic models share nothing. The *encoding* half (16-bit quantisation,
  varint delta indices) would help, but that is `bsweb` extended with skin data,
  not a second encoder.

---

# voiceterm: terminal-output-as-menu is disconnected, not just undrawn

`detect::picker_ui` shows on the uncalled-renderer list, and wiring it would
have been building on nothing. The gap is a level deeper.

`entity_extract` — 1,700 lines in `apps/voiceterm/src/`, with
`resolve_reference`, `extract_block`, `extract_top_processes`, `parse_verb` —
is the engine behind "run `bluetoothctl devices`, then say *connect to the
headphones*". In the egui port, the only file that mentions it is `detect.rs`
itself, and `detect.rs` mounts it by `#[path]`.

So the chain is broken at the first link:

```
terminal output  ──X──>  extract_block / extract_top_processes
                         (never called from the port)
                              │
                         resolve_reference
                              │
                         detect::decide  ──>  EntityPicker::ask
                                                   │
                                              picker_ui  (never called)
```

`EntityPicker` is not a field on the app, no command output is fed to an
extractor, and no voice phrase reaches `resolve_reference`. Drawing the picker
would give an empty dialog that can never open — `ask` refuses fewer than two
rows on purpose, so it would refuse every time.

**What it needs, in order.** Hold the last N `ExtractedBlock`s as command
output arrives; route an unresolved reference from the voice grammar through
`resolve_reference`; then `decide` picks between acting, asking and refusing,
and only the ASK case draws the picker. That is a feature, and it should be
built as one rather than reached by wiring a renderer to state that does not
exist.

Noted here rather than fixed because the honest version is a session's work,
and a picker wired to nothing would look finished on the audit while being
less useful than the current state, where at least nothing claims to work.

---

# voiceterm: three bottom bars, one drawn, none of them the fan-out one

`archived_view::bar_ui` is the last renderer on the uncalled list that is not
blocked on a missing feature, and wiring it would add a fourth path rather than
finish a third.

What exists:

| path | state | drawn |
|---|---|---|
| `shell::keycap_bar_ui` | `TERMINAL_KEYCAPS`, a const | **yes** |
| `bars::BarState` | field on the app, constructed | no — never read after `Default::default()` |
| `archived_view::bar_ui` | takes a `&BottomBar` | no |

`archived_view` and `bars` both `pub use` the same engine types from
`src/bottom_bar.rs`, so the two are views over one model, and neither is
reached. The bar the app actually draws is the keycap row, which is a different
thing: fixed keys that send, not buttons that fan out into option lists.

**Why this is not a wiring job.** `bar_ui` takes a `&BottomBar` — a set of
buttons with their options — and nothing in the app builds one. There is no
`BottomBar` anywhere outside the two view modules and their tests. Supplying
one means deciding what the buttons ARE, which is a product question about what
voiceterm's bottom bar should offer, not a missing call.

The engine's rule is worth keeping in view when that decision is made: *a button
with no options must not fan out into an empty popup — an empty popup reads as a
broken button, where a disabled one reads as "nothing here"*. Any invented
button set would hit that immediately, which is a good sign the set has to come
from someone who knows what the bar is for.

`bars::BarState` should probably go at the same time. Two view modules over one
engine model, both unreached, is a duplicate to resolve rather than two things
to wire.
